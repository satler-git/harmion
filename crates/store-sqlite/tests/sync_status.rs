use harmion_domain::{
    Action, Change, ClusterId, ContentId, Field, NodeId, Update, UpdateId, Value,
};
use harmion_node::{
    Error,
    access::{App, Credential, Grants},
    membership::{Peer, PeerKey},
    storage::{Identity, Store, accept_records},
    sync_status::{self, Observation},
};
use harmion_protocol::{AppId, LogEpoch, Record};
use harmion_store_sqlite::SqliteStore;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Test {
    path: PathBuf,
    store: SqliteStore,
    owner: Credential,
}
fn identity(n: u8) -> Identity {
    Identity {
        cluster: ClusterId([1; 32]),
        node: NodeId([n; 32]),
        epoch: LogEpoch([n; 32]),
    }
}
fn record(n: u8) -> Record {
    Record::from_update(Update {
        format_version: 1,
        cluster_id: identity(1).cluster,
        content_id: ContentId([n; 32]),
        root_id: UpdateId([n; 32]),
        update_id: UpdateId([n; 32]),
        author_node_id: identity(1).node,
        edited_at: 1,
        action: Action::Create,
        basis: BTreeSet::new(),
        changes: BTreeMap::from([(
            Field::Body,
            Change {
                value: Value::Body("test".into()),
                replaces: BTreeSet::new(),
            },
        )]),
    })
    .unwrap()
}
impl Test {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "harmion-sync-status-{}-{}.db",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut store = SqliteStore::open(&path, identity(1)).unwrap();
        let owner = Credential::from_bytes([7; 32]);
        store
            .transaction(|tx| {
                tx.insert_app(&App {
                    id: AppId([7; 32]),
                    verifier: owner.verifier(),
                    expires_at: i64::MAX,
                    revoked: false,
                    grants: Grants::administrator(),
                })?;
                tx.put_peer(&Peer {
                    node: identity(2).node,
                    key: PeerKey::Iroh([99; 32]),
                    endpoint: "secret.example:1234".into(),
                    retired: false,
                })
            })
            .unwrap();
        Self { path, store, owner }
    }
    fn get(&mut self, update: Option<UpdateId>) -> harmion_protocol::sync_status::SyncStatus {
        sync_status::get(&mut self.store, &self.owner, 10, update).unwrap()
    }
    fn save(&mut self, n: u8) {
        self.store
            .transaction(|tx| accept_records(tx, &[record(n)]).map(|_| ()))
            .unwrap();
    }
    fn observation(&self, ack: u64) -> Observation {
        Observation {
            local_epoch: identity(1).epoch,
            acknowledged_through: ack,
            remote_epoch: identity(2).epoch,
            remote_high: 0,
            received_through: 0,
            succeeded_at: 9_007_199_254_740_993,
        }
    }
    fn success(&mut self, ack: u64) {
        let observation = self.observation(ack);
        self.store
            .transaction(|tx| sync_status::record_success(tx, identity(2).node, &observation))
            .unwrap();
    }
}
impl Drop for Test {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[test]
fn persistence_new_records_unknown_update_and_redacted_json() {
    let mut t = Test::new();
    t.save(1);
    let before = t.get(Some(UpdateId([1; 32])));
    assert_eq!(before.peers[0].unconfirmed_local, "1");
    assert_eq!(before.peers[0].update_confirmed, None);
    t.success(1);
    assert_eq!(
        t.get(Some(UpdateId([1; 32]))).peers[0].update_confirmed,
        Some(true)
    );
    t.store
        .transaction(|tx| sync_status::record_failure(tx, identity(2).node, 2, "transport"))
        .unwrap();
    t.store = SqliteStore::open(&t.path, identity(1)).unwrap();
    let status = t.get(Some(UpdateId([1; 32])));
    assert_eq!(
        status.peers[0].last_success_at.as_deref(),
        Some("9007199254740993")
    );
    assert_eq!(
        status.peers[0].last_failure.as_ref().unwrap().attempted_at,
        "2"
    );
    assert_eq!(status.peers[0].update_confirmed, Some(true));
    let json = serde_json::to_string(&status).unwrap();
    assert!(!json.contains("secret.example"));
    assert!(!json.contains("endpoint"));
    assert!(!json.contains("credential"));
    assert!(!json.contains("key"));
    t.save(3);
    assert_eq!(
        t.get(Some(UpdateId([3; 32]))).peers[0].update_confirmed,
        Some(false)
    );
    assert_eq!(t.get(None).peers[0].unconfirmed_local, "1");
    assert!(matches!(
        sync_status::get(&mut t.store, &t.owner, 10, Some(UpdateId([90; 32]))),
        Err(Error::NotFound)
    ));
    // Completion order, not wall clock ordering, determines whether failure clears.
    t.success(2);
    assert!(t.get(None).peers[0].last_failure.is_none());
}

#[test]
fn owner_only_rechecks_revocation_expiration_and_grants() {
    let mut t = Test::new();
    for grants in [
        Grants::default(),
        Grants {
            manage_nodes: true,
            ..Grants::default()
        },
        Grants {
            read: Some(harmion_node::access::Filter::All),
            ..Grants::default()
        },
    ] {
        t.store
            .transaction(|tx| {
                let mut app = tx.app_by_id(AppId([7; 32]))?.unwrap();
                app.grants = grants;
                tx.update_app(&app)
            })
            .unwrap();
        assert!(matches!(
            sync_status::get(&mut t.store, &t.owner, 10, None),
            Err(Error::Forbidden)
        ));
    }
    t.store
        .transaction(|tx| {
            let mut app = tx.app_by_id(AppId([7; 32]))?.unwrap();
            app.grants = Grants::administrator();
            app.revoked = true;
            tx.update_app(&app)
        })
        .unwrap();
    assert!(matches!(
        sync_status::get(&mut t.store, &t.owner, 10, None),
        Err(Error::Unauthenticated)
    ));
    t.store
        .transaction(|tx| {
            let mut app = tx.app_by_id(AppId([7; 32]))?.unwrap();
            app.revoked = false;
            app.expires_at = 10;
            tx.update_app(&app)
        })
        .unwrap();
    assert!(matches!(
        sync_status::get(&mut t.store, &t.owner, 10, None),
        Err(Error::Unauthenticated)
    ));
}

#[test]
fn transaction_rollback_preserves_evidence_and_retired_failures_are_allowed() {
    let mut t = Test::new();
    t.save(1);
    let observation = t.observation(1);
    let result: Result<(), Error> = t.store.transaction(|tx| {
        sync_status::record_success(tx, identity(2).node, &observation)?;
        Err(Error::InvalidRequest)
    });
    assert!(result.is_err());
    assert!(t.get(None).peers[0].last_success_at.is_none());
    t.success(1);
    let result: Result<(), Error> = t.store.transaction(|tx| {
        sync_status::record_failure(tx, identity(2).node, 10, "transport")?;
        Err(Error::InvalidRequest)
    });
    assert!(result.is_err());
    assert!(t.get(None).peers[0].last_failure.is_none());
    t.store
        .transaction(|tx| {
            let mut peer = tx.peer(identity(2).node)?.unwrap();
            peer.retired = true;
            tx.put_peer(&peer)?;
            sync_status::record_failure(tx, peer.node, 11, "retired")
        })
        .unwrap();
    assert!(t.get(None).peers[0].retired);
    assert_eq!(
        t.get(Some(UpdateId([1; 32]))).peers[0].update_confirmed,
        Some(true)
    );
    assert!(matches!(
        t.store
            .transaction(|tx| sync_status::record_success(tx, identity(2).node, &observation)),
        Err(Error::PeerRetired)
    ));
}

#[test]
fn counts_records_not_sequence_distance_and_ignores_stale_epoch_proof() {
    let mut t = Test::new();
    t.save(1);
    t.success(1);
    let sql = rusqlite::Connection::open(&t.path).unwrap();
    sql.execute(
        "UPDATE sqlite_sequence SET seq=9 WHERE name='accept_log'",
        [],
    )
    .unwrap();
    t.save(3);
    assert_eq!(t.get(None).local_high, "10");
    assert_eq!(t.get(None).peers[0].unconfirmed_local, "1");
    let mut stale = t.observation(10);
    stale.local_epoch = LogEpoch([88; 32]);
    t.store
        .transaction(|tx| tx.put_sync_observation(identity(2).node, &stale))
        .unwrap();
    let status = t.get(Some(UpdateId([1; 32])));
    assert_eq!(status.peers[0].unconfirmed_local, "2");
    assert_eq!(status.peers[0].update_confirmed, None);
}

#[test]
fn migrates_v9_without_fabricating_evidence_and_restore_copies_only_content() {
    let mut t = Test::new();
    t.save(1);
    let sql = rusqlite::Connection::open(&t.path).unwrap();
    sql.execute_batch(
        "DROP TABLE sync_failures; DROP TABLE sync_observations; DROP TABLE identity_pins; DROP TABLE https_state; DROP TABLE https_challenges; DROP TABLE https_rate; PRAGMA user_version=9;",
    )
    .unwrap();
    t.store = SqliteStore::open(&t.path, identity(1)).unwrap();
    let status = t.get(Some(UpdateId([1; 32])));
    assert_eq!(status.peers[0].last_success_at, None);
    assert_eq!(status.peers[0].update_confirmed, None);
    assert_eq!(status.peers[0].unconfirmed_local, "1");
    assert_eq!(
        sql.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        13
    );
    t.success(1);
    let destination_path = t.path.with_extension("restore.db");
    let mut destination = SqliteStore::open(&destination_path, identity(3)).unwrap();
    destination.restore_backup(&t.path, identity(1)).unwrap();
    destination
        .transaction(|tx| {
            assert!(tx.accepted_record(UpdateId([1; 32]))?.is_some());
            assert!(tx.peers()?.is_empty());
            assert!(tx.sync_observation(identity(2).node)?.is_none());
            assert!(tx.sync_failure(identity(2).node)?.is_none());
            Ok(())
        })
        .unwrap();
    drop(destination);
    std::fs::remove_file(destination_path).unwrap();
}

#[test]
fn invalid_completion_and_failure_codes_never_create_evidence() {
    let mut t = Test::new();
    t.save(1);
    let mut observation = t.observation(2);
    assert!(
        t.store
            .transaction(|tx| sync_status::record_success(tx, identity(2).node, &observation))
            .is_err()
    );
    observation.acknowledged_through = 1;
    observation.remote_high = 1;
    assert!(
        t.store
            .transaction(|tx| sync_status::record_success(tx, identity(2).node, &observation))
            .is_err()
    );
    observation.received_through = 1;
    assert!(
        t.store
            .transaction(|tx| sync_status::record_success(tx, identity(2).node, &observation))
            .is_err()
    );
    t.store
        .transaction(|tx| tx.set_cursor(identity(2).node, identity(2).epoch, 1))
        .unwrap();
    t.store
        .transaction(|tx| sync_status::record_success(tx, identity(2).node, &observation))
        .unwrap();
    for code in ["", "https://secret.example", "secret credential", "UPPER"] {
        assert!(
            t.store
                .transaction(|tx| sync_status::record_failure(tx, identity(2).node, 10, code))
                .is_err()
        );
    }
    assert!(t.get(None).peers[0].last_failure.is_none());
}
