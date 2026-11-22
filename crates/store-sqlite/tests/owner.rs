use harmion_domain::{ClusterId, NodeId};
use harmion_node::{
    Error,
    access::Credential,
    application::{self as app, Host},
    owner,
    storage::{Identity, Store},
};
use harmion_protocol::{hex, owner::ChallengeRequest};
use harmion_store_sqlite::SqliteStore;
use std::{
    path::PathBuf,
    process::Command,
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
    keys: PathBuf,
    store: SqliteStore,
    host: TestHost,
    admin: Credential,
}
impl Test {
    fn new() -> Self {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("harmion-owner-test-{}-{n}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        let keys = path.join("keys");
        std::fs::create_dir(&keys).unwrap();
        let key = keys.join("owner_key");
        assert!(
            Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-C", "owner", "-f"])
                .arg(&key)
                .status()
                .unwrap()
                .success()
        );
        let mut store = SqliteStore::open(path.join("node.db"), identity()).unwrap();
        let mut host = TestHost {
            next: n * 100_000,
            now: 100,
        };
        let (_, admin) = app::bootstrap_admin(&mut store, &mut host, 10_000).unwrap();
        Self {
            path,
            keys,
            store,
            host,
            admin,
        }
    }

    fn public_key(&self) -> String {
        std::fs::read_to_string(self.keys.join("owner_key.pub")).unwrap()
    }

    fn sign(&self, statement: &[u8]) -> Vec<u8> {
        let message = self.keys.join("statement");
        let mut signature = message.as_os_str().to_owned();
        signature.push(".sig");
        let signature = PathBuf::from(signature);
        let _ = std::fs::remove_file(&signature);
        std::fs::write(&message, statement).unwrap();
        assert!(
            Command::new("ssh-keygen")
                .args(["-Y", "sign", "-f"])
                .arg(self.keys.join("owner_key.pub"))
                .args(["-n", "harmion-owner"])
                .arg(&message)
                .stdin(std::process::Stdio::null())
                .status()
                .unwrap()
                .success()
        );
        std::fs::read(signature).unwrap()
    }

    fn enroll(&mut self) {
        let public = self.public_key();
        let id = self.host.random_bytes().unwrap();
        let recovery = [7; 32];
        self.store
            .transaction(|tx| owner::enroll_initial(tx, &public, "primary", &recovery, id, 100))
            .unwrap();
    }

    fn challenge(&mut self, operation: &str, body: &[u8]) -> harmion_protocol::owner::Challenge {
        let request = ChallengeRequest {
            operation: operation.to_owned(),
            target: None,
            args_sha256: hex(&harmion_protocol::owner::args_hash(body)),
            session: None,
        };
        let id = self.host.random_bytes().unwrap();
        let credential = self.admin.clone();
        self.store
            .transaction(move |tx| owner::challenge(tx, &credential, request, id, 200))
            .unwrap()
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
        node: NodeId([2; 32]),
        epoch: harmion_protocol::LogEpoch([3; 32]),
    }
}

fn statement(challenge: &harmion_protocol::owner::Challenge) -> Vec<u8> {
    harmion_protocol::unhex_bytes(&challenge.statement, 16 * 1024).unwrap()
}

#[test]
fn ssh_owner_assertion_is_single_use_and_bound_to_the_body() {
    let mut test = Test::new();
    test.enroll();

    let body = br#"{"app_id":"00","revoked":false}"#;
    let challenge = test.challenge("change-grants", body);
    let signature = test.sign(&statement(&challenge));
    let replay = signature.clone();
    let id = harmion_protocol::unhex(&challenge.challenge).unwrap();
    let credential = test.admin.clone();
    test.store
        .transaction(move |tx| {
            owner::authorize(
                tx,
                &credential,
                "change-grants",
                body,
                Some(id),
                Some(&signature),
                "test",
                200,
            )
        })
        .unwrap();

    // The same challenge cannot authorize a second operation.
    let credential = test.admin.clone();
    let error = test
        .store
        .transaction(move |tx| {
            owner::authorize(
                tx,
                &credential,
                "change-grants",
                body,
                Some(id),
                Some(&replay),
                "test",
                200,
            )
        })
        .unwrap_err();
    assert!(matches!(error, Error::OwnerChallengeUsed));

    // A signature for one body cannot be moved to another body.
    let other = br#"{"app_id":"01","revoked":true}"#;
    let challenge = test.challenge("change-grants", other);
    let signature = test.sign(&statement(&challenge));
    let id = harmion_protocol::unhex(&challenge.challenge).unwrap();
    let credential = test.admin.clone();
    let error = test
        .store
        .transaction(move |tx| {
            owner::authorize(
                tx,
                &credential,
                "change-grants",
                body,
                Some(id),
                Some(&signature),
                "test",
                200,
            )
        })
        .unwrap_err();
    assert!(matches!(error, Error::OwnerAssertionInvalid));
}

#[test]
fn status_reports_enforcement_and_the_audit_chain() {
    let mut test = Test::new();
    test.enroll();
    let body = b"{}";
    let challenge = test.challenge("export", body);
    let signature = test.sign(&statement(&challenge));
    let id = harmion_protocol::unhex(&challenge.challenge).unwrap();
    let credential = test.admin.clone();
    test.store
        .transaction(move |tx| {
            owner::authorize(
                tx,
                &credential,
                "export",
                body,
                Some(id),
                Some(&signature),
                "test",
                200,
            )
        })
        .unwrap();
    let status = test.store.transaction(|tx| owner::status(tx)).unwrap();
    assert!(status.enforce);
    assert!(status.recovery_configured);
    assert!(status.audit_valid);
    assert_eq!(status.credentials.len(), 1);
    assert!(!status.credentials[0].revoked);
    assert!(status.audit.iter().any(|entry| entry.operation == "export"));
    assert!(
        status
            .audit
            .iter()
            .any(|entry| entry.operation == "owner-enroll")
    );
}

#[test]
fn recovery_replaces_all_credentials_and_rotates_the_code() {
    let mut test = Test::new();
    test.enroll();
    let public = test.public_key();
    let new_id = test.host.random_bytes().unwrap();
    let new_code = [8; 32];
    test.store
        .transaction(|tx| owner::recover(tx, &[7; 32], &public, "backup", &new_code, new_id, 300))
        .unwrap();
    let status = test.store.transaction(|tx| owner::status(tx)).unwrap();
    assert_eq!(status.credentials.len(), 2);
    assert!(
        status
            .credentials
            .iter()
            .filter(|credential| !credential.revoked)
            .count()
            == 1
    );
    assert!(status.recovery_configured);

    // The old recovery code is no longer valid.
    let another = test.host.random_bytes().unwrap();
    let error = test
        .store
        .transaction(|tx| owner::recover(tx, &[7; 32], &public, "again", &[9; 32], another, 400))
        .unwrap_err();
    assert!(matches!(error, Error::OwnerRecoveryInvalid));
}
