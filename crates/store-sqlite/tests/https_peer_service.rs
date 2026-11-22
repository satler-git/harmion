//! Signed-HTTPS v2 challenge/exchange service over the durable SQLite store.
//!
//! These tests do not use HTTP or Cloudflare. They prove the bounded durable
//! challenge CAS, freshness bindings and the node-handler hand-off.
use harmion_domain::{ClusterId, NodeId};
use harmion_node::{
    Error,
    access::Credential,
    application::{self, Host},
    https_peer::{self, ChallengeStore, Config, ExchangeOutcome, PeerV2Response},
    membership::{self, PeerKey},
    membership_state::{self, MembershipSigner, Status},
    storage::{Identity, Store},
};
use harmion_protocol::{
    LogEpoch, PeerCommand, PeerRequest, encode_peer_request,
    https_peer::{Body, Commitment, Common, ErrorCode, Frame, PayloadClass, ResponseResult},
    membership::{Binding, Member},
};
use harmion_store_sqlite::SqliteStore;
use std::path::PathBuf;

const CLUSTER: ClusterId = ClusterId([1; 32]);
const SERVER_ORIGIN: &str = "https://server.example";
const CLIENT_ORIGIN: &str = "https://client.example";

struct Entropy(u8);
impl Host for Entropy {
    fn random_bytes(&mut self) -> Result<[u8; 32], Error> {
        self.0 += 1;
        Ok([self.0; 32])
    }
    fn now_micros(&self) -> i64 {
        0
    }
}
fn signer(n: u8) -> MembershipSigner {
    MembershipSigner::from_bytes(&[n; 32])
}
fn node(n: u8) -> NodeId {
    NodeId([n; 32])
}
fn entropy() -> impl FnMut() -> Result<[u8; 32], Error> {
    let mut n = 0u8;
    move || {
        n = n.wrapping_add(1);
        Ok([n; 32])
    }
}
fn class_of(inner: &[u8]) -> PayloadClass {
    match harmion_protocol::decode_peer_request(inner)
        .unwrap()
        .command
    {
        PeerCommand::MembershipStatus
        | PeerCommand::MembershipCursor { .. }
        | PeerCommand::MembershipPage { .. }
        | PeerCommand::MembershipAccept(_) => PayloadClass::Evidence,
        _ => PayloadClass::Data,
    }
}
fn hpq(from: u8, command: PeerCommand) -> Vec<u8> {
    encode_peer_request(&PeerRequest {
        cluster: CLUSTER,
        node: node(from),
        command,
    })
    .unwrap()
}
fn common(sender: u8, audience: u8) -> Common {
    Common {
        cluster: CLUSTER,
        sender: node(sender),
        audience: node(audience),
        origin: SERVER_ORIGIN.to_owned(),
    }
}

