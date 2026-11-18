use harmion_domain::*;
use harmion_node::{
    Error,
    access::{Credential, Filter, Grants},
    application::{self as app, Host},
    storage::{Identity, Store, read_graph},
};
use harmion_protocol::{AppId, Command, EditToken, LogEpoch, RequestId, SaveRequest};
use harmion_store_sqlite::SqliteStore;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

struct TestHost {
    next: u64,
    now: i64,
}
impl Host for TestHost {
    fn random_bytes(&mut self) -> Result<[u8; 32], Error> {
        self.next += 1;
        let mut bytes = [0; 32];
        bytes[24..].copy_from_slice(&self.next.to_be_bytes());
        Ok(bytes)
    }
    fn now_micros(&self) -> i64 {
        self.now
    }
}
static NEXT: AtomicU64 = AtomicU64::new(0);

#[test]
fn v3_peer_migration_preserves_tls_binding_and_separates_iroh_keys() {
    use harmion_node::membership::{self, Peer, PeerKey};
    let mut t = Test::new();
    // Simulate a deployed v3 database, retaining all other state.
    let db = rusqlite::Connection::open(t.path.join("node.db")).unwrap();
    db.execute_batch("DROP TABLE membership_identity; DROP TABLE membership_recovery_roots; DROP TABLE membership_root; DROP TABLE membership_log; DROP TABLE membership_cursors; DROP TABLE peers; CREATE TABLE peers(node BLOB PRIMARY KEY, certificate BLOB UNIQUE NOT NULL, endpoint TEXT NOT NULL, retired INTEGER NOT NULL); PRAGMA user_version=3;").unwrap();
    db.execute(
        "INSERT INTO peers VALUES(?1,?2,?3,0)",
        rusqlite::params![[8u8; 32], [9u8; 32], "127.0.0.1:1"],
    )
    .unwrap();
    drop(db);
    t.restart();
    let tls = t
        .store
        .transaction(|tx| tx.peer(NodeId([8; 32])))
        .unwrap()
        .unwrap();
    assert_eq!(tls.key, PeerKey::TlsCertificate(vec![9; 32]));
    let peer = Peer {
        node: NodeId([7; 32]),
        key: PeerKey::Iroh([9; 32]),
        endpoint: String::new(),
        retired: false,
    };
    membership::approve(
        &mut t.store,
        100,
        &t.admin,
        identity().cluster,
        peer.clone(),
    )
    .unwrap();
    t.restart();
    assert_eq!(
        t.store.transaction(|tx| tx.peer(peer.node)).unwrap(),
        Some(peer.clone())
    );
    let replacement = Peer {
        key: PeerKey::Iroh([10; 32]),
        ..peer.clone()
    };
    assert!(matches!(
        membership::approve(&mut t.store, 100, &t.admin, identity().cluster, replacement),
        Err(Error::IdentityMismatch)
    ));
    membership::retire(&mut t.store, 100, &t.admin, peer.node).unwrap();
    assert!(matches!(
        membership::approve(&mut t.store, 100, &t.admin, identity().cluster, peer),
        Err(Error::PeerRetired)
    ));
}
struct Test {
    path: PathBuf,
    store: SqliteStore,
    host: TestHost,
    admin: Credential,
}
impl Test {
    fn new() -> Self {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("harmion-app-test-{}-{n}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        let mut store = SqliteStore::open(path.join("node.db"), identity()).unwrap();
        let mut host = TestHost {
            next: n * 100_000,
            now: 100,
        };
        let (_, admin) = app::bootstrap_admin(&mut store, &mut host, 10_000).unwrap();
        Self {
            path,
            store,
            host,
            admin,
        }
    }
    fn issue(&mut self, grants: Grants) -> (AppId, Credential) {
        app::issue_app(&mut self.store, &mut self.host, &self.admin, grants, 1000).unwrap()
    }
    fn create(&mut self) -> app::SaveResult {
        app::save(
            &mut self.store,
            &mut self.host,
            &self.admin,
            &create_request(1),
        )
        .unwrap()
    }
    fn get(&mut self, credential: &Credential) -> app::ReadView {
        app::get(&mut self.store, &mut self.host, credential, content()).unwrap()
    }
    fn save(
        &mut self,
        credential: &Credential,
        n: u8,
        command: Command,
    ) -> Result<app::SaveResult, Error> {
        app::save(
            &mut self.store,
            &mut self.host,
            credential,
            &request(n, command),
        )
    }
    fn count(&mut self) -> usize {
        self.store
            .transaction(|tx| Ok(tx.entries()?.len()))
            .unwrap()
    }
    fn snapshot(&mut self) -> ContentSnapshot {
        self.store
            .transaction(|tx| Ok(read_graph(tx)?.snapshot(content()).unwrap()))
            .unwrap()
    }
    fn restart(&mut self) {
        self.store = SqliteStore::open(self.path.join("node.db"), identity()).unwrap();
    }
}
impl Drop for Test {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
fn identity() -> Identity {
    Identity {
        cluster: ClusterId([1; 32]),
        node: NodeId([1; 32]),
        epoch: LogEpoch([1; 32]),
    }
}
fn content() -> ContentId {
    ContentId([1; 32])
}
fn attr() -> Field {
    Field::Attribute {
        namespace: "app".into(),
        key: "title".into(),
    }
}
fn body(s: &str) -> BTreeMap<Field, Value> {
    BTreeMap::from([(Field::Body, Value::Body(s.into()))])
}
fn request(n: u8, command: Command) -> SaveRequest {
    SaveRequest {
        request_id: RequestId([n; 32]),
        content_id: content(),
        command,
    }
}
fn create_request(n: u8) -> SaveRequest {
    let mut fields = body("base");
    fields.insert(Field::Kind("wiki".into()), Value::Kind(true));
    fields.insert(
        attr(),
        Value::Attribute(AttributeValue::String("title".into())),
    );
    request(n, Command::Create { fields })
}
fn read_write() -> Grants {
    Grants {
        read: Some(Filter::All),
        write: Some(Filter::All),
        ..Grants::default()
    }
}
fn wiki() -> Filter {
    Filter::Kinds(BTreeSet::from(["wiki".into()]))
}

#[test]
fn bootstrap_is_one_time_and_only_verifiers_are_stored() {
    let mut t = Test::new();
    assert!(matches!(
        app::bootstrap_admin(&mut t.store, &mut t.host, 10_000),
        Err(Error::AlreadyBootstrapped)
    ));
    let (id, credential) = t.issue(read_write());
    let stored = t.store.transaction(|tx| tx.app_by_id(id)).unwrap().unwrap();
    assert_ne!(&stored.verifier, credential.expose_bytes());
    assert_eq!(stored.verifier, credential.verifier());
    assert_eq!(format!("{credential:?}"), "Credential([redacted])");
    assert!(matches!(
        app::issue_app(
            &mut t.store,
            &mut t.host,
            &credential,
            Grants::administrator(),
            1000
        ),
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        app::get(
            &mut t.store,
            &mut t.host,
            &Credential::from_bytes([99; 32]),
            content()
        ),
        Err(Error::Unauthenticated)
    ));
}

#[test]
fn retry_after_restart_returns_original_result_and_reuse_is_rejected() {
    let mut t = Test::new();
    let admin = t.admin.clone();
    let first = t.create();
    let token = first.view.as_ref().unwrap().edit_token;
    t.save(
        &admin,
        2,
        Command::Edit {
            token,
            fields: body("later"),
        },
    )
    .unwrap();
    t.restart();
    assert_eq!(
        app::save(&mut t.store, &mut t.host, &admin, &create_request(1)).unwrap(),
        first
    );
    assert_eq!(t.count(), 2);
    let changed = request(
        1,
        Command::Create {
            fields: body("changed"),
        },
    );
    assert!(matches!(
        app::save(&mut t.store, &mut t.host, &admin, &changed),
        Err(Error::RequestIdReuse)
    ));
    assert_eq!(t.count(), 2);
}

#[test]
fn reusable_stale_token_keeps_independent_branches_and_invalid_tokens_never_rebase() {
    let mut t = Test::new();
    t.create();
    let admin = t.admin.clone();
    let (_, user) = t.issue(read_write());
    let token = t.get(&user).edit_token;
    t.save(
        &admin,
        2,
        Command::Edit {
            token,
            fields: body("admin branch"),
        },
    )
    .unwrap();
    t.restart();
    t.save(
        &user,
        2,
        Command::Edit {
            token,
            fields: body("old user edit"),
        },
    )
    .unwrap();
    t.save(
        &user,
        3,
        Command::Edit {
            token,
            fields: body("another old edit"),
        },
    )
    .unwrap();
    assert_eq!(t.snapshot().fields[&Field::Body].candidates.len(), 3);
    assert!(matches!(
        t.save(
            &user,
            4,
            Command::Edit {
                token: EditToken([99; 32]),
                fields: body("bad")
            }
        ),
        Err(Error::InvalidToken)
    ));
    let mut wrong_content = request(
        5,
        Command::Edit {
            token,
            fields: body("bad"),
        },
    );
    wrong_content.content_id = ContentId([2; 32]);
    assert!(matches!(
        app::save(&mut t.store, &mut t.host, &user, &wrong_content),
        Err(Error::InvalidToken)
    ));
    assert_eq!(t.count(), 4);
}

#[test]
fn write_only_uses_delegated_token_without_read_or_management_access() {
    let mut t = Test::new();
    let first = t.create();
    let (_, writer) = t.issue(Grants {
        write: Some(Filter::All),
        ..Grants::default()
    });
    let token = first.view.unwrap().edit_token;
    let saved = t
        .save(
            &writer,
            1,
            Command::Edit {
                token,
                fields: body("write only"),
            },
        )
        .unwrap();
    assert!(saved.view.is_none());
    assert!(matches!(
        app::get(&mut t.store, &mut t.host, &writer, content()),
        Err(Error::NotFound)
    ));
    assert!(matches!(
        app::get_conflicts(&mut t.store, &mut t.host, &writer, content()),
        Err(Error::NotFound)
    ));
    let (_, none) = t.issue(Grants::default());
    assert!(matches!(
        t.save(
            &none,
            1,
            Command::Edit {
                token,
                fields: body("unauthorized")
            }
        ),
        Err(Error::Forbidden)
    ));
    let (_, creator) = t.issue(Grants {
        write: Some(Filter::All),
        ..Grants::default()
    });
    let mut create = create_request(1);
    create.content_id = ContentId([2; 32]);
    assert!(
        app::save(&mut t.store, &mut t.host, &creator, &create)
            .unwrap()
            .view
            .is_none()
    );
}

#[test]
fn retry_redacts_revoked_read_and_current_out_of_scope_content() {
    let mut t = Test::new();
    t.create();
    let admin = t.admin.clone();
    let (user_id, user) = t.issue(Grants {
        read: Some(wiki()),
        write: Some(wiki()),
        ..Grants::default()
    });
    let token = t.get(&user).edit_token;
    let req = request(
        1,
        Command::Edit {
            token,
            fields: body("user saved"),
        },
    );
    let saved = app::save(&mut t.store, &mut t.host, &user, &req).unwrap();
    assert!(saved.view.is_some());
    app::change_grants(
        &mut t.store,
        t.host.now,
        &admin,
        user_id,
        Grants {
            write: Some(wiki()),
            ..Grants::default()
        },
        false,
    )
    .unwrap();
    assert!(
        app::save(&mut t.store, &mut t.host, &user, &req)
            .unwrap()
            .view
            .is_none()
    );
    app::change_grants(
        &mut t.store,
        t.host.now,
        &admin,
        user_id,
        Grants {
            read: Some(wiki()),
            write: Some(wiki()),
            ..Grants::default()
        },
        false,
    )
    .unwrap();
    let current = t.get(&admin).edit_token;
    t.host.now += 1;
    t.save(
        &admin,
        2,
        Command::Edit {
            token: current,
            fields: BTreeMap::from([(Field::Kind("wiki".into()), Value::Kind(false))]),
        },
    )
    .unwrap();
    let retry = app::save(&mut t.store, &mut t.host, &user, &req).unwrap();
    assert_eq!(retry.update_id, saved.update_id);
    assert!(retry.view.is_none());
    app::change_grants(
        &mut t.store,
        t.host.now,
        &admin,
        user_id,
        read_write(),
        true,
    )
    .unwrap();
    assert!(matches!(
        app::save(&mut t.store, &mut t.host, &user, &req),
        Err(Error::Unauthenticated)
    ));
    let (_, expires) = t.issue(read_write());
    t.host.now = 1000;
    assert!(matches!(
        app::get(&mut t.store, &mut t.host, &expires, content()),
        Err(Error::Unauthenticated)
    ));
}

#[test]
fn write_filters_check_before_and_after_with_current_kinds() {
    let mut t = Test::new();
    t.create();
    let admin = t.admin.clone();
    let (_, user) = t.issue(Grants {
        read: Some(wiki()),
        write: Some(wiki()),
        ..Grants::default()
    });
    let token = t.get(&user).edit_token;
    t.host.now += 1;
    let remove = BTreeMap::from([(Field::Kind("wiki".into()), Value::Kind(false))]);
    assert!(matches!(
        t.save(
            &user,
            1,
            Command::Edit {
                token,
                fields: remove.clone()
            }
        ),
        Err(Error::Forbidden)
    ));
    assert_eq!(t.count(), 1);
    t.save(
        &admin,
        2,
        Command::Edit {
            token,
            fields: remove,
        },
    )
    .unwrap();
    assert!(matches!(
        t.save(
            &user,
            2,
            Command::Edit {
                token,
                fields: body("stale but now outside filter")
            }
        ),
        Err(Error::Forbidden)
    ));
    assert_eq!(t.count(), 2);
    let req = request(
        3,
        Command::Create {
            fields: body("no allowed kind"),
        },
    );
    let req = SaveRequest {
        content_id: ContentId([3; 32]),
        ..req
    };
    assert!(matches!(
        app::save(&mut t.store, &mut t.host, &user, &req),
        Err(Error::Forbidden)
    ));
}

#[test]
fn kind_conflicts_are_hidden_and_cannot_be_changed_by_general_apps() {
    let mut t = Test::new();
    let token = t.create().view.unwrap().edit_token;
    let admin = t.admin.clone();
    t.save(
        &admin,
        2,
        Command::Edit {
            token,
            fields: BTreeMap::from([(Field::Kind("wiki".into()), Value::Kind(false))]),
        },
    )
    .unwrap();
    t.save(
        &admin,
        3,
        Command::Edit {
            token,
            fields: BTreeMap::from([(Field::Kind("wiki".into()), Value::Kind(true))]),
        },
    )
    .unwrap();
    let (_, user) = t.issue(Grants {
        read: Some(wiki()),
        write: Some(wiki()),
        ..Grants::default()
    });
    let view = t.get(&user);
    assert!(view.conflicts.is_empty());
    let conflicts = app::get_conflicts(&mut t.store, &mut t.host, &user, content()).unwrap();
    assert!(
        !conflicts
            .candidates
            .contains_key(&Field::Kind("wiki".into()))
    );
    assert!(matches!(
        t.save(
            &user,
            1,
            Command::Edit {
                token: view.edit_token,
                fields: BTreeMap::from([(Field::Kind("wiki".into()), Value::Kind(true))])
            }
        ),
        Err(Error::Forbidden)
    ));
    t.save(
        &user,
        2,
        Command::Edit {
            token: view.edit_token,
            fields: body("body is allowed"),
        },
    )
    .unwrap();
    assert!(
        t.get(&admin)
            .conflicts
            .contains(&Field::Kind("wiki".into()))
    );
}

#[test]
fn resolution_only_replaces_explicit_candidates_and_leaves_late_edits() {
    let mut t = Test::new();
    let token = t.create().view.unwrap().edit_token;
    let admin = t.admin.clone();
    let a = t
        .save(
            &admin,
            2,
            Command::Edit {
                token,
                fields: body("A"),
            },
        )
        .unwrap();
    let b = t
        .save(
            &admin,
            3,
            Command::Edit {
                token,
                fields: body("B"),
            },
        )
        .unwrap();
    let conflicts = app::get_conflicts(&mut t.store, &mut t.host, &admin, content()).unwrap();
    let before_late = t.get(&admin).edit_token;
    let late = t
        .save(
            &admin,
            4,
            Command::Edit {
                token: before_late,
                fields: body("late"),
            },
        )
        .unwrap();
    let changes = BTreeMap::from([(
        Field::Body,
        Change {
            value: Value::Body("resolved".into()),
            replaces: BTreeSet::from([a.update_id, b.update_id]),
        },
    )]);
    let resolved = t
        .save(
            &admin,
            5,
            Command::Resolve {
                token: conflicts.view.edit_token,
                changes,
            },
        )
        .unwrap();
    let snapshot = t.snapshot();
    let heads: BTreeSet<_> = snapshot.fields[&Field::Body]
        .candidates
        .iter()
        .flat_map(|c| c.versions.iter().copied())
        .collect();
    assert_eq!(heads, BTreeSet::from([late.update_id, resolved.update_id]));
    let unknown = BTreeMap::from([(
        Field::Body,
        Change {
            value: Value::Body("bad".into()),
            replaces: BTreeSet::from([late.update_id]),
        },
    )]);
    assert!(matches!(
        t.save(
            &admin,
            6,
            Command::Resolve {
                token: conflicts.view.edit_token,
                changes: unknown
            }
        ),
        Err(Error::InvalidRequest)
    ));
    assert!(matches!(
        t.save(
            &admin,
            7,
            Command::Resolve {
                token,
                changes: BTreeMap::new()
            }
        ),
        Err(Error::InvalidToken)
    ));
}

#[test]
fn deletion_stale_edit_keep_and_restore_preserve_management_boundaries() {
    let mut t = Test::new();
    let token = t.create().view.unwrap().edit_token;
    let admin = t.admin.clone();
    let (_, user) = t.issue(read_write());
    t.save(&user, 1, Command::Delete { token }).unwrap();
    assert!(matches!(
        app::get(&mut t.store, &mut t.host, &admin, content()),
        Err(Error::NotFound)
    ));
    let inspect = app::inspect(&mut t.store, &mut t.host, &admin, content()).unwrap();
    assert!(inspect.deleted);
    assert!(matches!(
        t.save(
            &user,
            2,
            Command::Restore {
                token: inspect.view.edit_token,
                fields: body("not admin")
            }
        ),
        Err(Error::Forbidden)
    ));
    t.save(
        &admin,
        2,
        Command::Restore {
            token: inspect.view.edit_token,
            fields: body("restored"),
        },
    )
    .unwrap();
    assert!(t.snapshot().content.attributes.is_empty());
    assert!(t.snapshot().content.kinds.is_empty());
    let current = t.get(&admin).edit_token;
    t.save(&admin, 3, Command::Delete { token: current })
        .unwrap();
    t.save(
        &user,
        3,
        Command::Edit {
            token: current,
            fields: body("late edit"),
        },
    )
    .unwrap();
    assert_eq!(t.snapshot().life, Life::Conflict);
    let conflict = app::inspect(&mut t.store, &mut t.host, &admin, content()).unwrap();
    assert!(matches!(
        t.save(
            &user,
            4,
            Command::Delete {
                token: conflict.view.edit_token
            }
        ),
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        t.save(
            &user,
            5,
            Command::Keep {
                token: conflict.view.edit_token
            }
        ),
        Err(Error::Forbidden)
    ));
    t.save(
        &admin,
        4,
        Command::Keep {
            token: conflict.view.edit_token,
        },
    )
    .unwrap();
    assert_eq!(t.snapshot().life, Life::Live);
}

#[test]
fn request_result_failure_rolls_back_update_token_and_result() {
    let mut t = Test::new();
    let admin = t.admin.clone();
    let db = rusqlite::Connection::open(t.path.join("node.db")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_result BEFORE INSERT ON request_results BEGIN SELECT RAISE(ABORT, 'injected result failure'); END;").unwrap();
    assert!(app::save(&mut t.store, &mut t.host, &admin, &create_request(1)).is_err());
    assert_eq!(t.count(), 0);
    let tokens: i64 = db
        .query_row("SELECT count(*) FROM edit_tokens", [], |r| r.get(0))
        .unwrap();
    assert_eq!(tokens, 0);
    db.execute_batch("DROP TRIGGER fail_result").unwrap();
    t.create();
    assert_eq!(t.count(), 1);
}

#[test]
fn synchronized_content_does_not_transfer_credentials_tokens_or_request_results() {
    use harmion_node::replication;
    let mut t = Test::new();
    let original = t.create();
    let token = original.view.unwrap().edit_token;
    let remote_id = Identity {
        node: NodeId([2; 32]),
        epoch: LogEpoch([2; 32]),
        ..identity()
    };
    let mut remote = SqliteStore::open(t.path.join("remote.db"), remote_id).unwrap();
    let (remote_app, remote_admin) =
        app::bootstrap_admin(&mut remote, &mut t.host, 10_000).unwrap();
    let page = t
        .store
        .transaction(|tx| {
            let high = replication::high(tx)?;
            replication::page(tx, identity().epoch, 0, high)
        })
        .unwrap();
    remote
        .transaction(|tx| replication::receive(tx, identity().node, &page))
        .unwrap();
    assert!(matches!(
        app::get(&mut remote, &mut t.host, &t.admin, content()),
        Err(Error::Unauthenticated)
    ));
    assert!(remote.transaction(|tx| tx.token(token)).unwrap().is_none());
    assert!(
        remote
            .transaction(|tx| tx.request(remote_app, RequestId([1; 32])))
            .unwrap()
            .is_none()
    );
    let req = request(
        1,
        Command::Edit {
            token,
            fields: body("cannot migrate token"),
        },
    );
    assert!(matches!(
        app::save(&mut remote, &mut t.host, &remote_admin, &req),
        Err(Error::InvalidToken)
    ));
    let remote_token = app::get(&mut remote, &mut t.host, &remote_admin, content())
        .unwrap()
        .edit_token;
    let req = request(
        1,
        Command::Edit {
            token: remote_token,
            fields: body("remote branch"),
        },
    );
    app::save(&mut remote, &mut t.host, &remote_admin, &req).unwrap();
    let page = remote
        .transaction(|tx| {
            let high = replication::high(tx)?;
            replication::page(tx, remote_id.epoch, 0, high)
        })
        .unwrap();
    t.store
        .transaction(|tx| replication::receive(tx, remote_id.node, &page))
        .unwrap();
    let admin = t.admin.clone();
    t.save(
        &admin,
        2,
        Command::Edit {
            token,
            fields: body("old local edit"),
        },
    )
    .unwrap();
    assert_eq!(t.snapshot().fields[&Field::Body].candidates.len(), 2);
}

#[test]
fn migration_preserves_v1_records_and_cursor_generation() {
    let mut t = Test::new();
    t.create();
    let db = rusqlite::Connection::open(t.path.join("node.db")).unwrap();
    // Reproduce the previous schema with its original log and identity intact.
    db.execute_batch(
        "DROP TABLE request_results; DROP TABLE token_basis; DROP TABLE edit_tokens;
        DROP TABLE app_kinds; DROP TABLE apps; DROP TABLE peers; DROP TABLE membership_identity; DROP TABLE membership_recovery_roots; DROP TABLE membership_root; DROP TABLE membership_log; DROP TABLE membership_cursors; PRAGMA user_version=1;",
    )
    .unwrap();
    t.restart();
    assert_eq!(t.count(), 1);
    assert_eq!(t.store.identity(), identity());
    assert_eq!(t.snapshot().content.body, "base");
    let (_, admin) = app::bootstrap_admin(&mut t.store, &mut t.host, 10_000).unwrap();
    app::get(&mut t.store, &mut t.host, &admin, content()).unwrap();
    let version: i64 = db
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 5);
}

#[test]
fn granting_read_later_does_not_add_historical_content_to_a_write_only_receipt() {
    let mut t = Test::new();
    let (writer_id, writer) = t.issue(Grants {
        write: Some(Filter::All),
        ..Grants::default()
    });
    let req = create_request(1);
    let receipt = app::save(&mut t.store, &mut t.host, &writer, &req).unwrap();
    assert!(receipt.view.is_none());
    app::change_grants(
        &mut t.store,
        t.host.now,
        &t.admin,
        writer_id,
        read_write(),
        false,
    )
    .unwrap();
    t.restart();
    assert_eq!(
        app::save(&mut t.store, &mut t.host, &writer, &req).unwrap(),
        receipt
    );
    assert_eq!(t.get(&writer).content.body, "base");
}
