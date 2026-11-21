//! Worker/DO entry over the memory host boundary, against the native node store.
use harmion_domain::{ClusterId, NodeId};
use harmion_node::{
    Error,
    access::Credential,
    application::{self, Host},
    https_peer::{ChallengeStore, Config},
    identity::Provenance,
    membership_state::{self, MembershipSigner},
    storage::{Identity, Store},
};
use harmion_protocol::{
    LogEpoch, PeerCommand, PeerRequest, encode_peer_request,
    https_peer::{Body, Commitment, Common, ErrorCode, Frame, ResponseResult},
    membership::{Binding, Member},
};
use harmion_store_sqlite::SqliteStore;
use harmion_worker::{MemoryKv, Request, WorkerChallenges, entry, is_signed_error};

const CLUSTER: ClusterId = ClusterId([1; 32]);
const ORIGIN: &str = "https://server.example";
const HOST: &str = "server.example";

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
fn config() -> Config {
    Config {
        origin: ORIGIN.to_owned(),
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
        origin: ORIGIN.to_owned(),
    }
}

struct Test {
    db: SqliteStore,
    identity: Identity,
    server: MembershipSigner,
    client: MembershipSigner,
    challenges: WorkerChallenges<MemoryKv>,
    admin: Credential,
}
impl Test {
    fn new() -> Self {
        let identity = Identity {
            cluster: CLUSTER,
            node: node(2),
            epoch: LogEpoch([2; 32]),
        };
        let mut db = SqliteStore::open(":memory:", identity).unwrap();
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
                endpoint: String::new(),
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
                endpoint: "https://client.example".to_owned(),
            },
            [9; 32],
        )
        .unwrap();
        Self {
            db,
            identity,
            server,
            client,
            challenges: WorkerChallenges::new(MemoryKv::new()),
            admin,
        }
    }
    fn call(
        &mut self,
        path: &str,
        body: &[u8],
        host: Option<&str>,
        origin: bool,
        now: i64,
    ) -> (u16, Vec<u8>) {
        let config = config();
        let context = entry::HostContext {
            signer: &self.server,
            config: &config,
            local: self.identity,
            now,
        };
        let response = entry::handle(
            &mut self.db,
            &mut self.challenges,
            &context,
            Request {
                method: "POST",
                path,
                host,
                has_origin: origin,
                body,
            },
            &mut entropy(),
        );
        (response.status, response.body)
    }
    fn challenge_request(&self, inner: &[u8], nonce: [u8; 32]) -> Frame {
        self.client
            .sign_challenge_request(
                common(3, 2),
                nonce,
                Commitment::from_request(inner).unwrap(),
            )
            .unwrap()
    }
    fn restart(&mut self) {
        let old = std::mem::replace(&mut self.challenges, WorkerChallenges::new(MemoryKv::new()));
        self.challenges = WorkerChallenges::new(old.into_host());
    }
}

#[test]
fn worker_entry_serves_the_challenge_exchange_round_trip() {
    let mut t = Test::new();
    let inner = hpq(3, PeerCommand::MembershipStatus);
    let request = t.challenge_request(&inner, [4; 32]);
    let (status, body) = t.call(
        harmion_protocol::https_peer::CHALLENGE_PATH,
        &request.encode().unwrap(),
        Some(HOST),
        false,
        1_000_000,
    );
    assert_eq!(status, 200);
    let challenge = Frame::decode(&body).unwrap();
    harmion_node::https_peer::verify_frame(&challenge, &t.server.public_key()).unwrap();
    // The DO host flushes before releasing an outcome.
    t.challenges.sync().unwrap();
    assert_eq!(t.challenges.host().syncs(), 1);

    let Body::Challenge {
        challenge_request_hash,
        client_nonce,
        generation,
        challenge_nonce,
        ..
    } = &challenge.body
    else {
        panic!("challenge")
    };
    let context = harmion_protocol::https_peer::ExchangeContext {
        challenge_request_hash: *challenge_request_hash,
        challenge_hash: challenge.hash().unwrap(),
        client_nonce: *client_nonce,
        generation: *generation,
        challenge_nonce: *challenge_nonce,
    };
    let exchange = t
        .client
        .sign_exchange(
            common(3, 2),
            context,
            harmion_protocol::https_peer::PayloadClass::Evidence,
            inner.clone(),
        )
        .unwrap();
    let (status, body) = t.call(
        harmion_protocol::https_peer::EXCHANGE_PATH,
        &exchange.encode().unwrap(),
        Some(HOST),
        false,
        1_000_000,
    );
    assert_eq!(status, 200);
    let response = Frame::decode(&body).unwrap();
    harmion_node::https_peer::verify_frame(&response, &t.server.public_key()).unwrap();
    let Body::Response {
        result: ResponseResult::Peer(bytes),
        ..
    } = &response.body
    else {
        panic!("peer response")
    };
    let reply = harmion_protocol::decode_peer_response(bytes).unwrap();
    assert!(matches!(
        reply.reply,
        harmion_protocol::PeerReply::Status { .. }
    ));

    // Replaying the consumed exchange yields a signed 409 and no second dispatch.
    let (status, body) = t.call(
        harmion_protocol::https_peer::EXCHANGE_PATH,
        &exchange.encode().unwrap(),
        Some(HOST),
        false,
        1_000_000,
    );
    assert_eq!(status, 409);
    assert_eq!(
        is_signed_error(&body),
        Some(ErrorCode::ChallengeUnavailable)
    );
}