struct Test {
    path: Option<PathBuf>,
    db: SqliteStore,
    identity: Identity,
    server: MembershipSigner,
    client: MembershipSigner,
    admin: Credential,
}
impl Test {
    fn new() -> Self {
        Self::at(None)
    }
    fn at(path: Option<PathBuf>) -> Self {
        if let Some(path) = &path {
            std::fs::create_dir_all(path).unwrap();
        }
        let identity = Identity {
            cluster: CLUSTER,
            node: node(2),
            epoch: LogEpoch([2; 32]),
        };
        let db_path = path
            .as_ref()
            .map(|path| path.join("db"))
            .unwrap_or_else(|| PathBuf::from(":memory:"));
        let mut db = SqliteStore::open(db_path, identity).unwrap();
        let server = signer(2);
        let client = signer(3);
        let (_, admin) = application::bootstrap_admin(&mut db, &mut Entropy(10), i64::MAX).unwrap();
        membership_state::initialize(
            &mut db,
            &server,
            Member {
                node: node(2),
                signing_key: server.public_key(),
                binding: Binding::SignedHttps(server.public_key()),
                endpoint: SERVER_ORIGIN.to_owned(),
            },
            true,
        )
        .unwrap();
        membership_state::approve(
            &mut db,
            0,
            &admin,
            &server,
            CLUSTER,
            Member {
                node: node(3),
                signing_key: client.public_key(),
                binding: Binding::SignedHttps(client.public_key()),
                endpoint: CLIENT_ORIGIN.to_owned(),
            },
            [9; 32],
        )
        .unwrap();
        Self {
            path,
            db,
            identity,
            server,
            client,
            admin,
        }
    }
    fn reopen(&mut self) {
        let path = self.path.as_ref().expect("file database").join("db");
        self.db = SqliteStore::open(path, self.identity).unwrap();
    }
    fn request(&self, inner: &[u8], client_nonce: [u8; 32]) -> Frame {
        self.client
            .sign_challenge_request(
                common(3, 2),
                client_nonce,
                Commitment::from_request(inner).unwrap(),
            )
            .unwrap()
    }
    fn challenge(&mut self, inner: &[u8], client_nonce: [u8; 32], now: i64) -> Frame {
        let request = self.request(inner, client_nonce);
        let config = config();
        let session = https_peer::Session {
            signer: &self.server,
            config: &config,
            local: self.identity,
            requester: node(3),
            now,
        };
        let response =
            https_peer::issue_challenge(&mut self.db, &session, &request, &mut entropy()).unwrap();
        match response {
            PeerV2Response::Challenge(frame) => frame,
            other => panic!("expected challenge, got {other:?}"),
        }
    }
    fn exchange(&mut self, challenge: &Frame, inner: &[u8], now: i64) -> (ExchangeOutcome, Frame) {
        let Body::Challenge {
            challenge_request_hash,
            client_nonce,
            generation,
            challenge_nonce,
            ..
        } = &challenge.body
        else {
            panic!("challenge frame")
        };
        let context = harmion_protocol::https_peer::ExchangeContext {
            challenge_request_hash: *challenge_request_hash,
            challenge_hash: challenge.hash().unwrap(),
            client_nonce: *client_nonce,
            generation: *generation,
            challenge_nonce: *challenge_nonce,
        };
        let request = self
            .client
            .sign_exchange(common(3, 2), context, class_of(inner), inner.to_vec())
            .unwrap();
        let config = config();
        let session = https_peer::Session {
            signer: &self.server,
            config: &config,
            local: self.identity,
            requester: node(3),
            now,
        };
        let outcome =
            https_peer::begin_exchange(&mut self.db, &session, &request, &mut entropy()).unwrap();
        (outcome, request)
    }
    fn dispatch(&mut self, consumed: &https_peer::ConsumedExchange) -> PeerV2Response {
        let result = membership::handle(
            &mut self.db,
            &PeerKey::SignedHttps(self.client.public_key()),
            &consumed.request,
        )
        .and_then(|response| Ok(harmion_protocol::encode_peer_response(&response)?));
        let config = config();
        let session = https_peer::Session {
            signer: &self.server,
            config: &config,
            local: self.identity,
            requester: node(3),
            now: 0,
        };
        https_peer::complete_exchange(&session, consumed, result).unwrap()
    }
}
impl Drop for Test {
    fn drop(&mut self) {
        if let Some(path) = &self.path {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

fn config() -> Config {
    Config {
        origin: SERVER_ORIGIN.to_owned(),
    }
}

fn issue(t: &mut Test, request: &Frame, now: i64) -> Result<PeerV2Response, Error> {
    let config = config();
    let session = https_peer::Session {
        signer: &t.server,
        config: &config,
        local: t.identity,
        requester: node(3),
        now,
    };
    https_peer::issue_challenge(&mut t.db, &session, request, &mut entropy())
}

fn challenge_code(response: &PeerV2Response) -> Option<ErrorCode> {
    match response {
        PeerV2Response::ChallengeError(frame) => match &frame.body {
            Body::ChallengeError { error, .. } => Some(error.code),
            _ => None,
        },
        _ => None,
    }
}

#[test]
fn challenge_is_bound_and_replay_of_the_exchange_is_rejected() {
    let mut t = Test::new();
    let inner = hpq(3, PeerCommand::MembershipStatus);
    let challenge = t.challenge(&inner, [4; 32], 1_000_000);
    // The client verifies the server proof with its already-trusted key.
    https_peer::verify_frame(&challenge, &t.server.public_key()).unwrap();
    assert!(https_peer::verify_frame(&challenge, &t.client.public_key()).is_err());
    let request_hash = t.request(&inner, [4; 32]).hash().unwrap();
    let Body::Challenge {
        challenge_request_hash,
        client_nonce,
        commitment,
        ttl_ms,
        ..
    } = &challenge.body
    else {
        panic!("challenge")
    };
    assert_eq!(*client_nonce, [4; 32]);
    assert_eq!(*challenge_request_hash, request_hash);
    assert_eq!(commitment, &Commitment::from_request(&inner).unwrap());
    assert!(*ttl_ms > 0 && *ttl_ms <= 60_000);

    let (outcome, _) = t.exchange(&challenge, &inner, 1_000_000);
    let ExchangeOutcome::Consumed(consumed) = outcome else {
        panic!("challenge should be consumed");
    };
    let response = t.dispatch(&consumed);
    let PeerV2Response::Exchange(frame) = &response else {
        panic!("response")
    };
    assert_eq!(response.http_status(), 200);
    https_peer::verify_frame(frame, &t.server.public_key()).unwrap();
    let Body::Response { result, .. } = &frame.body else {
        panic!("response body")
    };
    let ResponseResult::Peer(bytes) = result else {
        panic!("peer reply")
    };
    let reply = harmion_protocol::decode_peer_response(bytes).unwrap();
    assert_eq!(reply.cluster, CLUSTER);
    assert_eq!(reply.node, node(2));
    assert!(matches!(
        reply.reply,
        harmion_protocol::PeerReply::Status { .. }
    ));

    // The consumed challenge cannot authorize a second dispatch.
    let (outcome, _) = t.exchange(&challenge, &inner, 1_000_000);
    let ExchangeOutcome::Rejected(rejected) = outcome else {
        panic!("replay must be rejected");
    };
    assert_eq!(rejected.http_status(), 409);
    assert!(matches!(
        rejected,
        PeerV2Response::Exchange(ref frame)
            if matches!(&frame.body, Body::Response { result: ResponseResult::Error(error), .. }
                if error.code == ErrorCode::ChallengeUnavailable)
    ));
}

#[test]
fn duplicate_challenge_requests_return_the_identical_frame_without_renewal() {
    let mut t = Test::new();
    let inner = hpq(3, PeerCommand::MembershipStatus);
    let first = t.challenge(&inner, [4; 32], 1_000_000);
    let second = t.challenge(&inner, [4; 32], 1_000_000);
    assert_eq!(first.encode().unwrap(), second.encode().unwrap());

    // A different commitment under the same live client nonce is not a new row.
    let other = hpq(3, PeerCommand::Status);
    let request = t.request(&other, [4; 32]);
    let response = issue(&mut t, &request, 1_000_000).unwrap();
    assert_eq!(challenge_code(&response), Some(ErrorCode::BindingMismatch));
    let count =
        t.db.challenge_transaction(|tx| tx.count_challenges(Some(node(3))))
            .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn clock_reversal_fails_closed_and_leaves_live_challenges_usable() {
    let mut t = Test::new();
    let inner = hpq(3, PeerCommand::MembershipStatus);
    let challenge = t.challenge(&inner, [4; 32], 2_000_000);
    let request = t.request(&inner, [5; 32]);
    assert!(matches!(
        issue(&mut t, &request, 1_000_000),
        Err(Error::ClockReversal)
    ));
    let (outcome, _) = t.exchange(&challenge, &inner, 2_000_000);
    assert!(matches!(outcome, ExchangeOutcome::Consumed(_)));
}

#[test]
fn generation_reset_clears_clock_high_water() {
    let path =
        std::env::temp_dir().join(format!("harmion-https-peer-reset-{}", std::process::id()));
    let mut t = Test::at(Some(path));
    let inner = hpq(3, PeerCommand::MembershipStatus);
    t.challenge(&inner, [4; 32], 5_000_000);
    assert!(matches!(
        t.db.challenge_transaction(|tx| tx.clock(1_000_000, &mut entropy())),
        Err(Error::ClockReversal)
    ));
    t.db.challenge_transaction(|tx| tx.reset_generation(&mut || Ok([91; 32])))
        .unwrap();
    // The recovery command runs in a new process; reopen to reset the runtime
    // monotonic clamp while keeping the durable state under test.
    t.reopen();
    let clock =
        t.db.challenge_transaction(|tx| tx.clock(1_000_000, &mut entropy()))
            .unwrap();
    assert_eq!(clock.now, 1_000_000);
}

#[test]
fn expired_challenges_are_unavailable_and_never_dispatch() {
    let mut t = Test::new();
    let inner = hpq(3, PeerCommand::MembershipStatus);
    let challenge = t.challenge(&inner, [4; 32], 1_000_000);
    let (outcome, _) = t.exchange(&challenge, &inner, 1_000_000 + 60_000_001);
    let ExchangeOutcome::Rejected(rejected) = outcome else {
        panic!("expired challenge must not dispatch");
    };
    assert_eq!(rejected.http_status(), 409);
}

#[test]
fn challenge_rows_are_bounded_per_requester_and_spent_rows_are_reclaimed() {
    let mut t = Test::new();
    let inner = hpq(3, PeerCommand::MembershipStatus);
    let mut issued = Vec::new();
    for n in 0..https_peer::MAX_CHALLENGES_PER_REQUESTER {
        issued.push(t.challenge(&inner, [n as u8 + 1; 32], 1_000_000));
    }
    let request = t.request(&inner, [200; 32]);
    let response = issue(&mut t, &request, 1_000_000).unwrap();
    assert_eq!(challenge_code(&response), Some(ErrorCode::Capacity));

    // Spent rows are reclaimed before capacity checks instead of imposing a
    // fixed commands-per-minute ceiling.
    let first = issued.remove(0);
    let (outcome, _) = t.exchange(&first, &inner, 1_000_000);
    assert!(matches!(outcome, ExchangeOutcome::Consumed(_)));
    let request = t.request(&inner, [201; 32]);
    let response = issue(&mut t, &request, 1_000_000).unwrap();
    assert!(matches!(response, PeerV2Response::Challenge(_)));
}

#[test]
fn explicit_generation_reset_fences_live_challenges() {
    let mut t = Test::new();
    let inner = hpq(3, PeerCommand::MembershipStatus);
    let challenge = t.challenge(&inner, [4; 32], 1_000_000);
    let old =
        t.db.challenge_transaction(|tx| tx.clock(1_000_000, &mut entropy()))
            .unwrap()
            .generation;
    let new =
        t.db.challenge_transaction(|tx| tx.reset_generation(&mut || Ok([77; 32])))
            .unwrap();
    assert_ne!(old, new);
    assert_eq!(new, [77; 32]);
    let (outcome, _) = t.exchange(&challenge, &inner, 1_000_000);
    assert!(matches!(outcome, ExchangeOutcome::Rejected(_)));
}

#[test]
fn cold_start_preserves_generation_rows_and_high_water() {
    let path =
        std::env::temp_dir().join(format!("harmion-https-peer-service-{}", std::process::id()));
    let mut t = Test::at(Some(path));
    let inner = hpq(3, PeerCommand::MembershipStatus);
    let challenge = t.challenge(&inner, [4; 32], 1_000_000);
    t.reopen();
    // Identical bytes after restart: durable generation and stored fields, and
    // the duplicate is not reissued or renewed.
    let again = t.challenge(&inner, [4; 32], 1_000_000);
    assert_eq!(challenge.encode().unwrap(), again.encode().unwrap());
    let (outcome, _) = t.exchange(&challenge, &inner, 1_000_000);
    assert!(matches!(outcome, ExchangeOutcome::Consumed(_)));
    t.reopen();
    assert!(matches!(
        t.db.challenge_transaction(|tx| tx.clock(500_000, &mut entropy())),
        Err(Error::ClockReversal)
    ));
}

#[test]
fn retirement_blocks_data_but_allows_pinned_evidence_transport() {
    let mut t = Test::new();
    membership_state::retire(&mut t.db, 0, &t.admin, &t.server, node(3), [7; 32]).unwrap();
    assert!(matches!(
        t.db.transaction(|tx| Ok(
            membership_state::state(tx).unwrap().unwrap().nodes[&node(3)].clone()
        )),
        Ok(Status::Retired)
    ));

    let evidence = hpq(3, PeerCommand::MembershipStatus);
    let challenge = t.challenge(&evidence, [4; 32], 1_000_000);
    let (outcome, _) = t.exchange(&challenge, &evidence, 1_000_000);
    let ExchangeOutcome::Consumed(consumed) = outcome else {
        panic!("evidence exchange must consume")
    };
    assert_eq!(t.dispatch(&consumed).http_status(), 200);

    let data = hpq(3, PeerCommand::Status);
    let challenge = t.challenge(&data, [5; 32], 1_000_000);
    let (outcome, _) = t.exchange(&challenge, &data, 1_000_000);
    let ExchangeOutcome::Consumed(consumed) = outcome else {
        panic!("data exchange must consume, not reuse")
    };
    let response = t.dispatch(&consumed);
    assert_eq!(response.http_status(), 403);
    assert!(matches!(
        response,
        PeerV2Response::Exchange(ref frame)
            if matches!(&frame.body, Body::Response { result: ResponseResult::Error(error), .. }
                if error.code == ErrorCode::NotAuthorized)
    ));
}
