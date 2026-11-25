use harmion_client::Client;
use harmion_domain::*;
use harmion_local::ProtectedDir;
use harmion_protocol::*;
use harmion_runtime::{
    ConfigOverrides, InitializeOptions, NodeRuntime, RuntimeOptions, initialize, is_initialized,
};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Base(PathBuf);

impl Base {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "harmion-runtime-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn node(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Base {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn options() -> RuntimeOptions {
    RuntimeOptions {
        overrides: ConfigOverrides {
            use_iroh: Some(false),
            public_lookup: Some(false),
            relay_urls: Some(Vec::new()),
            sync_seconds: Some(0),
            oauth_listen: Some(None),
            ..ConfigOverrides::default()
        },
        ..RuntimeOptions::default()
    }
}

fn client(root: &Path) -> Client {
    let info: ConnectionInfo =
        serde_json::from_slice(&std::fs::read(root.join("connection.json")).unwrap()).unwrap();
    let secret: [u8; 32] = std::fs::read(root.join("admin.secret"))
        .unwrap()
        .try_into()
        .unwrap();
    Client::new(info, secret).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn initialize_start_status_stop_and_restart() {
    let base = Base::new();
    let root = base.node("node");
    assert!(!is_initialized(&root));
    let initialized = initialize(&root, &InitializeOptions::default()).unwrap();
    assert!(is_initialized(&root));
    assert!(initialized.config.founder);
    for file in [
        "node.db",
        "certificate.der",
        "private-key.der",
        "admin.secret",
        "iroh.secret",
        "oauth-identity.json",
        "node.json",
    ] {
        assert!(root.join(file).is_file(), "{file} must exist");
    }
    // Initialization is not idempotent: a second attempt must not rewrite keys.
    assert!(initialize(&root, &InitializeOptions::default()).is_err());

    let runtime = NodeRuntime::start(&root, options()).await.unwrap();
    let status = runtime.status().await.unwrap();
    assert!(status.running);
    assert_eq!(status.membership, "active");
    assert_eq!(status.peers, 0);
    assert_eq!(status.node_id, initialized.config.node_id);
    assert!(status.storage.database_bytes > 0);
    assert!(status.app_endpoint.starts_with("127.0.0.1:"));
    assert!(status.oauth_origin.is_none());
    assert!(!status.sync.enabled);

    // A second runtime must not open a second writer for the same directory.
    assert!(NodeRuntime::start(&root, options()).await.is_err());

    let connection: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("connection.json")).unwrap()).unwrap();
    assert_eq!(
        connection["endpoint"].as_str().unwrap(),
        runtime.ready().app_endpoint
    );
    assert_eq!(
        connection["node_id"].as_str().unwrap(),
        initialized.config.node_id
    );

    runtime.shutdown().await.unwrap();
    assert!(runtime.is_stopped());
    assert!(!runtime.status().await.unwrap().running);

    // Durable state reopens without replay, and the identity is stable.
    let restarted = NodeRuntime::start(&root, options()).await.unwrap();
    assert_eq!(restarted.ready().peer.node_id, initialized.config.node_id);
    restarted.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn join_approve_and_sync_both_directions_through_the_app_api() {
    let base = Base::new();
    let a_root = base.node("a");
    let b_root = base.node("b");
    let a = initialize(&a_root, &InitializeOptions::default()).unwrap();
    let join = InitializeOptions {
        cluster_id: Some(a.config.cluster_id.clone()),
        ..InitializeOptions::default()
    };
    let b = initialize(&b_root, &join).unwrap();
    assert!(!b.config.founder);
    assert_eq!(b.config.cluster_id, a.config.cluster_id);

    let a_runtime = NodeRuntime::start(&a_root, options()).await.unwrap();
    let b_runtime = NodeRuntime::start(&b_root, options()).await.unwrap();
    // A joiner is not active until the existing cluster admits it.
    assert_eq!(b_runtime.status().await.unwrap().membership, "unconfirmed");

    let a_client = client(&a_root);
    let b_client = client(&b_root);
    let a_registration = std::fs::read(a_root.join("peer.json")).unwrap();
    let b_directory = ProtectedDir::open(&b_root).unwrap();
    b_client
        .join_directory(&b_directory, &a_registration, false)
        .await
        .unwrap();
    // The registration now carries the cluster root the owner verified.
    let b_registration: serde_json::Value =
        serde_json::from_slice(&std::fs::read(b_root.join("peer.json")).unwrap()).unwrap();
    assert!(b_registration["cluster_root"].is_string());
    let approved = a_client
        .call("approve", &serde_json::to_vec(&b_registration).unwrap())
        .await
        .unwrap();
    assert_eq!(approved["approved"], true);

    let from_a = ContentId([4; 32]);
    a_client
        .save(&SaveRequest {
            request_id: RequestId([1; 32]),
            content_id: from_a,
            command: Command::Create {
                fields: [(Field::Body, Value::Body("from-a".into()))].into(),
            },
        })
        .await
        .unwrap();

    let report = b_runtime.sync_now().await.unwrap();
    assert!(
        report.peers.iter().all(|peer| peer.error.is_none()),
        "{report:?}"
    );
    assert_eq!(
        b_client.get(from_a).await.unwrap()["content"]["body"],
        "from-a"
    );
    // Admitting B also delivers the membership evidence, so B becomes active.
    assert_eq!(b_runtime.status().await.unwrap().membership, "active");

    let from_b = ContentId([5; 32]);
    b_client
        .save(&SaveRequest {
            request_id: RequestId([2; 32]),
            content_id: from_b,
            command: Command::Create {
                fields: [(Field::Body, Value::Body("from-b".into()))].into(),
            },
        })
        .await
        .unwrap();
    a_runtime.sync_now().await.unwrap();
    assert_eq!(
        a_client.get(from_b).await.unwrap()["content"]["body"],
        "from-b"
    );
    let status = a_runtime.status().await.unwrap();
    assert_eq!(status.peers, 1);
    assert_eq!(status.sync.rounds_completed, 0); // manual sync does not bump the scheduler
    assert!(status.sync.last_error.is_none());

    a_runtime.shutdown().await.unwrap();
    b_runtime.shutdown().await.unwrap();
}