#[test]
fn unknown_requesters_and_bad_routes_allocate_nothing() {
    let mut t = Test::new();
    let inner = hpq(9, PeerCommand::MembershipStatus);
    let stranger = signer(9)
        .sign_challenge_request(
            Common {
                cluster: CLUSTER,
                sender: node(9),
                audience: node(2),
                origin: ORIGIN.to_owned(),
            },
            [4; 32],
            Commitment::from_request(&inner).unwrap(),
        )
        .unwrap();
    let (status, body) = t.call(
        harmion_protocol::https_peer::CHALLENGE_PATH,
        &stranger.encode().unwrap(),
        Some(HOST),
        false,
        1_000_000,
    );
    assert_eq!((status, body.len()), (403, 0));
    assert_eq!(t.challenges.host().len(), 0);

    let legitimate = t.challenge_request(&inner, [4; 32]);
    let body = legitimate.encode().unwrap();
    for (path, host, origin) in [
        ("/peer/v2/unknown", HOST, false),
        ("/peer/v1", HOST, false),
        (
            harmion_protocol::https_peer::CHALLENGE_PATH,
            "evil.example",
            false,
        ),
        (harmion_protocol::https_peer::CHALLENGE_PATH, HOST, true),
    ] {
        let (status, _) = t.call(path, &body, Some(host), origin, 1_000_000);
        assert_eq!(status, 400, "{path} {host} {origin}");
    }
    assert_eq!(t.challenges.host().len(), 0);
}

#[test]
fn activation_preserves_challenge_state_and_generation() {
    let mut t = Test::new();
    let inner = hpq(3, PeerCommand::MembershipStatus);
    let request = t.challenge_request(&inner, [4; 32]);
    let path = harmion_protocol::https_peer::CHALLENGE_PATH;
    let (status, first) = t.call(
        path,
        &request.encode().unwrap(),
        Some(HOST),
        false,
        1_000_000,
    );
    assert_eq!(status, 200);
    t.restart();
    let (status, second) = t.call(
        path,
        &request.encode().unwrap(),
        Some(HOST),
        false,
        1_000_000,
    );
    assert_eq!(status, 200);
    assert_eq!(
        first, second,
        "the same live challenge is replayed unchanged"
    );
    // A consumed row is never resurrected by activation.
    let challenge = Frame::decode(&second).unwrap();
    let Body::Challenge {
        challenge_request_hash,
        client_nonce,
        generation,
        challenge_nonce,
        ..
    } = &challenge.body
    else {
        panic!("challenge")
    };
    let exchange = t
        .client
        .sign_exchange(
            common(3, 2),
            harmion_protocol::https_peer::ExchangeContext {
                challenge_request_hash: *challenge_request_hash,
                challenge_hash: challenge.hash().unwrap(),
                client_nonce: *client_nonce,
                generation: *generation,
                challenge_nonce: *challenge_nonce,
            },
            harmion_protocol::https_peer::PayloadClass::Evidence,
            inner.clone(),
        )
        .unwrap();
    let (status, _) = t.call(
        harmion_protocol::https_peer::EXCHANGE_PATH,
        &exchange.encode().unwrap(),
        Some(HOST),
        false,
        1_000_000,
    );
    assert_eq!(status, 200);
    t.restart();
    let (status, body) = t.call(
        harmion_protocol::https_peer::EXCHANGE_PATH,
        &exchange.encode().unwrap(),
        Some(HOST),
        false,
        1_000_000,
    );
    assert_eq!(status, 409);
    assert_eq!(
        is_signed_error(&body),
        Some(ErrorCode::ChallengeUnavailable)
    );
}

#[test]
fn clock_reversal_fails_closed_with_503() {
    let mut t = Test::new();
    let inner = hpq(3, PeerCommand::MembershipStatus);
    let path = harmion_protocol::https_peer::CHALLENGE_PATH;
    let first = t.challenge_request(&inner, [4; 32]);
    assert_eq!(
        t.call(path, &first.encode().unwrap(), Some(HOST), false, 2_000_000)
            .0,
        200
    );
    let second = t.challenge_request(&inner, [5; 32]);
    assert_eq!(
        t.call(
            path,
            &second.encode().unwrap(),
            Some(HOST),
            false,
            1_000_000
        )
        .0,
        503
    );
}

#[test]
fn retirement_keeps_pinned_evidence_and_denies_data() {
    let mut t = Test::new();
    membership_state::retire(&mut t.db, 0, &t.admin, &t.server, node(3), [7; 32]).unwrap();
    // Both key and pin survive retirement; evidence is still transportable.
    let pin =
        t.db.transaction(|tx| tx.known_peer_identity(node(3)))
            .unwrap()
            .unwrap();
    assert_eq!(pin.provenance, Provenance::OwnerRegistration);

    let evidence = hpq(3, PeerCommand::MembershipStatus);
    let request = t.challenge_request(&evidence, [4; 32]);
    let (status, _) = t.call(
        harmion_protocol::https_peer::CHALLENGE_PATH,
        &request.encode().unwrap(),
        Some(HOST),
        false,
        1_000_000,
    );
    assert_eq!(status, 200);

    // The preliminary class check refuses data before any challenge allocation.
    let data = hpq(3, PeerCommand::Status);
    let request = t.challenge_request(&data, [5; 32]);
    let (status, _) = t.call(
        harmion_protocol::https_peer::CHALLENGE_PATH,
        &request.encode().unwrap(),
        Some(HOST),
        false,
        1_000_000,
    );
    assert_eq!(status, 403);
    assert_eq!(
        t.challenges
            .challenge_transaction(|tx| tx.count_challenges(None))
            .unwrap(),
        1,
        "only the evidence row exists"
    );
}
