use harmion_domain::*;
use harmion_node::{
    Error,
    access::{self, Credential, Filter},
    application::{self as app, Host},
    connect,
    interpreter::registry,
    oauth,
    storage::Identity,
};
use harmion_protocol::connect::{
    CONNECTION_TTL_SECONDS, ConnectionOffer, ConnectionStatus, decode_offer,
};
use harmion_protocol::interpreter::{
    CONTRACT_VERSION, DefaultQuery, ProviderRegistration, ProviderStatus,
};
use harmion_protocol::{FilterSpec, GrantSpec, LogEpoch};
use harmion_store_sqlite::SqliteStore;
use serde_json::json;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

const VERIFIER: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._~";

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
        let path =
            std::env::temp_dir().join(format!("harmion-connect-test-{}-{n}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        let mut store = SqliteStore::open(path.join("node.db"), identity()).unwrap();
        let mut host = TestHost {
            next: n * 100_000,
            now: 100,
        };
        let (_, admin) = app::bootstrap_admin(&mut store, &mut host, i64::MAX).unwrap();
        Self {
            path,
            store,
            host,
            admin,
        }
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
fn manifest() -> ProviderRegistration {
    manifest_named("ledger/balance")
}
fn manifest_named(name: &str) -> ProviderRegistration {
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
fn multi_offer(name: &str, provider_names: &[&str]) -> ConnectionOffer {
    ConnectionOffer {
        version: 1,
        name: name.into(),
        redirect_uri: "http://127.0.0.1:43123/callback".into(),
        web_origin: Some("http://127.0.0.1:43123".into()),
        client_id: None,
        challenge: oauth::pkce_challenge(VERIFIER).unwrap(),
        state: "state".into(),
        grants: GrantSpec {
            read: Some(FilterSpec::All),
            write: None,
            call: true,
            provide: true,
            provide_names: provider_names.iter().map(|name| (*name).into()).collect(),
            manage_apps: false,
            manage_nodes: false,
        },
        provider: None,
        providers: provider_names
            .iter()
            .map(|name| manifest_named(name))
            .collect(),
        provider_replace: false,
    }
}
fn offer(name: &str, provider: bool) -> ConnectionOffer {
    ConnectionOffer {
        version: 1,
        name: name.into(),
        redirect_uri: "http://127.0.0.1:43123/callback".into(),
        web_origin: Some("http://127.0.0.1:43123".into()),
        client_id: None,
        challenge: oauth::pkce_challenge(VERIFIER).unwrap(),
        state: "state".into(),
        grants: GrantSpec {
            read: Some(FilterSpec::All),
            write: None,
            call: true,
            provide: provider,
            provide_names: if provider {
                vec!["ledger/balance".into()]
            } else {
                Vec::new()
            },
            manage_apps: false,
            manage_nodes: false,
        },
        provider: provider.then(manifest),
        providers: Vec::new(),
        provider_replace: false,
    }
}

#[test]
fn integrated_connection_grants_app_and_provider_and_issues_tokens() {
    let mut t = Test::new();
    let offer = offer("AntiWebUI", true);
    decode_offer(&serde_json::to_vec(&offer).unwrap()).unwrap();
    let started = connect::start(&mut t.store, &mut t.host, offer.clone()).unwrap();
    assert_eq!(
        connect::status(&mut t.store, &mut t.host, started.id, started.secret)
            .unwrap()
            .status,
        "pending"
    );
    assert_eq!(
        connect::list(&mut t.store, &t.admin, t.host.now)
            .unwrap()
            .len(),
        1
    );
    let approved = connect::approve(
        &mut t.store,
        &mut t.host,
        &t.admin,
        started.id,
        true,
        false,
        None,
    )
    .unwrap();
    assert_eq!(approved.status, ConnectionStatus::Approved);
    let provider = approved.provider.unwrap();
    assert_eq!(
        registry::describe(&mut t.store, &t.admin, t.host.now, provider)
            .unwrap()
            .status,
        ProviderStatus::Approved
    );
    let completion = connect::complete(
        &mut t.store,
        &mut t.host,
        started.id,
        started.secret,
        VERIFIER,
    )
    .unwrap();
    assert_eq!(completion.provider, Some(provider));
    assert_eq!(completion.provider_name.as_deref(), Some("ledger/balance"));
    let permissions =
        access::permissions(&mut t.store, &completion.tokens.access, t.host.now).unwrap();
    assert_eq!(permissions.read, Some(Filter::All));
    assert!(permissions.call);
    assert!(permissions.provide);
    assert_eq!(
        permissions.provide_names,
        ["ledger/balance".to_string()].into()
    );
    assert!(matches!(
        connect::complete(
            &mut t.store,
            &mut t.host,
            started.id,
            started.secret,
            VERIFIER
        ),
        Err(Error::ConnectionCompleted)
    ));
    assert!(matches!(
        connect::status(&mut t.store, &mut t.host, started.id, [0; 32]),
        Err(Error::ConnectionNotFound)
    ));
}

#[test]
fn connection_denial_and_expiry_are_terminal() {
    let mut t = Test::new();
    let denied = connect::start(&mut t.store, &mut t.host, offer("Denied", false)).unwrap();
    connect::approve(
        &mut t.store,
        &mut t.host,
        &t.admin,
        denied.id,
        false,
        false,
        None,
    )
    .unwrap();
    assert!(matches!(
        connect::complete(
            &mut t.store,
            &mut t.host,
            denied.id,
            denied.secret,
            VERIFIER
        ),
        Err(Error::ConnectionDenied)
    ));
    let expired = connect::start(&mut t.store, &mut t.host, offer("Expired", false)).unwrap();
    t.host.now += (CONNECTION_TTL_SECONDS + 1) * 1_000_000;
    assert_eq!(
        connect::status(&mut t.store, &mut t.host, expired.id, expired.secret)
            .unwrap()
            .status,
        "expired"
    );
    assert!(matches!(
        connect::complete(
            &mut t.store,
            &mut t.host,
            expired.id,
            expired.secret,
            VERIFIER
        ),
        Err(Error::ConnectionExpired)
    ));
    t.host.now += 8 * 24 * 60 * 60 * 1_000_000;
    let fresh = connect::start(&mut t.store, &mut t.host, offer("Fresh", false)).unwrap();
    let listed = connect::list(&mut t.store, &t.admin, t.host.now).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, fresh.id);
}

#[test]
fn provider_replacement_requires_explicit_owner_approval() {
    let mut t = Test::new();
    let first = connect::start(&mut t.store, &mut t.host, offer("First", true)).unwrap();
    let first_approval = connect::approve(
        &mut t.store,
        &mut t.host,
        &t.admin,
        first.id,
        true,
        false,
        None,
    )
    .unwrap();
    let first_provider = first_approval.provider.unwrap();
    connect::complete(&mut t.store, &mut t.host, first.id, first.secret, VERIFIER).unwrap();

    let second = connect::start(&mut t.store, &mut t.host, offer("Second", true)).unwrap();
    assert!(matches!(
        connect::approve(
            &mut t.store,
            &mut t.host,
            &t.admin,
            second.id,
            true,
            false,
            None
        ),
        Err(Error::InterpreterNameConflict)
    ));
    let second_approval = connect::approve(
        &mut t.store,
        &mut t.host,
        &t.admin,
        second.id,
        true,
        true,
        None,
    )
    .unwrap();
    assert_eq!(second_approval.replaced, Some(first_provider));
    assert_eq!(
        registry::describe(&mut t.store, &t.admin, t.host.now, first_provider)
            .unwrap()
            .status,
        ProviderStatus::Superseded
    );
}

#[test]
fn multi_provider_connection_registers_all_and_persists_ids() {
    let mut t = Test::new();
    let names = ["wiki/backlinks", "wiki/outline", "wiki/transclude"];
    let expected_names = names
        .iter()
        .map(|name| (*name).to_string())
        .collect::<Vec<_>>();
    let started = connect::start(&mut t.store, &mut t.host, multi_offer("Wiki", &names)).unwrap();
    let pending = connect::list(&mut t.store, &t.admin, t.host.now).unwrap();
    assert_eq!(pending[0].provider_names().unwrap(), expected_names);

    let approved = connect::approve(
        &mut t.store,
        &mut t.host,
        &t.admin,
        started.id,
        true,
        false,
        None,
    )
    .unwrap();
    let providers = approved.providers.clone();
    assert_eq!(providers.len(), 3);
    assert_eq!(approved.provider, Some(providers[0]));
    assert_eq!(approved.provider_names, expected_names);
    assert_eq!(approved.replaced_providers, Vec::new());
    for provider in &providers {
        assert_eq!(
            registry::describe(&mut t.store, &t.admin, t.host.now, *provider)
                .unwrap()
                .status,
            ProviderStatus::Approved
        );
    }
    let view = connect::status(&mut t.store, &mut t.host, started.id, started.secret).unwrap();
    assert_eq!(view.provider_name.as_deref(), Some("wiki/backlinks"));
    assert_eq!(view.provider_names, expected_names);

    // The unpacked `providers` column must round-trip through a real reopen.
    let path = t.path.join("node.db");
    t.store = SqliteStore::open(&path, identity()).unwrap();
    let listed = connect::list(&mut t.store, &t.admin, t.host.now).unwrap();
    assert_eq!(listed[0].provider_ids, providers);
    assert_eq!(listed[0].provider_id, Some(providers[0]));
    assert_eq!(listed[0].provider_names().unwrap(), expected_names);

    let completion = connect::complete(
        &mut t.store,
        &mut t.host,
        started.id,
        started.secret,
        VERIFIER,
    )
    .unwrap();
    assert_eq!(completion.providers, providers);
    assert_eq!(completion.provider_names, expected_names);
    assert_eq!(completion.provider, Some(providers[0]));
    assert_eq!(completion.provider_name.as_deref(), Some("wiki/backlinks"));
}

#[test]
fn one_provider_name_conflict_rejects_the_whole_multi_provider_request() {
    let mut t = Test::new();
    let first = connect::start(
        &mut t.store,
        &mut t.host,
        multi_offer("First", &["wiki/backlinks"]),
    )
    .unwrap();
    let first_provider = connect::approve(
        &mut t.store,
        &mut t.host,
        &t.admin,
        first.id,
        true,
        false,
        None,
    )
    .unwrap()
    .provider
    .unwrap();

    let second = connect::start(
        &mut t.store,
        &mut t.host,
        multi_offer("Second", &["wiki/backlinks", "wiki/outline"]),
    )
    .unwrap();
    assert!(matches!(
        connect::approve(
            &mut t.store,
            &mut t.host,
            &t.admin,
            second.id,
            true,
            false,
            None
        ),
        Err(Error::InterpreterNameConflict)
    ));
    // The conflicting approval must not leave the other manifest registered.
    let registrations = registry::list(&mut t.store, &t.admin, t.host.now).unwrap();
    assert_eq!(registrations.len(), 1);
    assert_eq!(registrations[0].provider, first_provider);

    let approved = connect::approve(
        &mut t.store,
        &mut t.host,
        &t.admin,
        second.id,
        true,
        true,
        None,
    )
    .unwrap();
    assert_eq!(approved.providers.len(), 2);
    assert_eq!(approved.replaced_providers, vec![first_provider]);
}

#[test]
fn legacy_single_object_manifest_row_still_approves_and_completes() {
    let mut t = Test::new();
    let started = connect::start(&mut t.store, &mut t.host, offer("Legacy", true)).unwrap();
    let sql = rusqlite::Connection::open(t.path.join("node.db")).unwrap();
    sql.execute(
        "UPDATE connection_requests SET provider_manifest=?2,providers='[]' WHERE id=?1",
        rusqlite::params![
            started.id.0.as_slice(),
            serde_json::to_vec(&manifest()).unwrap()
        ],
    )
    .unwrap();
    drop(sql);

    let approved = connect::approve(
        &mut t.store,
        &mut t.host,
        &t.admin,
        started.id,
        true,
        false,
        None,
    )
    .unwrap();
    let provider = approved.provider.unwrap();
    // Pre-v18 rows have no Provider ID array; completion falls back to `provider`.
    let sql = rusqlite::Connection::open(t.path.join("node.db")).unwrap();
    sql.execute(
        "UPDATE connection_requests SET providers='[]' WHERE id=?1",
        rusqlite::params![started.id.0.as_slice()],
    )
    .unwrap();
    drop(sql);

    let completion = connect::complete(
        &mut t.store,
        &mut t.host,
        started.id,
        started.secret,
        VERIFIER,
    )
    .unwrap();
    assert_eq!(completion.providers, vec![provider]);
    assert_eq!(completion.provider, Some(provider));
    assert_eq!(completion.provider_name.as_deref(), Some("ledger/balance"));
}

#[test]
fn schema_v17_database_upgrades_and_keeps_existing_connection_rows() {
    let mut t = Test::new();
    let started = connect::start(&mut t.store, &mut t.host, offer("Existing", true)).unwrap();
    let path = t.path.join("node.db");
    let store = std::mem::replace(
        &mut t.store,
        SqliteStore::open(":memory:", identity()).unwrap(),
    );
    drop(store);

    let sql = rusqlite::Connection::open(&path).unwrap();
    sql.execute_batch(
        "ALTER TABLE connection_requests DROP COLUMN providers; PRAGMA user_version=17;",
    )
    .unwrap();
    drop(sql);

    t.store = SqliteStore::open(&path, identity()).unwrap();
    let sql = rusqlite::Connection::open(&path).unwrap();
    let version: i64 = sql
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 19);
    let columns: i64 = sql
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('connection_requests') WHERE name='providers'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(columns, 1);
    drop(sql);

    let listed = connect::list(&mut t.store, &t.admin, t.host.now).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, started.id);
    assert_eq!(
        listed[0].provider_names().unwrap(),
        ["ledger/balance".to_string()]
    );
    assert!(listed[0].provider_ids.is_empty());
}

