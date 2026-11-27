//! Worker SQL store over a real SQLite host, with the signed-HTTPS entry.
//!
//! The test host plays the Durable Object boundary: one synchronous transaction
//! and a physical sync gate. It is a native test double, not a Cloudflare
//! deployment.
use harmion_domain::{ClusterId, NodeId};
use harmion_node::{
    Error,
    https_peer::Config,
    identity::Provenance,
    membership_state::{self, MembershipSigner},
    storage::{Identity, Store},
};
use harmion_protocol::{
    LogEntry, LogEpoch, Page, PeerCommand, PeerReply, PeerRequest, Record, encode_peer_request,
    hex,
    https_peer::{Body, Commitment, Common, ExchangeContext, Frame, PayloadClass, ResponseResult},
    membership::{Action, Binding, Certificate, Member, Statement},
};
use harmion_worker::{
    HostContext, MemoryKv, Request, SqlHost, SqlTransaction, SqlValue, WorkerChallenges,
    WorkerStore, entry,
};
use rusqlite::{Connection, TransactionBehavior, types::Value as SqliteValue};
use std::path::{Path, PathBuf};

const CLUSTER: ClusterId = ClusterId([1; 32]);
const ORIGIN: &str = "https://server.example";
const HOST: &str = "server.example";

fn signer(n: u8) -> MembershipSigner {
    MembershipSigner::from_bytes(&[n; 32])
}
fn node(n: u8) -> NodeId {
    NodeId([n; 32])
}
fn member(n: u8) -> Member {
    let key = signer(n).public_key();
    Member {
        node: node(n),
        signing_key: key,
        binding: Binding::SignedHttps(key),
        endpoint: format!("https://client{n}.example"),
    }
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
fn common(sender: u8, audience: u8) -> Common {
    Common {
        cluster: CLUSTER,
        sender: node(sender),
        audience: node(audience),
        origin: ORIGIN.to_owned(),
    }
}

/// Native test double for the synchronous DO SQL boundary. Committed data is
/// durable in the database file; a failed callback rolls back.
struct RusqliteHost {
    connection: Connection,
    syncs: usize,
}

impl RusqliteHost {
    fn open(path: impl AsRef<Path>) -> Self {
        let connection = Connection::open(path).unwrap();
        connection
            .execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")
            .unwrap();
        Self {
            connection,
            syncs: 0,
        }
    }
}

struct RusqliteTransaction<'a> {
    transaction: &'a rusqlite::Transaction<'a>,
}

fn to_sqlite(value: &SqlValue) -> SqliteValue {
    match value {
        SqlValue::Null => SqliteValue::Null,
        SqlValue::Int(value) => SqliteValue::Integer(*value),
        SqlValue::Blob(bytes) => SqliteValue::Blob(bytes.clone()),
        SqlValue::Text(text) => SqliteValue::Text(text.clone()),
    }
}

impl SqlTransaction for RusqliteTransaction<'_> {
    fn execute(
        &mut self,
        sql: &str,
        params: &[SqlValue],
    ) -> Result<usize, harmion_worker::HostError> {
        let params: Vec<SqliteValue> = params.iter().map(to_sqlite).collect();
        self.transaction
            .execute(sql, rusqlite::params_from_iter(params))
            .map_err(|error| harmion_worker::HostError(error.to_string()))
    }

    fn query(
        &mut self,
        sql: &str,
        params: &[SqlValue],
    ) -> Result<Vec<Vec<SqlValue>>, harmion_worker::HostError> {
        let params: Vec<SqliteValue> = params.iter().map(to_sqlite).collect();
        let mut statement = self
            .transaction
            .prepare(sql)
            .map_err(|error| harmion_worker::HostError(error.to_string()))?;
        let column_count = statement.column_count();
        let mut rows = statement
            .query(rusqlite::params_from_iter(params))
            .map_err(|error| harmion_worker::HostError(error.to_string()))?;
        let mut result = Vec::new();
        while let Some(row) = rows
            .next()
            .map_err(|error| harmion_worker::HostError(error.to_string()))?
        {
            let mut values = Vec::with_capacity(column_count);
            for index in 0..column_count {
                let value = row
                    .get_ref(index)
                    .map_err(|error| harmion_worker::HostError(error.to_string()))?;
                values.push(match value {
                    rusqlite::types::ValueRef::Null => SqlValue::Null,
                    rusqlite::types::ValueRef::Integer(value) => SqlValue::Int(value),
                    rusqlite::types::ValueRef::Real(value) => SqlValue::Text(value.to_string()),
                    rusqlite::types::ValueRef::Text(text) => {
                        SqlValue::Text(String::from_utf8_lossy(text).into_owned())
                    }
                    rusqlite::types::ValueRef::Blob(bytes) => SqlValue::Blob(bytes.to_vec()),
                });
            }
            result.push(values);
        }
        Ok(result)
    }
}

