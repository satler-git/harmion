use harmion_domain::*;
use harmion_node::{
    Error,
    access::{Credential, Filter, Grants},
    application::{self as app, Host},
    interpreter::{invocation, registry},
    storage::{Identity, Store},
};
use harmion_protocol::AppId;
use harmion_protocol::interpreter::{
    CONTRACT_VERSION, CallRequest, ContentSelection, DefaultQuery, ProviderRegistration,
    ProviderStatus,
};
use harmion_protocol::{Command, LogEpoch, RequestId, SaveRequest};
use harmion_store_sqlite::SqliteStore;
use serde_json::json;
use std::{
    collections::BTreeMap,
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

struct Test {
    path: PathBuf,
    store: SqliteStore,
    host: TestHost,
    admin: Credential,
}
impl Test {
    fn new() -> Self {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "harmion-interpreter-test-{}-{n}",
            std::process::id()
        ));
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
    fn issue(&mut self, grants: Grants) -> Credential {
        app::issue_app(&mut self.store, &mut self.host, &self.admin, grants, 1000)
            .unwrap()
            .1
    }
    fn restart(&mut self) {
        self.store = SqliteStore::open(self.path.join("node.db"), identity()).unwrap();
    }
    fn create(&mut self) -> ContentId {
        let id = ContentId([1; 32]);
        let request = SaveRequest {
            request_id: RequestId([1; 32]),
            content_id: id,
            command: Command::Create {
                fields: BTreeMap::from([
                    (Field::Body, Value::Body("one".into())),
                    (Field::Kind("ledger".into()), Value::Kind(true)),
                ]),
            },
        };
        app::save(&mut self.store, &mut self.host, &self.admin, &request).unwrap();
        id
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
fn provide_only() -> Grants {
    Grants {
        provide: true,
        ..Grants::default()
    }
}
fn read_and_call() -> Grants {
    Grants {
        read: Some(Filter::All),
        call: true,
        ..Grants::default()
    }
}
fn manifest(name: &str) -> ProviderRegistration {
    ProviderRegistration {
        name: name.into(),
        contract_version: CONTRACT_VERSION,
        input_schema: json!({"type": "object"}),
        output_schema: json!({"type": "object"}),
        default_query: Some(DefaultQuery {
            kinds: vec!["ledger".into()],
        }),
    }
}

#[test]
fn registration_needs_provide_and_owner_approval_and_names_are_unique() {
    let mut t = Test::new();
    let provider = t.issue(provide_only());
    let other = t.issue(provide_only());
    let caller = t.issue(read_and_call());
    let registration = registry::register(
        &mut t.store,
        &mut t.host,
        &provider,
        manifest("ledger/balance"),
    )
    .unwrap();
    assert_eq!(registration.status, ProviderStatus::Pending);
    assert!(matches!(
        registry::authorize_attach(&mut t.store, &provider, t.host.now, registration.provider),
        Err(Error::InterpreterNotApproved)
    ));
    let own_pending = registry::list(&mut t.store, &provider, t.host.now).unwrap();
    assert_eq!(own_pending.len(), 1);
    assert!(
        registry::list(&mut t.store, &caller, t.host.now)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        registry::list(&mut t.store, &t.admin.clone(), t.host.now)
            .unwrap()
            .len(),
        1
    );

    let (approved, _) = registry::approve(
        &mut t.store,
        &mut t.host,
        &t.admin.clone(),
        registration.provider,
        false,
    )
    .unwrap();
    assert_eq!(approved.status, ProviderStatus::Approved);
    registry::authorize_attach(&mut t.store, &provider, t.host.now, registration.provider).unwrap();
    assert_eq!(
        registry::list(&mut t.store, &caller, t.host.now)
            .unwrap()
            .len(),
        1
    );

    let replacement = registry::register(
        &mut t.store,
        &mut t.host,
        &other,
        manifest("ledger/balance"),
    )
    .unwrap();
    assert!(matches!(
        registry::approve(
            &mut t.store,
            &mut t.host,
            &t.admin.clone(),
            replacement.provider,
            false
        ),
        Err(Error::InterpreterNameConflict)
    ));
    let (replaced, superseded) = registry::approve(
        &mut t.store,
        &mut t.host,
        &t.admin.clone(),
        replacement.provider,
        true,
    )
    .unwrap();
    assert_eq!(superseded, Some(registration.provider));
    assert_eq!(replaced.status, ProviderStatus::Approved);
    assert_eq!(
        registry::describe(
            &mut t.store,
            &t.admin.clone(),
            t.host.now,
            registration.provider
        )
        .unwrap()
        .status,
        ProviderStatus::Superseded
    );
    assert!(matches!(
        registry::authorize_attach(&mut t.store, &provider, t.host.now, registration.provider),
        Err(Error::InterpreterNotApproved)
    ));
    let visible = registry::list(&mut t.store, &caller, t.host.now).unwrap();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].provider, replacement.provider);
}

