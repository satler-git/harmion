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
fn oauth_expiry_uses_persisted_deadlines_and_ipv6_loopback_callback() {
    use harmion_node::{access, oauth};
    let mut t = Test::new();
    let admin = t.admin.clone();
    let registered = "http://[::1]/callback";
    let callback = "http://[::1]:49152/callback";
    let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    let challenge = oauth::pkce_challenge(verifier).unwrap();
    assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    let client = oauth::register(
        &mut t.store,
        &mut t.host,
        &admin,
        "example".into(),
        registered.into(),
    )
    .unwrap();
    let mut code = || {
        let authorization = oauth::start(
            &mut t.store,
            &mut t.host,
            oauth::Start {
                client: client.id,
                redirect: callback.into(),
                challenge: challenge.clone(),
                state: "state".into(),
                grants: read_write(),
            },
        )
        .unwrap();
        oauth::decide(
            &mut t.store,
            &mut t.host,
            &admin,
            authorization.request,
            true,
        )
        .unwrap()
        .code
        .unwrap()
    };
    let fresh = code();
    let unused = code();
    let issued_at = t.host.now;
    let tokens = oauth::exchange(
        &mut t.store,
        &mut t.host,
        client.id,
        fresh,
        callback,
        verifier,
    )
    .unwrap();
    t.restart();
    t.host.now = issued_at + 60_000_000;
    assert!(matches!(
        oauth::exchange(
            &mut t.store,
            &mut t.host,
            client.id,
            unused,
            callback,
            verifier
        ),
        Err(Error::OAuthInvalidGrant)
    ));
    t.host.now = issued_at + 600_000_000;
    assert!(matches!(
        access::permissions(&mut t.store, &tokens.access, t.host.now),
        Err(Error::Unauthenticated)
    ));
    let rotated = oauth::refresh(&mut t.store, &mut t.host, client.id, tokens.refresh).unwrap();
    assert_eq!(
        access::permissions(&mut t.store, &rotated.access, t.host.now).unwrap(),
        read_write()
    );
    t.host.now = issued_at + 30 * 24 * 60 * 60 * 1_000_000;
    assert!(matches!(
        oauth::refresh(&mut t.store, &mut t.host, client.id, rotated.refresh),
        Err(Error::OAuthInvalidGrant)
    ));
}