impl SqlHost for RusqliteHost {
    fn transaction<T>(
        &mut self,
        f: impl FnOnce(&mut dyn SqlTransaction) -> Result<T, harmion_worker::HostError>,
    ) -> Result<T, harmion_worker::HostError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| harmion_worker::HostError(error.to_string()))?;
        let value = {
            let mut sql = RusqliteTransaction {
                transaction: &transaction,
            };
            f(&mut sql)?
        };
        transaction
            .commit()
            .map_err(|error| harmion_worker::HostError(error.to_string()))?;
        Ok(value)
    }
    fn sync(&mut self) -> Result<(), harmion_worker::HostError> {
        self.syncs += 1;
        Ok(())
    }
}

fn identity() -> Identity {
    Identity {
        cluster: CLUSTER,
        node: node(2),
        epoch: LogEpoch([2; 32]),
    }
}

fn record() -> Record {
    let hex = include_str!("../../../contracts/fixtures/create-v1.hex").trim();
    let bytes: Vec<_> = hex
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect();
    Record::from_bytes(&bytes).unwrap()
}

struct Test {
    path: PathBuf,
    store: WorkerStore<RusqliteHost>,
    challenges: WorkerChallenges<MemoryKv>,
    server: MembershipSigner,
    client: MembershipSigner,
}
impl Test {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "harmion-worker-store-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        let db = path.join("do.db");
        let mut store = WorkerStore::open(RusqliteHost::open(&db), identity()).unwrap();
        let server = signer(2);
        let client = signer(3);
        membership_state::initialize(&mut store, &server, member(2), true).unwrap();
        let admit = server
            .sign(Statement {
                cluster: CLUSTER,
                issuer: node(2),
                signing_key: server.public_key(),
                nonce: [7; 32],
                action: Action::Admit(member(3)),
            })
            .unwrap();
        store
            .transaction(|tx| membership_state::apply(tx, &[admit]))
            .unwrap();
        Self {
            path,
            store,
            challenges: WorkerChallenges::new(MemoryKv::new()),
            server,
            client,
        }
    }
    fn reopen(&mut self) {
        self.store =
            WorkerStore::open(RusqliteHost::open(self.path.join("do.db")), identity()).unwrap();
    }
    fn call(&mut self, path: &str, body: &[u8], now: i64) -> (u16, Vec<u8>) {
        let config = config();
        let context = HostContext {
            signer: &self.server,
            config: &config,
            local: identity(),
            now,
        };
        let response = entry::handle(
            &mut self.store,
            &mut self.challenges,
            &context,
            Request {
                method: "POST",
                path,
                host: Some(HOST),
                has_origin: false,
                body,
            },
            &mut entropy(),
        );
        (response.status, response.body)
    }
    fn class(command: &PeerCommand) -> PayloadClass {
        match command {
            PeerCommand::MembershipStatus
            | PeerCommand::MembershipCursor { .. }
            | PeerCommand::MembershipPage { .. }
            | PeerCommand::MembershipAccept(_) => PayloadClass::Evidence,
            _ => PayloadClass::Data,
        }
    }
    /// Runs the challenge and exchange for exact inner HPQ bytes and returns the
    /// final HTTP status plus the signed frame body.
    fn exchange_raw(&mut self, inner: Vec<u8>, class: PayloadClass, now: i64) -> (u16, Vec<u8>) {
        let request = self
            .client
            .sign_challenge_request(
                common(3, 2),
                [4; 32],
                Commitment::from_request(&inner).unwrap(),
            )
            .unwrap();
        let (status, body) = self.call(
            harmion_protocol::https_peer::CHALLENGE_PATH,
            &request.encode().unwrap(),
            now,
        );
        assert_eq!(status, 200);
        let challenge = Frame::decode(&body).unwrap();
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
        let exchange = self
            .client
            .sign_exchange(
                common(3, 2),
                ExchangeContext {
                    challenge_request_hash: *challenge_request_hash,
                    challenge_hash: challenge.hash().unwrap(),
                    client_nonce: *client_nonce,
                    generation: *generation,
                    challenge_nonce: *challenge_nonce,
                },
                class,
                inner,
            )
            .unwrap();
        self.call(
            harmion_protocol::https_peer::EXCHANGE_PATH,
            &exchange.encode().unwrap(),
            now,
        )
    }
    fn exchange(&mut self, command: PeerCommand, now: i64) -> PeerReply {
        let inner = encode_peer_request(&PeerRequest {
            cluster: CLUSTER,
            node: node(3),
            command: command.clone(),
        })
        .unwrap();
        let class = Self::class(&command);
        let (status, body) = self.exchange_raw(inner, class, now);
        let response = Frame::decode(&body).unwrap();
        harmion_node::https_peer::verify_frame(&response, &self.server.public_key()).unwrap();
        let Body::Response { result, .. } = &response.body else {
            panic!("response")
        };
        match result {
            ResponseResult::Peer(bytes) => {
                assert_eq!(status, 200);
                harmion_protocol::decode_peer_response(bytes).unwrap().reply
            }
            ResponseResult::Error(error) => {
                assert_eq!(status, error.code.http_status());
                panic!("signed error {:?}", error.code)
            }
        }
    }
    fn accept_request(&mut self, page: Page) -> (u16, Vec<u8>) {
        let inner = encode_peer_request(&PeerRequest {
            cluster: CLUSTER,
            node: node(3),
            command: PeerCommand::Accept(page),
        })
        .unwrap();
        self.exchange_raw(inner, PayloadClass::Data, 1_000_000)
    }
}
impl Drop for Test {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[test]
fn peer_only_store_serves_membership_and_record_pages_durably() {
    let mut t = Test::new();
    assert!(matches!(
        t.exchange(PeerCommand::MembershipStatus, 1_000_000),
        PeerReply::Status { .. }
    ));
    assert!(matches!(
        t.exchange(PeerCommand::Status, 1_000_000),
        PeerReply::Status { high: 0, .. }
    ));

    // Upload one record through the Peer Accept path. The page epoch is the
    // sender's, not this Worker's.
    let expected = record();
    let sender_epoch = LogEpoch([3; 32]);
    let page = Page {
        epoch: sender_epoch,
        after: 0,
        through: 1,
        high: 1,
        entries: vec![LogEntry {
            seq: 1,
            record: expected.clone(),
        }],
    };
    assert!(matches!(
        t.exchange(PeerCommand::Accept(page), 1_000_000),
        PeerReply::Ack { through: 1, .. }
    ));
    let PeerReply::Page(page) = t.exchange(
        PeerCommand::Page {
            epoch: identity().epoch,
            after: 0,
            high: 1,
        },
        1_000_000,
    ) else {
        panic!("page")
    };
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].record.bytes(), expected.bytes());

    // A restart preserves the log, cursor and pins; the identity is unchanged.
    t.reopen();
    let PeerReply::Status { high, .. } = t.exchange(PeerCommand::Status, 1_000_000) else {
        panic!("status")
    };
    assert_eq!(high, 1);
    t.store
        .transaction(|tx| {
            let pin = tx.known_peer_identity(node(3))?.expect("pin");
            assert_eq!(
                pin.provenance,
                Provenance::AcceptedAdmission(
                    tx.membership_entries()?
                        .into_iter()
                        .find(|entry| matches!(
                            &entry.record.statement().action,
                            Action::Admit(member) if member.node == node(3)
                        ))
                        .unwrap()
                        .record
                        .id()
                )
            );
            assert_eq!(tx.cursor(node(3), sender_epoch)?, 1);
            assert_eq!(tx.cursor(node(3), identity().epoch)?, 0);
            assert_eq!(tx.latest_cursor(node(3))?, Some((sender_epoch, 1)));
            Ok(())
        })
        .unwrap();

    let overview = harmion_worker::admin::handle("overview", &mut t.store, ORIGIN, 1);
    let overview: serde_json::Value = serde_json::from_slice(&overview.body).unwrap();
    assert_eq!(overview["peers"][0]["received_through"], "1");
    let stats = t
        .store
        .transaction(|tx| tx.replica_stats())
        .unwrap()
        .unwrap();
    assert_eq!(stats.contents, 1);
    assert_eq!(stats.accepted_updates, 1);
    assert_eq!(stats.record_bytes, expected.bytes().len() as u64);

    // Reopening with a different Node ID must fail closed.
    assert!(
        WorkerStore::open(
            RusqliteHost::open(t.path.join("do.db")),
            Identity {
                node: node(9),
                ..identity()
            }
        )
        .is_err()
    );
}