#[test]
fn approved_manifest_change_requires_reapproval() {
    let mut t = Test::new();
    let provider = t.issue(provide_only());
    let registration = registry::register(
        &mut t.store,
        &mut t.host,
        &provider,
        manifest("ledger/balance"),
    )
    .unwrap();
    registry::approve(
        &mut t.store,
        &mut t.host,
        &t.admin.clone(),
        registration.provider,
        false,
    )
    .unwrap();

    // Re-registering the exact same definition stays approved and keeps the id.
    let same = registry::register(
        &mut t.store,
        &mut t.host,
        &provider,
        manifest("ledger/balance"),
    )
    .unwrap();
    assert_eq!(same.provider, registration.provider);
    assert_eq!(same.status, ProviderStatus::Approved);

    // A changed default query widens what callers are told to send; the owner
    // approved the previous contract, so this must go back to pending.
    let mut changed = manifest("ledger/balance");
    changed.default_query = Some(DefaultQuery {
        kinds: vec!["wiki".into()],
    });
    let updated = registry::register(&mut t.store, &mut t.host, &provider, changed).unwrap();
    assert_eq!(updated.provider, registration.provider);
    assert_eq!(updated.status, ProviderStatus::Pending);

    // The status must persist, not only be reflected in the returned value.
    t.restart();
    let stored = registry::list(&mut t.store, &t.admin.clone(), t.host.now).unwrap();
    assert!(stored.iter().any(|registration| {
        registration.provider == updated.provider && registration.status == ProviderStatus::Pending
    }));
    assert!(matches!(
        registry::authorize_attach(&mut t.store, &provider, t.host.now, updated.provider),
        Err(Error::InterpreterNotApproved)
    ));
}

#[test]
fn call_materializes_the_caller_selected_snapshot_and_checks_read() {
    let mut t = Test::new();
    let provider = t.issue(provide_only());
    let caller = t.issue(read_and_call());
    let registration = registry::register(
        &mut t.store,
        &mut t.host,
        &provider,
        manifest("ledger/balance"),
    )
    .unwrap();
    registry::approve(
        &mut t.store,
        &mut t.host,
        &t.admin.clone(),
        registration.provider,
        false,
    )
    .unwrap();
    let content = t.create();
    let view = app::get(&mut t.store, &mut t.host, &caller, content).unwrap();
    let pinned = view.edit_token;
    // Move the latest value forward after the caller fixed its snapshot.
    let edited = app::save(
        &mut t.store,
        &mut t.host,
        &t.admin.clone(),
        &SaveRequest {
            request_id: RequestId([2; 32]),
            content_id: content,
            command: Command::Edit {
                token: view.edit_token,
                fields: BTreeMap::from([(Field::Body, Value::Body("two".into()))]),
            },
        },
    );
    assert!(edited.is_ok(), "{edited:?}");
    let request = CallRequest {
        provider: "ledger/balance".into(),
        input: json!({"currency": "JPY"}),
        contents: vec![ContentSelection {
            content,
            token: pinned,
        }],
        timeout_ms: 5_000,
    };
    let prepared = invocation::prepare_call(&mut t.store, &mut t.host, &caller, &request).unwrap();
    assert_eq!(prepared.provider, registration.provider);
    assert_eq!(prepared.provider_name, "ledger/balance");
    assert_eq!(prepared.contents.len(), 1);
    assert_eq!(prepared.contents[0].content["body"], "one");
    assert_eq!(prepared.contents[0].content["id"], "01".repeat(32));
    assert!(!prepared.contents[0].conflict);
    assert_eq!(prepared.input, json!({"currency": "JPY"}));

    let unreadable = t.issue(Grants {
        call: true,
        ..Grants::default()
    });
    assert!(matches!(
        invocation::prepare_call(&mut t.store, &mut t.host, &unreadable, &request),
        Err(Error::NotFound)
    ));
    let no_call = t.issue(Grants {
        read: Some(Filter::All),
        ..Grants::default()
    });
    assert!(matches!(
        invocation::prepare_call(&mut t.store, &mut t.host, &no_call, &request),
        Err(Error::Forbidden)
    ));
    let other = ContentId([9; 32]);
    let wrong = CallRequest {
        contents: vec![ContentSelection {
            content: other,
            token: pinned,
        }],
        ..request.clone()
    };
    assert!(matches!(
        invocation::prepare_call(&mut t.store, &mut t.host, &caller, &wrong),
        Err(Error::InvalidToken)
    ));
}

