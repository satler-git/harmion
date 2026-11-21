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
use harmion_protocol::connect::{ConnectionOffer, ConnectionStatus, decode_offer};
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
    ProviderRegistration {
        name: "ledger/balance".into(),
        contract_version: CONTRACT_VERSION,
        input_schema: json!({"type": "object"}),
        output_schema: json!({"type": "object"}),
        default_query: Some(DefaultQuery {
            kinds: vec!["ledger".into()],
        }),
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
    let approved =
        connect::approve(&mut t.store, &mut t.host, &t.admin, started.id, true, false).unwrap();
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
    connect::approve(&mut t.store, &mut t.host, &t.admin, denied.id, false, false).unwrap();
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
    t.host.now += 6 * 60 * 1_000_000;
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
    let first_approval =
        connect::approve(&mut t.store, &mut t.host, &t.admin, first.id, true, false).unwrap();
    let first_provider = first_approval.provider.unwrap();
    connect::complete(&mut t.store, &mut t.host, first.id, first.secret, VERIFIER).unwrap();

    let second = connect::start(&mut t.store, &mut t.host, offer("Second", true)).unwrap();
    assert!(matches!(
        connect::approve(&mut t.store, &mut t.host, &t.admin, second.id, true, false),
        Err(Error::InterpreterNameConflict)
    ));
    let second_approval =
        connect::approve(&mut t.store, &mut t.host, &t.admin, second.id, true, true).unwrap();
    assert_eq!(second_approval.replaced, Some(first_provider));
    assert_eq!(
        registry::describe(&mut t.store, &t.admin, t.host.now, first_provider)
            .unwrap()
            .status,
        ProviderStatus::Superseded
    );
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