#[test]
fn oauth_authorization_grants_call_and_named_provide() {
    let mut t = Test::new();
    let admin = t.admin.clone();
    let client = oauth::register(
        &mut t.store,
        &mut t.host,
        &admin,
        "Web app".into(),
        "http://127.0.0.1:43123/callback".into(),
    )
    .unwrap();
    let grants = GrantSpec {
        read: Some(FilterSpec::All),
        write: None,
        call: true,
        provide: true,
        provide_names: vec!["ledger/balance".into()],
        manage_apps: false,
        manage_nodes: false,
    };
    let pending = oauth::start(
        &mut t.store,
        &mut t.host,
        oauth::Start {
            client: client.id,
            redirect: client.redirect.clone(),
            challenge: oauth::pkce_challenge(VERIFIER).unwrap(),
            state: "state".into(),
            grants: grants.clone().into(),
        },
    )
    .unwrap();
    let decision = oauth::decide(&mut t.store, &mut t.host, &admin, pending.request, true).unwrap();
    let tokens = oauth::exchange(
        &mut t.store,
        &mut t.host,
        client.id,
        decision.code.unwrap(),
        &client.redirect,
        VERIFIER,
    )
    .unwrap();
    let permissions = access::permissions(&mut t.store, &tokens.access, t.host.now).unwrap();
    assert!(permissions.call);
    assert!(permissions.provide);
    assert_eq!(
        permissions.provide_names,
        ["ledger/balance".to_string()].into()
    );
}

#[test]
fn approval_can_set_an_app_deadline() {
    let mut t = Test::new();
    let started = connect::start(&mut t.store, &mut t.host, offer("Expiry", false)).unwrap();
    let now = t.host.now;
    let deadline = now + 30 * 24 * 60 * 60 * 1_000_000;
    let approved = connect::approve(
        &mut t.store,
        &mut t.host,
        &t.admin,
        started.id,
        true,
        false,
        Some(deadline),
    )
    .unwrap();
    let app = approved.app.expect("approved app");
    let apps = app::list_apps(&mut t.store, &t.admin, t.host.now).unwrap();
    let stored = apps.iter().find(|candidate| candidate.id == app).unwrap();
    assert_eq!(stored.expires_at, deadline);

    let other = connect::start(&mut t.store, &mut t.host, offer("Expiry2", false)).unwrap();
    assert!(matches!(
        connect::approve(
            &mut t.store,
            &mut t.host,
            &t.admin,
            other.id,
            true,
            false,
            Some(now - 1),
        ),
        Err(Error::InvalidRequest)
    ));
}