#[test]
fn approval_and_grants_survive_restart_and_revoked_provider_is_unavailable() {
    let mut t = Test::new();
    let provider = t.issue(provide_only());
    let caller = t.issue(read_and_call());
    let registration = registry::register(
        &mut t.store,
        &mut t.host,
        &provider,
        manifest("ledger/balance"),
    )
    .unwrap();
    registry::approve(
        &mut t.store,
        &mut t.host,
        &t.admin.clone(),
        registration.provider,
        false,
    )
    .unwrap();
    let content = t.create();
    let token = app::get(&mut t.store, &mut t.host, &caller, content)
        .unwrap()
        .edit_token;
    t.restart();
    assert_eq!(
        registry::describe(&mut t.store, &caller, t.host.now, registration.provider)
            .unwrap()
            .status,
        ProviderStatus::Approved
    );
    let request = CallRequest {
        provider: "ledger/balance".into(),
        input: json!(null),
        contents: vec![ContentSelection { content, token }],
        timeout_ms: 5_000,
    };
    assert!(invocation::prepare_call(&mut t.store, &mut t.host, &caller, &request).is_ok());
    let provider_id = t
        .store
        .transaction(|tx| Ok(tx.app_by_verifier(provider.verifier())?.map(|app| app.id)))
        .unwrap()
        .unwrap();
    app::change_grants(
        &mut t.store,
        t.host.now,
        &t.admin.clone(),
        provider_id,
        Grants::default(),
        false,
        None,
    )
    .unwrap();
    assert!(matches!(
        invocation::prepare_call(&mut t.store, &mut t.host, &caller, &request),
        Err(Error::InterpreterUnavailable)
    ));
}

#[test]
fn interpreter_grants_roundtrip_through_the_store() {
    let mut t = Test::new();
    let credential = t.issue(Grants {
        read: Some(Filter::All),
        call: true,
        provide: true,
        ..Grants::default()
    });
    let id: AppId = t
        .store
        .transaction(|tx| Ok(tx.app_by_verifier(credential.verifier())?.map(|app| app.id)))
        .unwrap()
        .unwrap();
    let app = t.store.transaction(|tx| tx.app_by_id(id)).unwrap().unwrap();
    assert!(app.grants.call);
    assert!(app.grants.provide);
    assert!(!app.grants.manage_apps);
}

#[test]
fn v13_database_gains_interpreter_columns_and_registry_without_losing_admin() {
    let mut t = Test::new();
    let admin = t.admin.clone();
    // Recreate the deployed v13 app table without the interpreter columns.
    {
        let db = rusqlite::Connection::open(t.path.join("node.db")).unwrap();
        db.execute_batch(
            "PRAGMA foreign_keys=OFF;
             CREATE TABLE apps_v13 (app_id BLOB PRIMARY KEY CHECK(length(app_id)=32),
                 verifier BLOB UNIQUE NOT NULL CHECK(length(verifier)=32),
                 expires_at INTEGER NOT NULL,
                 revoked INTEGER NOT NULL CHECK(revoked IN (0,1)),
                 read_all INTEGER CHECK(read_all IN (0,1)),
                 write_all INTEGER CHECK(write_all IN (0,1)),
                 manage_apps INTEGER NOT NULL CHECK(manage_apps IN (0,1)),
                 manage_nodes INTEGER NOT NULL CHECK(manage_nodes IN (0,1)));
             INSERT INTO apps_v13 SELECT app_id,verifier,expires_at,revoked,read_all,write_all,manage_apps,manage_nodes FROM apps;
             DROP TABLE apps;
             ALTER TABLE apps_v13 RENAME TO apps;
             DROP TABLE interpreters;
             PRAGMA user_version=13;",
        )
        .unwrap();
    }
    t.restart();
    let grants = harmion_node::access::permissions(&mut t.store, &admin, t.host.now).unwrap();
    assert_eq!(grants.read, Some(Filter::All));
    assert_eq!(grants.write, Some(Filter::All));
    assert!(grants.manage_apps && grants.manage_nodes);
    assert!(grants.is_administrator());
    assert!(!grants.call && !grants.provide);

    // The migrated database can still issue an interpreter-capable app and
    // persist a new registry.
    let provider = t.issue(provide_only());
    let registration = registry::register(
        &mut t.store,
        &mut t.host,
        &provider,
        manifest("ledger/balance"),
    )
    .unwrap();
    registry::approve(
        &mut t.store,
        &mut t.host,
        &admin,
        registration.provider,
        false,
    )
    .unwrap();
    t.restart();
    assert_eq!(
        registry::describe(&mut t.store, &admin, t.host.now, registration.provider)
            .unwrap()
            .status,
        ProviderStatus::Approved
    );
}