#[test]
fn app_and_query_surfaces_fail_closed_instead_of_faking_data() {
    let mut t = Test::new();
    assert!(matches!(
        t.store.transaction(|tx| tx.has_apps()),
        Err(Error::Storage(_))
    ));
    assert!(matches!(
        t.store.transaction(|tx| tx.node_metrics(0).map(|_| ())),
        Err(Error::Storage(_))
    ));
    assert!(matches!(
        t.store.transaction(|tx| tx.search_content_ids(
            &harmion_protocol::query::Condition::And(vec![]),
            &harmion_node::access::Filter::All,
            None,
            1
        )),
        Err(Error::Storage(_))
    ));
}

#[test]
fn an_inconsistent_accept_page_rolls_back_the_log_and_cursor() {
    let mut t = Test::new();
    // through/high disagree with the only entry: the page is invalid before any
    // record or cursor write is committed.
    let page = Page {
        epoch: identity().epoch,
        after: 0,
        through: 2,
        high: 2,
        entries: vec![LogEntry {
            seq: 1,
            record: record(),
        }],
    };
    let (status, _) = t.accept_request(page);
    assert_ne!(status, 200);
    t.store
        .transaction(|tx| {
            assert_eq!(tx.accepted_high()?, 0);
            assert_eq!(tx.cursor(node(3), identity().epoch)?, 0);
            Ok(())
        })
        .unwrap();
}