#[test]
fn v3_peer_migration_preserves_tls_binding_and_separates_iroh_keys() {
    use harmion_node::membership::{self, Peer, PeerKey};
    let mut t = Test::new();
    let admin = t.admin.clone();
    // Simulate a deployed v3 database, retaining all other state.
    let db = rusqlite::Connection::open(t.path.join("node.db")).unwrap();
    db.execute_batch("DROP TABLE sync_failures; DROP TABLE sync_observations;
        DROP TABLE oauth_refresh; DROP TABLE oauth_access; DROP TABLE oauth_families;
        DROP TABLE oauth_codes; DROP TABLE oauth_pending; DROP TABLE oauth_clients;
        DROP TABLE grant_generations; DROP TABLE content_changes;
        DROP TABLE query_cursors; DROP TABLE query_basis; DROP TABLE query_sessions;
        DROP TABLE search_basis; DROP TABLE content_records;
        DROP TABLE search_elements; DROP TABLE search_attributes; DROP TABLE search_kinds; DROP TABLE search_contents;
        DROP TABLE membership_identity; DROP TABLE membership_recovery_roots; DROP TABLE membership_root; DROP TABLE membership_log; DROP TABLE membership_cursors; DROP TABLE identity_pins; DROP TABLE https_state; DROP TABLE https_challenges; DROP TABLE https_rate; DROP TABLE peers; CREATE TABLE peers(node BLOB PRIMARY KEY, certificate BLOB UNIQUE NOT NULL, endpoint TEXT NOT NULL, retired INTEGER NOT NULL); PRAGMA user_version=3;").unwrap();
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
    membership::approve(&mut t.store, 100, &admin, identity().cluster, peer.clone()).unwrap();
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
        membership::approve(&mut t.store, 100, &admin, identity().cluster, replacement),
        Err(Error::IdentityMismatch)
    ));
    membership::retire(&mut t.store, 100, &admin, peer.node).unwrap();
    assert!(matches!(
        membership::approve(&mut t.store, 100, &admin, identity().cluster, peer),
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
        Err(Error::TokenExpired)
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
fn selects_attribute_and_kind_from_fixed_candidates() {
    use harmion_protocol::FieldSelection;
    let mut t = Test::new();
    let token = t.create().view.unwrap().edit_token;
    let admin = t.admin.clone();
    let attribute = Field::Attribute {
        namespace: "wiki".into(),
        key: "title".into(),
    };
    let kind = Field::Kind("wiki".into());
    let absent = t
        .save(
            &admin,
            2,
            Command::Edit {
                token,
                fields: BTreeMap::from([
                    (attribute.clone(), Value::Absent),
                    (kind.clone(), Value::Kind(false)),
                ]),
            },
        )
        .unwrap();
    let present = t
        .save(
            &admin,
            3,
            Command::Edit {
                token,
                fields: BTreeMap::from([
                    (attribute.clone(), Value::Attribute(AttributeValue::Null)),
                    (kind.clone(), Value::Kind(true)),
                ]),
            },
        )
        .unwrap();
    let inspect = app::inspect(&mut t.store, &mut t.host, &admin, content()).unwrap();
    let late = t
        .save(
            &admin,
            4,
            Command::Edit {
                token,
                fields: BTreeMap::from([(
                    attribute.clone(),
                    Value::Attribute(AttributeValue::String("late".into())),
                )]),
            },
        )
        .unwrap();
    let command = Command::Select {
        token: inspect.view.edit_token,
        selections: [attribute.clone(), kind.clone()]
            .into_iter()
            .map(|field| {
                (
                    field,
                    FieldSelection {
                        version: absent.update_id,
                        replaces: BTreeSet::from([absent.update_id, present.update_id]),
                    },
                )
            })
            .collect(),
    };
    let encoded = harmion_protocol::encode_save_request(&request(5, command.clone())).unwrap();
    assert_eq!(
        harmion_protocol::decode_save_request(&encoded).unwrap(),
        request(5, command.clone())
    );
    let (_, regular) = t.issue(read_write());
    assert!(matches!(
        t.save(&regular, 5, command.clone()),
        Err(Error::Forbidden)
    ));
    let resolution = t.save(&admin, 5, command.clone()).unwrap();
    assert_eq!(t.save(&admin, 5, command).unwrap(), resolution);
    let snapshot = t.snapshot();
    assert!(!snapshot.content.kinds.contains("wiki"));
    assert_eq!(snapshot.fields[&kind].candidates.len(), 1);
    assert!(
        snapshot.fields[&attribute]
            .candidates
            .iter()
            .any(|candidate| {
                candidate.value == Value::Absent
                    && candidate.versions.contains(&resolution.update_id)
            })
    );
    assert!(
        snapshot.fields[&attribute]
            .candidates
            .iter()
            .any(|candidate| { candidate.versions.contains(&late.update_id) })
    );
}

#[test]
fn merge_base_is_scoped_to_the_fixed_owner_snapshot_and_reports_ambiguity() {
    use harmion_protocol::resolution::{MergeInput, MergeInputRequest, MergePart};
    let mut t = Test::new();
    let root = t.create();
    let initial = root.view.unwrap().edit_token;
    let admin = t.admin.clone();
    let a = t
        .save(
            &admin,
            2,
            Command::Edit {
                token: initial,
                fields: body("A"),
            },
        )
        .unwrap();
    let b = t
        .save(
            &admin,
            3,
            Command::Edit {
                token: initial,
                fields: body("B"),
            },
        )
        .unwrap();
    let first = app::inspect(&mut t.store, &mut t.host, &admin, content()).unwrap();
    let resolve = |value: &str| Command::Resolve {
        token: first.view.edit_token,
        changes: BTreeMap::from([(
            Field::Body,
            Change {
                value: Value::Body(value.into()),
                replaces: BTreeSet::from([a.update_id, b.update_id]),
            },
        )]),
    };
    let x = t.save(&admin, 4, resolve("X")).unwrap();
    let y = t.save(&admin, 5, resolve("Y")).unwrap();
    let saved = app::inspect(&mut t.store, &mut t.host, &admin, content()).unwrap();
    let request = MergeInputRequest {
        content: content(),
        token: saved.view.edit_token,
        left: x.update_id,
        right: y.update_id,
        part: MergePart::Base(None),
    };
    let (_, user) = t.issue(read_write());
    assert!(matches!(
        app::merge_input(&mut t.store, &user, t.host.now, request),
        Err(Error::Forbidden)
    ));
    assert!(
        matches!(app::merge_input(&mut t.store, &admin, t.host.now, request).unwrap(),
        MergeInput::ChooseBase { versions } if versions.iter().cloned().collect::<BTreeSet<_>>() ==
        [a.update_id, b.update_id].into_iter().map(|id| harmion_protocol::hex(&id.0)).collect())
    );
    let choose = |version| MergeInputRequest {
        part: MergePart::Base(Some(version)),
        ..request
    };
    assert!(
        matches!(app::merge_input(&mut t.store, &admin, t.host.now, choose(a.update_id)).unwrap(),
        MergeInput::Body { text, .. } if text == "A")
    );
    assert!(matches!(
        app::merge_input(&mut t.store, &admin, t.host.now, choose(root.update_id)),
        Err(Error::InvalidRequest)
    ));
    assert!(matches!(
        app::merge_input(
            &mut t.store,
            &admin,
            t.host.now,
            MergeInputRequest {
                token: first.view.edit_token,
                ..request
            }
        ),
        Err(Error::InvalidRequest)
    ));
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
    let indexed: i64 = db
        .query_row("SELECT count(*) FROM search_contents", [], |r| r.get(0))
        .unwrap();
    assert_eq!(indexed, 0);
    db.execute_batch("DROP TRIGGER fail_result").unwrap();
    t.create();
    assert_eq!(t.count(), 1);
}

#[test]
fn synchronized_content_does_not_transfer_credentials_tokens_or_request_results() {
    use harmion_node::replication;
    let mut t = Test::new();
    let admin = t.admin.clone();
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
        app::get(&mut remote, &mut t.host, &admin, content()),
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
        Err(Error::TokenExpired)
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
fn subscription_connects_frozen_snapshot_to_changes_and_rechecks_read() {
    use harmion_node::subscription::Subscriptions;
    use harmion_protocol::{query::Condition, subscription::Event};

    let mut t = Test::new();
    let admin = t.admin.clone();
    let grants = Grants {
        read: Some(wiki()),
        ..Grants::default()
    };
    let (app_id, reader) = t.issue(grants.clone());
    let initial_token = t.create().view.unwrap().edit_token;
    let mut subscriptions = Subscriptions::default();
    let Event::SnapshotBegin {
        subscription,
        cursor,
    } = subscriptions
        .start(&mut t.store, &mut t.host, &reader, Condition::And(vec![]))
        .unwrap()
    else {
        panic!("expected snapshot_begin")
    };
    assert_eq!(
        subscriptions
            .resume(&mut t.store, &mut t.host, &admin, subscription, cursor)
            .unwrap(),
        Some(Event::ResetRequired)
    );
    t.host.now += 1;
    let intermediate_token = t
        .save(
            &admin,
            2,
            Command::Edit {
                token: initial_token,
                fields: body("intermediate during snapshot"),
            },
        )
        .unwrap()
        .view
        .unwrap()
        .edit_token;
    t.host.now += 1;
    let latest_token = t
        .save(
            &admin,
            4,
            Command::Edit {
                token: intermediate_token,
                fields: body("updated during snapshot"),
            },
        )
        .unwrap()
        .view
        .unwrap()
        .edit_token;
    let Event::SnapshotChunk { items, cursor } = subscriptions
        .resume(&mut t.store, &mut t.host, &reader, subscription, cursor)
        .unwrap()
        .unwrap()
    else {
        panic!("expected snapshot_chunk")
    };
    assert_eq!(items[0].content.body, "base");
    let Event::SnapshotEnd { cursor: baseline } = subscriptions
        .resume(&mut t.store, &mut t.host, &reader, subscription, cursor)
        .unwrap()
        .unwrap()
    else {
        panic!("expected snapshot_end")
    };
    let Event::Changes {
        sequence,
        upserts,
        removes,
        ..
    } = subscriptions
        .resume(&mut t.store, &mut t.host, &reader, subscription, baseline)
        .unwrap()
        .unwrap()
    else {
        panic!("expected changes after snapshot_end")
    };
    assert_eq!(upserts[0].content.body, "updated during snapshot");
    assert_eq!(upserts.len(), 1);
    assert!(removes.is_empty());
    let Event::Changes {
        sequence: retried_sequence,
        cursor: applied,
        upserts,
        ..
    } = subscriptions
        .resume(&mut t.store, &mut t.host, &reader, subscription, baseline)
        .unwrap()
        .unwrap()
    else {
        panic!("expected retry from the last applied cursor")
    };
    assert!(retried_sequence > sequence);
    assert_eq!(upserts[0].content.body, "updated during snapshot");
    assert!(
        subscriptions
            .resume(&mut t.store, &mut t.host, &reader, subscription, applied)
            .unwrap()
            .is_none()
    );

    t.save(
        &admin,
        3,
        Command::Edit {
            token: latest_token,
            fields: [
                (Field::Body, Value::Body("now private".into())),
                (Field::Kind("wiki".into()), Value::Kind(false)),
            ]
            .into(),
        },
    )
    .unwrap();
    let Event::Changes {
        cursor,
        upserts,
        removes,
        ..
    } = subscriptions
        .resume(&mut t.store, &mut t.host, &reader, subscription, applied)
        .unwrap()
        .unwrap()
    else {
        panic!("expected removal")
    };
    assert!(upserts.is_empty());
    assert_eq!(removes, [content()]);

    // Shrinking and restoring a grant before resume still invalidates the generation.
    app::change_grants(
        &mut t.store,
        t.host.now,
        &admin,
        app_id,
        Grants::default(),
        false,
    )
    .unwrap();
    app::change_grants(&mut t.store, t.host.now, &admin, app_id, grants, false).unwrap();
    assert_eq!(
        subscriptions
            .resume(&mut t.store, &mut t.host, &reader, subscription, cursor)
            .unwrap(),
        Some(Event::ResetRequired)
    );
}

#[test]
fn subscription_requests_reset_when_retained_window_no_longer_covers_cursor() {
    use harmion_node::{storage::MAX_RETAINED_CHANGES, subscription::Subscriptions};
    use harmion_protocol::{query::Condition, subscription::Event};

    let mut t = Test::new();
    let admin = t.admin.clone();
    t.create();
    let mut subscriptions = Subscriptions::default();
    let Event::SnapshotBegin {
        subscription,
        cursor,
    } = subscriptions
        .start(&mut t.store, &mut t.host, &admin, Condition::And(vec![]))
        .unwrap()
    else {
        panic!("expected snapshot_begin")
    };
    let snapshot = t.snapshot();
    t.store
        .transaction(|tx| {
            for _ in 0..=MAX_RETAINED_CHANGES {
                tx.put_search_projection(&snapshot)?;
            }
            let window = tx.change_window()?;
            assert_eq!(window.discarded_through, 2);
            assert_eq!(
                tx.changes_after(0, u32::MAX)?.len() as u64,
                MAX_RETAINED_CHANGES
            );
            Ok(())
        })
        .unwrap();
    assert_eq!(
        subscriptions
            .resume(&mut t.store, &mut t.host, &admin, subscription, cursor)
            .unwrap(),
        Some(Event::ResetRequired)
    );
}

#[test]
fn public_query_uses_persisted_read_kinds_for_large_grants() {
    let mut t = Test::new();
    t.create();
    let mut kinds: BTreeSet<_> = (0..33_000).map(|id| format!("kind-{id}")).collect();
    kinds.insert("wiki".into());
    let (_, credential) = t.issue(Grants {
        read: Some(Filter::Kinds(kinds)),
        ..Grants::default()
    });
    let page = harmion_node::query::start(
        &mut t.store,
        &mut t.host,
        &credential,
        &harmion_protocol::query::Condition::And(vec![]),
        1,
    )
    .unwrap();
    assert_eq!(page.items[0].content.id, content());
}

#[test]
fn query_pages_freeze_membership_value_and_edit_basis_with_current_read_checks() {
    use harmion_node::query;
    use harmion_protocol::query::Condition;

    let mut t = Test::new();
    let admin = t.admin.clone();
    let (_, reader) = app::issue_app(
        &mut t.store,
        &mut t.host,
        &admin,
        Grants {
            read: Some(wiki()),
            write: Some(wiki()),
            ..Grants::default()
        },
        query::QUERY_TTL_MICROS * 2,
    )
    .unwrap();
    let create = |t: &mut Test, id: u8| {
        let mut req = create_request(id);
        req.content_id = ContentId([id; 32]);
        app::save(&mut t.store, &mut t.host, &admin, &req)
            .unwrap()
            .view
            .unwrap()
            .edit_token
    };
    create(&mut t, 1);
    let second_token = create(&mut t, 2);
    let third_token = create(&mut t, 3);
    let first = query::start(
        &mut t.store,
        &mut t.host,
        &reader,
        &Condition::Kind("wiki".into()),
        1,
    )
    .unwrap();
    assert_eq!(first.items[0].content.id, ContentId([1; 32]));
    let cursor = first.next.unwrap();
    assert!(matches!(
        query::next(&mut t.store, &mut t.host, &admin, cursor, 1),
        Err(Error::QueryExpired)
    ));

    t.host.now = 200;
    for (id, command) in [
        (
            2,
            Command::Edit {
                token: second_token,
                fields: body("new body"),
            },
        ),
        (
            3,
            Command::Edit {
                token: third_token,
                fields: [(Field::Kind("wiki".into()), Value::Kind(false))].into(),
            },
        ),
    ] {
        let mut req = request(10 + id, command);
        req.content_id = ContentId([id; 32]);
        app::save(&mut t.store, &mut t.host, &admin, &req).unwrap();
    }
    create(&mut t, 4);
    t.restart();
    let second = query::next(&mut t.store, &mut t.host, &reader, cursor, 1).unwrap();
    let repeated = query::next(&mut t.store, &mut t.host, &reader, cursor, 1).unwrap();
    assert_eq!(second.next, repeated.next);
    let item = &second.items[0];
    assert_eq!(item.content.id, ContentId([2; 32]));
    assert_eq!(item.content.body, "base");
    let mut req = request(
        40,
        Command::Edit {
            token: item.edit_token,
            fields: body("edit from frozen basis"),
        },
    );
    req.content_id = item.content.id;
    let saved = app::save(&mut t.store, &mut t.host, &reader, &req).unwrap();
    assert!(saved.view.unwrap().conflicts.contains(&Field::Body));
    let last = query::next(&mut t.store, &mut t.host, &reader, second.next.unwrap(), 1).unwrap();
    assert!(last.items.is_empty() && last.next.is_none());
    t.host.now = 100 + query::QUERY_TTL_MICROS;
    assert!(matches!(
        query::next(&mut t.store, &mut t.host, &reader, cursor, 1),
        Err(Error::QueryExpired)
    ));
}

#[test]
fn export_pins_all_content_at_start_and_reauthorizes_each_read() {
    use harmion_node::export::Snapshot;

    let mut t = Test::new();
    let admin = t.admin.clone();
    let (owner_id, owner) = t.issue(Grants::administrator());
    let (_, reader) = t.issue(read_write());
    let original = t.create();
    let mut deleted = create_request(2);
    deleted.content_id = ContentId([2; 32]);
    let deleted_view = app::save(&mut t.store, &mut t.host, &admin, &deleted)
        .unwrap()
        .view
        .unwrap();
    let mut delete = request(
        3,
        Command::Delete {
            token: deleted_view.edit_token,
        },
    );
    delete.content_id = deleted.content_id;
    app::save(&mut t.store, &mut t.host, &admin, &delete).unwrap();
    assert!(matches!(
        Snapshot::start(&mut t.store, &reader, t.host.now),
        Err(Error::Forbidden)
    ));
    let snapshot = Snapshot::start(&mut t.store, &owner, t.host.now).unwrap();
    t.save(
        &admin,
        4,
        Command::Edit {
            token: original.view.unwrap().edit_token,
            fields: body("after export began"),
        },
    )
    .unwrap();
    let mut late = create_request(5);
    late.content_id = ContentId([3; 32]);
    app::save(&mut t.store, &mut t.host, &admin, &late).unwrap();

    assert_eq!(
        snapshot
            .ids(&mut t.store, &owner, t.host.now, None)
            .unwrap(),
        [content(), deleted.content_id]
    );
    assert_eq!(
        snapshot
            .ids(&mut t.store, &owner, t.host.now, Some(content()))
            .unwrap(),
        [deleted.content_id]
    );
    assert_eq!(
        snapshot
            .content(&mut t.store, &owner, t.host.now, content())
            .unwrap()
            .snapshot(content())
            .unwrap()
            .content
            .body,
        "base"
    );
    assert_eq!(
        snapshot
            .content(&mut t.store, &owner, t.host.now, deleted.content_id)
            .unwrap()
            .snapshot(deleted.content_id)
            .unwrap()
            .life,
        Life::Deleted
    );
    assert!(matches!(
        snapshot.ids(&mut t.store, &admin, t.host.now, None),
        Err(Error::Forbidden)
    ));
    app::change_grants(
        &mut t.store,
        t.host.now,
        &admin,
        owner_id,
        Grants::administrator(),
        true,
    )
    .unwrap();
    assert!(matches!(
        snapshot.content(&mut t.store, &owner, t.host.now, content()),
        Err(Error::Unauthenticated)
    ));
}

#[test]
fn migration_preserves_v1_records_and_cursor_generation() {
    let mut t = Test::new();
    t.create();
    let all = harmion_protocol::query::Condition::And(vec![]);
    assert_eq!(
        t.store
            .transaction(|tx| tx.search_content_ids(&all, &Filter::All, None, 8))
            .unwrap(),
        [content()]
    );
    let db = rusqlite::Connection::open(t.path.join("node.db")).unwrap();
    // Reproduce the previous schema with its original log and identity intact.
    db.execute_batch(
        "DROP TABLE sync_failures; DROP TABLE sync_observations;
        DROP TABLE oauth_refresh; DROP TABLE oauth_access; DROP TABLE oauth_families;
        DROP TABLE oauth_codes; DROP TABLE oauth_pending; DROP TABLE oauth_clients;
        DROP TABLE grant_generations; DROP TABLE content_changes;
        DROP TABLE query_cursors; DROP TABLE query_basis; DROP TABLE query_sessions;
        DROP TABLE search_basis; DROP TABLE content_records;
        DROP TABLE search_elements; DROP TABLE search_attributes; DROP TABLE search_kinds; DROP TABLE search_contents;
        DROP TABLE request_results; DROP TABLE token_basis; DROP TABLE edit_tokens;
        DROP TABLE app_kinds; DROP TABLE apps; DROP TABLE identity_pins; DROP TABLE https_state; DROP TABLE https_challenges; DROP TABLE https_rate; DROP TABLE peers; DROP TABLE membership_identity; DROP TABLE membership_recovery_roots; DROP TABLE membership_root; DROP TABLE membership_log; DROP TABLE membership_cursors; PRAGMA user_version=1;",
    )
    .unwrap();
    t.restart();
    assert_eq!(t.count(), 1);
    assert_eq!(
        t.store
            .transaction(|tx| tx.search_content_ids(&all, &Filter::All, None, 8))
            .unwrap(),
        [content()]
    );
    assert_eq!(t.store.identity(), identity());
    assert_eq!(t.snapshot().content.body, "base");
    let (_, admin) = app::bootstrap_admin(&mut t.store, &mut t.host, 10_000).unwrap();
    app::get(&mut t.store, &mut t.host, &admin, content()).unwrap();
    let version: i64 = db
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 18);
}

#[test]
fn granting_read_later_does_not_add_historical_content_to_a_write_only_receipt() {
    let mut t = Test::new();
    let admin = t.admin.clone();
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
        &admin,
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

#[test]
fn retention_reclaims_old_tokens_and_receipts_and_keeps_grandfathered_rows() {
    use harmion_node::application::{TokenPurpose, TokenState};
    let mut t = Test::new();
    let admin = t.admin.clone();
    t.host.now = 1_000;
    let created = t.create();
    let token = created.view.unwrap().edit_token;
    t.host.now = 3_000;
    t.save(
        &admin,
        2,
        Command::Edit {
            token,
            fields: body("edited"),
        },
    )
    .unwrap();
    // A row created before the retention policy carries created_at = 0.
    t.store
        .transaction(|tx| {
            tx.insert_token(
                EditToken([7; 32]),
                &TokenState {
                    content_id: content(),
                    basis: BTreeSet::new(),
                    purpose: TokenPurpose::Edit,
                    created_at: 0,
                },
            )
        })
        .unwrap();

    let deleted = t.store.gc_tokens(2_000).unwrap();
    assert_eq!(deleted, 1);
    t.store
        .transaction(|tx| {
            assert!(tx.token(token)?.is_none());
            assert!(tx.token(EditToken([7; 32]))?.is_some());
            Ok(())
        })
        .unwrap();

    // A live receipt still answers the same request without the reclaimed basis.
    let replay = t
        .save(
            &admin,
            2,
            Command::Edit {
                token,
                fields: body("edited"),
            },
        )
        .unwrap();
    assert!(replay.view.is_some());
    // A new request with the reclaimed basis must ask the app to get again.
    assert!(matches!(
        t.save(
            &admin,
            3,
            Command::Edit {
                token,
                fields: body("new"),
            },
        ),
        Err(Error::TokenExpired)
    ));
}