#[test]
fn oauth_preapproved_provider_name_is_auto_approved_but_conflicts_are_not() {
    let mut t = Test::new();
    let provider = t.issue(Grants {
        provide: true,
        provide_names: ["ledger/balance".into()].into(),
        ..Grants::default()
    });
    let registration = registry::register(
        &mut t.store,
        &mut t.host,
        &provider,
        manifest("ledger/balance"),
    )
    .unwrap();
    assert_eq!(registration.status, ProviderStatus::Approved);
    assert!(registration.approved_at.is_some());

    let other = t.issue(Grants {
        provide: true,
        provide_names: ["ledger/balance".into()].into(),
        ..Grants::default()
    });
    assert!(matches!(
        registry::register(
            &mut t.store,
            &mut t.host,
            &other,
            manifest("ledger/balance")
        ),
        Err(Error::InterpreterNameConflict)
    ));
}

#[test]
fn manual_registration_and_approval_respect_provide_names() {
    let mut t = Test::new();
    let (allowed_app, allowed) = app::issue_app(
        &mut t.store,
        &mut t.host,
        &t.admin,
        Grants {
            provide: true,
            provide_names: ["allowed/name".into()].into(),
            ..Grants::default()
        },
        1000,
    )
    .unwrap();
    assert!(matches!(
        registry::register(&mut t.store, &mut t.host, &allowed, manifest("other/name")),
        Err(Error::Forbidden)
    ));
    let approved = registry::register(
        &mut t.store,
        &mut t.host,
        &allowed,
        manifest("allowed/name"),
    )
    .unwrap();
    assert_eq!(approved.status, ProviderStatus::Approved);

    // A pending registration created before a grant narrowing becomes
    // superseded and cannot be approved manually while the name is disallowed.
    let (broad_app, broad) =
        app::issue_app(&mut t.store, &mut t.host, &t.admin, provide_only(), 1000).unwrap();
    let pending =
        registry::register(&mut t.store, &mut t.host, &broad, manifest("allowed/name")).unwrap();
    assert_eq!(pending.status, ProviderStatus::Pending);
    app::change_grants(
        &mut t.store,
        t.host.now,
        &t.admin,
        broad_app,
        Grants {
            provide: true,
            provide_names: ["other/name".into()].into(),
            ..Grants::default()
        },
        false,
        None,
    )
    .unwrap();
    assert!(matches!(
        registry::approve(&mut t.store, &mut t.host, &t.admin, pending.provider, false),
        Err(Error::InterpreterNotApproved)
    ));
    assert_eq!(
        registry::describe(&mut t.store, &t.admin, t.host.now, approved.provider)
            .unwrap()
            .app,
        allowed_app
    );
}

#[test]
fn stream_prepare_accounts_exact_content_bytes_and_reports_limits() {
    let mut t = Test::new();
    let provider = t.issue(provide_only());
    let caller = t.issue(read_and_call());
    let registration = registry::register(
        &mut t.store,
        &mut t.host,
        &provider,
        manifest("ledger/balance"),
    )
    .unwrap();
    registry::approve(
        &mut t.store,
        &mut t.host,
        &t.admin.clone(),
        registration.provider,
        false,
    )
    .unwrap();
    let content = t.create();
    let token = app::get(&mut t.store, &mut t.host, &caller, content)
        .unwrap()
        .edit_token;
    let request = CallRequest {
        provider: "ledger/balance".into(),
        input: json!(null),
        contents: vec![ContentSelection { content, token }],
        timeout_ms: 5_000,
    };
    let prepared =
        invocation::prepare_stream_call(&mut t.store, &mut t.host, &caller, &request).unwrap();
    assert_eq!(prepared.contents.len(), 1);
    assert!(prepared.content_bytes > 0);
    let actual = prepared.content_bytes;
    match invocation::prepare_call_with_content_limit(
        &mut t.store,
        &mut t.host,
        &caller,
        &request,
        (actual - 1) as usize,
    ) {
        Err(invocation::PrepareCallError::InputTooLarge {
            actual_bytes,
            limit_bytes,
        }) => {
            assert_eq!(actual_bytes, actual);
            assert_eq!(limit_bytes, actual - 1);
        }
        Ok(_) => panic!("expected explicit limit error, got a prepared call"),
        Err(other) => panic!("expected explicit limit error, got {other:?}"),
    }
    // The streaming path is not constrained by the legacy single-frame cap when
    // its logical limit is larger, and zero contents remains valid.
    let empty = CallRequest {
        contents: Vec::new(),
        ..request
    };
    let prepared =
        invocation::prepare_stream_call(&mut t.store, &mut t.host, &caller, &empty).unwrap();
    assert!(prepared.contents.is_empty());
    assert_eq!(prepared.content_bytes, 0);
}