fn admit(issuer: u8, target: Member) -> Certificate {
    signer(issuer)
        .sign(Statement {
            cluster: CLUSTER,
            issuer: node(issuer),
            signing_key: signer(issuer).public_key(),
            nonce: [target.node.0[0]; 32],
            action: Action::Admit(target),
        })
        .unwrap()
}

fn provisioned_store(path: &Path) -> WorkerStore<RusqliteHost> {
    let identity = Identity {
        cluster: CLUSTER,
        node: node(3),
        epoch: LogEpoch([3; 32]),
    };
    let mut store = WorkerStore::open(RusqliteHost::open(path.join("do.db")), identity).unwrap();
    let root = admit(2, member(2));
    let evidence = [admit(2, member(3))];
    harmion_node::provisioning::provision(
        &mut store,
        &signer(3),
        &harmion_node::provisioning::Manifest {
            own: member(3),
            root,
            evidence: &evidence,
            contacts: &[member(2)],
        },
    )
    .unwrap();
    store
}

#[test]
fn unprovisioned_worker_reports_an_unconfigured_replica() {
    let path = std::env::temp_dir().join(format!(
        "harmion-worker-unconfigured-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    let identity = Identity {
        cluster: CLUSTER,
        node: node(3),
        epoch: LogEpoch([3; 32]),
    };
    let mut store = WorkerStore::open(RusqliteHost::open(path.join("do.db")), identity).unwrap();

    let overview = harmion_worker::admin::handle("overview", &mut store, ORIGIN, 1);
    let overview: serde_json::Value = serde_json::from_slice(&overview.body).unwrap();
    assert_eq!(overview["self_state"], "unconfigured");
    assert_eq!(overview["configured"], false);
    assert_eq!(overview["stats"]["accepted_updates"], "0");
    assert_eq!(overview["members"]["members"], serde_json::json!([]));
    assert_eq!(overview["peers"], serde_json::json!([]));
}

#[test]
fn provisioned_worker_exposes_only_the_read_only_admin_surface() {
    let path = std::env::temp_dir().join(format!(
        "harmion-worker-admin-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    let mut store = provisioned_store(&path);

    let status = harmion_worker::admin::handle("provision-status", &mut store, ORIGIN, 1);
    assert_eq!(status.status, 200);
    let status: serde_json::Value = serde_json::from_slice(&status.body).unwrap();
    assert_eq!(status["configured"], true);
    assert_eq!(status["node_id"], serde_json::json!(hex(&node(3).0)));

    let members = harmion_worker::admin::handle("members", &mut store, ORIGIN, 1);
    let members: serde_json::Value = serde_json::from_slice(&members.body).unwrap();
    let states: Vec<_> = members["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            (
                m["node_id"].as_str().unwrap().to_owned(),
                m["state"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert!(states.contains(&(hex(&node(2).0), "active".to_owned())));
    assert!(states.contains(&(hex(&node(3).0), "active".to_owned())));
    let active = members["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|member| member["node_id"] == hex(&node(3).0))
        .unwrap();
    assert_eq!(
        active["signing_key"],
        serde_json::json!(hex(&signer(3).public_key()))
    );
    assert_eq!(active["binding"]["transport"], "signed_https");
    assert_eq!(
        active["binding"]["origin"],
        serde_json::json!(member(3).endpoint)
    );
    assert_eq!(active["endpoint"], serde_json::json!(member(3).endpoint));

    let peers = harmion_worker::admin::handle("peers", &mut store, ORIGIN, 1);
    let peers: serde_json::Value = serde_json::from_slice(&peers.body).unwrap();
    let peer = &peers["peers"][0];
    assert_eq!(peer["node_id"], serde_json::json!(hex(&node(2).0)));
    assert_eq!(peer["pinned"], true);
    assert_eq!(peer["pin"]["provenance"], "owner_registration");
    assert_eq!(peer["binding"]["transport"], "signed_https");
    assert!(peer["received_through"].is_null());
    assert!(peer["membership_received_through"].is_null());

    let overview = harmion_worker::admin::handle("overview", &mut store, ORIGIN, 1);
    let overview: serde_json::Value = serde_json::from_slice(&overview.body).unwrap();
    assert_eq!(overview["self_state"], "active");
    assert_eq!(overview["stats"]["accepted_updates"], "0");
    assert_eq!(overview["stats"]["contents"], "0");
    assert_eq!(overview["stats"]["record_bytes"], "0");
    // The root and one evidence admission were written by provisioning.
    assert_eq!(overview["stats"]["membership_high"], "3");
    assert!(overview["peers"][0]["received_through"].is_null());
    assert_eq!(
        overview["identity"]["signing_key"],
        serde_json::json!(hex(&signer(3).public_key()))
    );

    let capabilities = harmion_worker::admin::handle("capabilities", &mut store, ORIGIN, 1);
    let capabilities: serde_json::Value = serde_json::from_slice(&capabilities.body).unwrap();
    assert_eq!(capabilities["mode"], "worker");
    assert_eq!(capabilities["read_only"], true);
    assert_eq!(capabilities["sections"]["contents"], false);
    assert!(capabilities["sections"].get("members").is_none());
    assert_eq!(capabilities["operations"]["peer_approve"], false);

    // Unknown and write operations are not routed.
    assert_eq!(
        harmion_worker::admin::handle("contents", &mut store, ORIGIN, 1).status,
        404
    );
    assert_eq!(
        harmion_worker::admin::handle("approve", &mut store, ORIGIN, 1).status,
        404
    );

    // The bearer is required whenever the host configured one; an unset token
    // fails closed instead of allowing open access.
    assert!(harmion_worker::bearer_matches(
        Some("secret-token"),
        "secret-token"
    ));
    assert!(!harmion_worker::bearer_matches(
        Some("secret-tokeN"),
        "secret-token"
    ));
    assert!(!harmion_worker::bearer_matches(None, "secret-token"));
    assert!(!harmion_worker::bearer_matches(Some("anything"), ""));

    // A provisioned worker still answers pinned peers through the Peer entry.
    let mut challenges = WorkerChallenges::new(MemoryKv::new());
    let client = signer(2);
    let inner = encode_peer_request(&PeerRequest {
        cluster: CLUSTER,
        node: node(2),
        command: PeerCommand::MembershipStatus,
    })
    .unwrap();
    let request = client
        .sign_challenge_request(
            common(2, 3),
            [4; 32],
            Commitment::from_request(&inner).unwrap(),
        )
        .unwrap();
    let config = config();
    let context = HostContext {
        signer: &signer(3),
        config: &config,
        local: Identity {
            cluster: CLUSTER,
            node: node(3),
            epoch: LogEpoch([3; 32]),
        },
        now: 1_000_000,
    };
    let response = entry::handle(
        &mut store,
        &mut challenges,
        &context,
        Request {
            method: "POST",
            path: harmion_protocol::https_peer::CHALLENGE_PATH,
            host: Some(HOST),
            has_origin: false,
            body: &request.encode().unwrap(),
        },
        &mut entropy(),
    );
    assert_eq!(response.status, 200);
    let challenge = Frame::decode(&response.body).unwrap();
    harmion_node::https_peer::verify_frame(&challenge, &signer(3).public_key()).unwrap();
    let _ = std::fs::remove_dir_all(&path);
}
