//! JSON operations behind the JNI surface.
//!
//! Every call returns one JSON object:
//! `{"ok":true, ...}` or `{"ok":false,"error":{"code":"…","message":"…"}}`.
//! No long-term credential is ever returned to Kotlin: the node app reads
//! `admin.secret` from its own app-private directory when it needs the admin
//! App API, and third-party apps never receive it.
use harmion_protocol::unhex_bytes;
use harmion_runtime::{
    BlockReason, ConfigOverrides, InitializeOptions, NodeConfig, NodeRuntime, RuntimeOptions,
    SyncTrigger, initialize as node_initialize, initialize::is_initialized, storage,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

static NEXT_HANDLE: AtomicU64 = AtomicU64::new(1);
static RUNTIMES: LazyLock<Mutex<BTreeMap<u64, Arc<Handle>>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

struct Handle {
    tokio: tokio::runtime::Runtime,
    node: NodeRuntime,
}

fn ok(value: Value) -> String {
    let mut object = match value {
        Value::Object(object) => object,
        other => {
            let mut object = serde_json::Map::new();
            object.insert("value".into(), other);
            object
        }
    };
    object.insert("ok".into(), Value::Bool(true));
    Value::Object(object).to_string()
}

fn error_reply(code: &str, message: impl Into<String>) -> String {
    json!({"ok": false, "error": {"code": code, "message": message.into()}}).to_string()
}

fn error_code(error: &harmion_runtime::Error) -> &'static str {
    use harmion_runtime::Error as E;
    match error {
        E::UnsupportedConfig(_) => "unsupported_config",
        E::InvalidListenerConfig => "invalid_listener",
        E::InvalidOAuthListener => "invalid_oauth_listener",
        E::MissingRelay => "missing_relay",
        E::InvalidKeyLength(_) => "invalid_key",
        E::SnapshotDirectory => "snapshot_directory",
        E::InvalidTrustRoot => "invalid_trust_root",
        E::Stopped => "stopped",
        E::SyncBlocked(_) => "sync_blocked",
        E::Start(_) => "start_failed",
        E::Io(_) => "io",
        E::Node(harmion_node::Error::AlreadyBootstrapped) => "already_enrolled",
        E::Node(_) => "node",
        E::Transport(_) => "transport",
        E::Json(_) => "invalid_json",
        E::Codec(_) => "codec",
    }
}

fn failure(error: &harmion_runtime::Error) -> String {
    error_reply(error_code(error), error.to_string())
}

/// Host-provided overrides. Unknown fields are rejected so a typo in the Kotlin
/// layer fails loudly instead of silently running with defaults.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct HostOptions {
    #[serde(default)]
    cluster_id: Option<String>,
    #[serde(default)]
    use_iroh: Option<bool>,
    #[serde(default)]
    sync_seconds: Option<u64>,
    #[serde(default)]
    public_lookup: Option<bool>,
    #[serde(default)]
    relay_urls: Option<Vec<String>>,
    #[serde(default)]
    app_listen: Option<String>,
    #[serde(default)]
    peer_listen: Option<String>,
    /// `"off"` disables the browser OAuth listener; absent keeps the saved value.
    #[serde(default)]
    oauth_listen: Option<String>,
    /// Additional DER trust anchors (hex) for signed-HTTPS peers.
    #[serde(default)]
    signed_https_roots: Option<Vec<String>>,
    /// Host policy evaluated before the scheduler spawns. A code means the
    /// gate starts blocked; absent means permitted.
    #[serde(default)]
    sync_blocked_code: Option<String>,
    #[serde(default)]
    sync_blocked_message: Option<String>,
    /// Tokio worker threads. Two is enough for a phone-sized node.
    #[serde(default)]
    worker_threads: Option<usize>,
}

fn parse_options(text: &str) -> Result<HostOptions, String> {
    if text.trim().is_empty() {
        return Ok(HostOptions::default());
    }
    serde_json::from_str(text)
        .map_err(|failure| error_reply("invalid_options", failure.to_string()))
}

fn address(text: &str, field: &str) -> Result<Option<std::net::SocketAddr>, String> {
    match text.parse() {
        Ok(address) => Ok(Some(address)),
        Err(_) => Err(format!("invalid {field}: {text}")),
    }
}

/// `None` means "keep the saved value"; `Some(None)` means "disable".
fn oauth_override(value: &Option<String>) -> Result<Option<Option<std::net::SocketAddr>>, String> {
    match value.as_deref() {
        None => Ok(None),
        Some("off") => Ok(Some(None)),
        Some(text) => Ok(Some(address(text, "oauth_listen")?)),
    }
}

fn overrides(options: &HostOptions) -> Result<ConfigOverrides, String> {
    Ok(ConfigOverrides {
        app_listen: match &options.app_listen {
            Some(text) => address(text, "app_listen")?,
            None => None,
        },
        peer_listen: match &options.peer_listen {
            Some(text) => address(text, "peer_listen")?,
            None => None,
        },
        oauth_listen: oauth_override(&options.oauth_listen)?,
        sync_seconds: options.sync_seconds,
        use_iroh: options.use_iroh,
        public_lookup: options.public_lookup,
        relay_urls: options.relay_urls.clone(),
        founder: None,
    })
}

fn trust_roots(roots: &[String]) -> Result<Vec<Vec<u8>>, String> {
    roots
        .iter()
        .map(|root| {
            let bytes =
                unhex_bytes(root, 65536).map_err(|_| "invalid signed_https_roots".to_owned())?;
            if bytes.is_empty() {
                return Err("empty signed_https_roots entry".to_owned());
            }
            Ok(bytes)
        })
        .collect()
}

/// Creates a node directory. Idempotent from the caller's point of view: when the
/// directory is already initialized the existing identity is returned instead of
/// rewriting keys.
pub fn initialize(root: &str, options: &str) -> String {
    let options = match parse_options(options) {
        Ok(options) => options,
        Err(reply) => return reply,
    };
    if is_initialized(root) {
        return identifiers(root);
    }
    let overrides = match overrides(&options) {
        Ok(overrides) => overrides,
        Err(message) => return error_reply("invalid_options", message),
    };
    let app_listen = overrides
        .app_listen
        .unwrap_or_else(|| "127.0.0.1:0".parse().expect("literal address"));
    let peer_listen = overrides
        .peer_listen
        .unwrap_or_else(|| "127.0.0.1:0".parse().expect("literal address"));
    let oauth_listen = match overrides.oauth_listen {
        None => Some("127.0.0.1:0".parse().expect("literal address")),
        Some(address) => address,
    };
    let init = InitializeOptions {
        cluster_id: options.cluster_id.clone(),
        use_iroh: options.use_iroh.unwrap_or(true),
        sync_seconds: options.sync_seconds.unwrap_or(30),
        relay_urls: options.relay_urls.clone(),
        public_lookup: options.public_lookup.unwrap_or(true),
        app_listen,
        peer_listen,
        oauth_listen,
    };
    match node_initialize(root, &init) {
        Ok(initialized) => ok(json!({
            "initialized": true,
            "cluster_id": initialized.config.cluster_id,
            "node_id": initialized.config.node_id,
            "founder": initialized.config.founder,
        })),
        Err(failure) => failure_reply(&failure),
    }
}

fn failure_reply(error: &harmion_runtime::Error) -> String {
    failure(error)
}

fn with_runtime<T>(
    handle: u64,
    operation: impl FnOnce(&Handle) -> Result<T, harmion_runtime::Error>,
) -> Result<T, String> {
    let runtime = RUNTIMES
        .lock()
        .expect("runtime registry")
        .get(&handle)
        .cloned()
        .ok_or_else(|| error_reply("unknown_handle", format!("no runtime for handle {handle}")))?;
    operation(&runtime).map_err(|failure| self::failure(&failure))
}

/// Starts the node. Returns a handle used by every later call.
pub fn start(root: &str, options: &str) -> String {
    let options = match parse_options(options) {
        Ok(options) => options,
        Err(reply) => return reply,
    };
    let overrides = match overrides(&options) {
        Ok(overrides) => overrides,
        Err(message) => return error_reply("invalid_options", message),
    };
    let signed_https_roots = match options
        .signed_https_roots
        .as_deref()
        .map(trust_roots)
        .transpose()
    {
        Ok(roots) => roots.unwrap_or_default(),
        Err(message) => return error_reply("invalid_options", message),
    };
    let worker_threads = options.worker_threads.unwrap_or(2).clamp(1, 8);
    let tokio = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(worker_threads)
        .enable_all()
        .thread_name("harmion-node")
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => return error_reply("io", error.to_string()),
    };
    let sync_blocked = match options.sync_blocked_code.as_deref() {
        Some(code) if !code.is_empty() => Some(harmion_runtime::BlockReason {
            code: code.to_owned(),
            message: options
                .sync_blocked_message
                .clone()
                .filter(|message| !message.is_empty()),
        }),
        _ => None,
    };
    let node = match tokio.block_on(NodeRuntime::start(
        root,
        RuntimeOptions {
            overrides,
            signed_https_roots,
            sync_gate: None,
            sync_blocked,
        },
    )) {
        Ok(node) => node,
        Err(failure) => return failure_reply(&failure),
    };
    let ready = json!({
        "app_endpoint": node.ready().app_endpoint,
        "peer": node.ready().peer,
        "oauth_origin": node.ready().oauth_origin,
    });
    let handle = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
    RUNTIMES
        .lock()
        .expect("runtime registry")
        .insert(handle, Arc::new(Handle { tokio, node }));
    ok(json!({"handle": handle, "ready": ready}))
}

pub fn status(handle: u64) -> String {
    match with_runtime(handle, |runtime| {
        runtime.tokio.block_on(runtime.node.status())
    }) {
        Ok(status) => match serde_json::to_value(status) {
            Ok(value) => ok(value),
            Err(error) => error_reply("serialize", error.to_string()),
        },
        Err(reply) => reply,
    }
}

/// Blocking manual sync. The caller (Kotlin UI) must have asked the user first.
pub fn sync_now(handle: u64) -> String {
    sync_with(handle, true)
}

/// Background sync: the host policy gate still applies.
pub fn sync_gated(handle: u64) -> String {
    sync_with(handle, false)
}

fn sync_with(handle: u64, manual: bool) -> String {
    match with_runtime(handle, |runtime| {
        runtime.tokio.block_on(async {
            if manual {
                runtime.node.sync_now().await
            } else {
                runtime.node.sync_gated().await
            }
        })
    }) {
        Ok(report) => match serde_json::to_value(report) {
            Ok(value) => ok(json!({"report": value})),
            Err(error) => error_reply("serialize", error.to_string()),
        },
        Err(reply) => reply,
    }
}

pub fn stop(handle: u64) -> String {
    let runtime = RUNTIMES.lock().expect("runtime registry").remove(&handle);
    let Some(runtime) = runtime else {
        return ok(json!({"stopped": false}));
    };
    match runtime.tokio.block_on(runtime.node.shutdown()) {
        Ok(()) => ok(json!({"stopped": true})),
        Err(failure) => failure_reply(&failure),
    }
}

/// Host policy update: `reason` is `""` or `{"code":"…","message":"…"}`.
pub fn set_sync_policy(handle: u64, permitted: bool, reason: &str) -> String {
    let reason = if reason.trim().is_empty() {
        None
    } else {
        match serde_json::from_str::<BlockReason>(reason) {
            Ok(reason) => Some(reason),
            Err(error) => return error_reply("invalid_reason", error.to_string()),
        }
    };
    match with_runtime(handle, |runtime| {
        runtime.node.set_sync_policy(permitted, reason);
        Ok(())
    }) {
        Ok(()) => ok(json!({"permitted": permitted})),
        Err(reply) => reply,
    }
}

pub fn request_sync(handle: u64, trigger: &str) -> String {
    let Some(trigger) = sync_trigger(trigger) else {
        return error_reply("invalid_trigger", "unknown sync trigger");
    };
    match with_runtime(handle, |runtime| {
        runtime.node.request_sync(trigger);
        Ok(())
    }) {
        Ok(()) => ok(json!({"requested": true, "trigger": trigger.as_str()})),
        Err(reply) => reply,
    }
}

fn sync_trigger(value: &str) -> Option<SyncTrigger> {
    Some(match value {
        "periodic" => SyncTrigger::Periodic,
        "local_save" => SyncTrigger::LocalSave,
        "app_connect" => SyncTrigger::AppConnect,
        "network_return" => SyncTrigger::NetworkReturn,
        "manual" => SyncTrigger::Manual,
        _ => return None,
    })
}

/// Identity of an initialized node, without starting it.
pub fn identifiers(root: &str) -> String {
    if !is_initialized(root) {
        return ok(json!({"initialized": false}));
    }
    let identifiers = match NodeRuntime::identifiers(root) {
        Ok(identifiers) => identifiers,
        Err(failure) => return failure_reply(&failure),
    };
    let founder = match storage::open_node(root)
        .and_then(|directory| storage::read_config(&directory).map(|config| config.founder))
    {
        Ok(founder) => founder,
        Err(failure) => return failure_reply(&failure),
    };
    ok(json!({
        "initialized": true,
        "cluster_id": harmion_protocol::hex(&identifiers.cluster.0),
        "node_id": harmion_protocol::hex(&identifiers.node.0),
        "founder": founder,
    }))
}

/// Persisted discovery information for the bound service. Public data only.
pub fn connection(root: &str) -> String {
    if !is_initialized(root) {
        return error_reply("not_initialized", "node directory is not initialized");
    }
    let directory = match storage::open_node(root) {
        Ok(directory) => directory,
        Err(failure) => return failure_reply(&failure),
    };
    let bytes = match directory.read("connection.json", 65536) {
        Ok(bytes) => bytes,
        Err(failure) => return error_reply("not_initialized", failure.to_string()),
    };
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(value) => ok(json!({"connection": value})),
        Err(failure) => error_reply("invalid_json", failure.to_string()),
    }
}

/// Effective persisted configuration for diagnostics.
pub fn config(root: &str) -> String {
    if !is_initialized(root) {
        return error_reply("not_initialized", "node directory is not initialized");
    }
    let directory = match storage::open_node(root) {
        Ok(directory) => directory,
        Err(failure) => return failure_reply(&failure),
    };
    match storage::read_config(&directory) {
        Ok(config) => match serde_json::to_value::<NodeConfig>(config) {
            Ok(value) => ok(json!({"config": value})),
            Err(failure) => error_reply("serialize", failure.to_string()),
        },
        Err(failure) => failure_reply(&failure),
    }
}

/// Test-only gate injection point. The default policy gate is what Android uses.
#[doc(hidden)]
pub fn gate_is_policy(handle: u64) -> Result<(), String> {
    with_runtime(handle, |_| Ok(()))
}

/// Browser OAuth trust root (hex DER). The bound service hands this to client
/// apps so they can pin the OAuth origin for refresh-token exchange.
pub fn oauth_root(root: &str) -> String {
    if !is_initialized(root) {
        return error_reply("not_initialized", "node directory is not initialized");
    }
    let directory = match storage::open_node(root) {
        Ok(directory) => directory,
        Err(failure) => return failure_reply(&failure),
    };
    match storage::oauth_tls(&directory) {
        Ok((tls, _)) => ok(json!({"root": harmion_protocol::hex(&tls.root_der)})),
        Err(failure) => failure_reply(&failure),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{path::PathBuf, sync::atomic::AtomicU64};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Base(PathBuf);

    impl Base {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "harmion-android-ffi-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Base {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn parse(reply: &str) -> Value {
        serde_json::from_str(reply).unwrap()
    }

    fn offline() -> String {
        json!({
            "use_iroh": false,
            "public_lookup": false,
            "sync_seconds": 0,
            "oauth_listen": "off",
            "worker_threads": 1,
        })
        .to_string()
    }

    #[test]
    fn initialize_start_status_stop_through_json() {
        let base = Base::new();
        let root = base.0.join("node");
        let root = root.to_str().unwrap();
        let init = parse(&initialize(root, ""));
        assert_eq!(init["ok"], true);
        assert_eq!(init["initialized"], true);
        assert_eq!(init["founder"], true);
        // Initializing again returns the same identity instead of rewriting keys.
        let again = parse(&initialize(root, &offline()));
        assert_eq!(again["node_id"], init["node_id"]);

        let started = parse(&start(root, &offline()));
        assert_eq!(started["ok"], true, "{started}");
        let handle = started["handle"].as_u64().unwrap();
        assert!(
            started["ready"]["app_endpoint"]
                .as_str()
                .unwrap()
                .starts_with("127.0.0.1:")
        );

        let view = parse(&status(handle));
        assert_eq!(view["ok"], true, "{view}");
        assert_eq!(view["node_id"], init["node_id"]);
        assert_eq!(view["membership"], "active");

        // The policy gate is host-driven; setting it does not stop the runtime.
        assert_eq!(
            parse(&set_sync_policy(
                handle,
                false,
                r#"{"code":"wifi_only","message":"waiting for Wi-Fi"}"#
            ))["permitted"],
            false
        );
        assert_eq!(
            parse(&request_sync(handle, "network_return"))["requested"],
            true
        );
        assert_eq!(parse(&status(handle))["running"], true);

        let synced = parse(&sync_now(handle));
        assert_eq!(synced["ok"], true, "{synced}");

        assert_eq!(parse(&stop(handle))["stopped"], true);
        assert_eq!(parse(&status(handle))["error"]["code"], "unknown_handle");
        assert_eq!(parse(&stop(handle))["stopped"], false);

        // Restart after stop works and keeps the identity.
        let restarted = parse(&start(root, &offline()));
        assert_eq!(restarted["ok"], true, "{restarted}");
        assert_eq!(
            parse(&stop(restarted["handle"].as_u64().unwrap()))["stopped"],
            true
        );
    }

    #[test]
    fn invalid_inputs_are_rejected_with_codes() {
        let base = Base::new();
        let root = base.0.join("node");
        let root = root.to_str().unwrap();
        assert_eq!(
            parse(&start(root, "not json"))["error"]["code"],
            "invalid_options"
        );
        assert_eq!(
            parse(&start(root, r#"{"watch_network":true}"#))["error"]["code"],
            "invalid_options"
        );
        assert_eq!(
            parse(&start(
                root,
                r#"{"sync_seconds":0,"app_listen":"not-an-address"}"#
            ))["error"]["code"],
            "invalid_options"
        );
        assert_eq!(parse(&start(root, "{}"))["error"]["code"], "io");
        assert_eq!(parse(&status(99))["error"]["code"], "unknown_handle");
        assert_eq!(
            parse(&request_sync(99, "periodic"))["error"]["code"],
            "unknown_handle"
        );

        let init = parse(&initialize(root, ""));
        assert_eq!(init["ok"], true);
        let handle = parse(&start(root, &offline()))["handle"].as_u64().unwrap();
        assert_eq!(
            parse(&request_sync(handle, "now"))["error"]["code"],
            "invalid_trigger"
        );
        assert_eq!(
            parse(&set_sync_policy(handle, false, "{bad"))["error"]["code"],
            "invalid_reason"
        );
        let _ = stop(handle);
    }

    #[test]
    fn identifiers_and_connection_are_available_before_start() {
        let base = Base::new();
        let root = base.0.join("node");
        let root = root.to_str().unwrap();
        assert_eq!(parse(&identifiers(root))["initialized"], false);
        assert_eq!(parse(&connection(root))["error"]["code"], "not_initialized");
        let init = parse(&initialize(root, "")).to_string();
        let init = parse(&init);
        let ids = parse(&identifiers(root));
        assert_eq!(ids["initialized"], true);
        assert_eq!(ids["node_id"], init["node_id"]);
        let handle = parse(&start(root, &offline()))["handle"].as_u64().unwrap();
        let connection = parse(&connection(root));
        assert_eq!(connection["connection"]["node_id"], init["node_id"]);
        let config = parse(&config(root));
        assert_eq!(config["config"]["sync_seconds"], 0);
        let _ = stop(handle);
    }
}

/// OpenSSH public key and fingerprint for an Ed25519 owner seed.
///
/// The seed never leaves the device: the caller (the node app) keeps it wrapped
/// by Keystore and passes it only for the duration of one signing operation.
pub fn owner_public_key(seed_hex: &str) -> String {
    let seed = match owner_seed(seed_hex) {
        Ok(seed) => seed,
        Err(reply) => return reply,
    };
    let (public_key, fingerprint) = harmion_node::owner::owner_public_key(&seed);
    ok(json!({"public_key": public_key, "fingerprint": fingerprint}))
}

/// Signs a node-issued owner statement. The signature is an armored SSHSIG in
/// the `harmion-owner` namespace, identical in form to `ssh-keygen -Y sign`.
pub fn owner_sign(seed_hex: &str, statement_hex: &str) -> String {
    let seed = match owner_seed(seed_hex) {
        Ok(seed) => seed,
        Err(reply) => return reply,
    };
    let statement = match unhex_bytes(statement_hex, 16 * 1024) {
        Ok(statement) if !statement.is_empty() => statement,
        _ => return error_reply("invalid_statement", "statement must be hex"),
    };
    ok(json!({"signature": harmion_node::owner::owner_sign(&seed, &statement)}))
}

fn owner_seed(seed_hex: &str) -> Result<[u8; 32], String> {
    harmion_protocol::unhex::<32>(seed_hex)
        .map_err(|_| error_reply("invalid_seed", "seed must be 32 hex bytes"))
}

/// Enrolls the first owner key for this node.
///
/// Enrollment is a host-only operation: the FFI is reachable only from the node
/// app's own process, and the administrative credential is checked first. The
/// returned recovery code is shown to the owner once and never stored in
/// plaintext.
/// Enrolls the first owner key for a running node.
///
/// Enrollment is a host-only operation: the FFI is reachable only from the node
/// app's own process, and the caller already holds the running runtime. The
/// returned recovery code is shown to the owner once and is never stored in
/// plaintext.
pub fn owner_enroll(handle: u64, public_key: &str, label: &str) -> String {
    match with_runtime(handle, |runtime| {
        runtime
            .tokio
            .block_on(runtime.node.enroll_owner(public_key, label))
    }) {
        Ok(enrollment) => ok(json!({
            "enrolled": true,
            "fingerprint": enrollment.fingerprint,
            "label": enrollment.label,
            "recovery_code": enrollment.recovery_code,
        })),
        Err(reply) => reply,
    }
}

#[cfg(test)]
mod owner_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Base(std::path::PathBuf);

    impl Drop for Base {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn owner_key_is_enrolled_and_signs_verifiable_assertions() {
        let base = Base(std::env::temp_dir().join(format!(
            "harmion-android-owner-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )));
        std::fs::create_dir_all(&base.0).unwrap();
        let root = base.0.join("node");
        let root = root.to_str().unwrap();
        let offline = json!({
            "use_iroh": false,
            "public_lookup": false,
            "sync_seconds": 0,
            "oauth_listen": "off",
            "worker_threads": 1,
        })
        .to_string();
        assert_eq!(parse(&initialize(root, &offline))["ok"], true);
        let started = parse(&start(root, &offline));
        assert_eq!(started["ok"], true, "{started}");
        let handle = started["handle"].as_u64().unwrap();

        let seed = [9u8; 32];
        let reply = parse(&owner_public_key(&harmion_protocol::hex(&seed)));
        assert_eq!(reply["ok"], true, "{reply}");
        let public_key = reply["public_key"].as_str().unwrap().to_owned();
        assert!(public_key.starts_with("ssh-ed25519 "));
        let fingerprint = reply["fingerprint"].as_str().unwrap().to_owned();
        assert!(fingerprint.starts_with("SHA256:"));

        let enrolled = parse(&owner_enroll(handle, &public_key, "primary"));
        assert_eq!(enrolled["ok"], true, "{enrolled}");
        assert_eq!(enrolled["enrolled"], true);
        assert_eq!(enrolled["fingerprint"], fingerprint);
        assert_eq!(enrolled["recovery_code"].as_str().unwrap().len(), 64);
        // A second enrollment must not silently replace the owner.
        assert_eq!(
            parse(&owner_enroll(handle, &public_key, "second"))["error"]["code"],
            "already_enrolled"
        );

        let statement = b"HOA1\0cluster=00\noperation=connect-approve\n";
        let signed = parse(&owner_sign(
            &harmion_protocol::hex(&seed),
            &harmion_protocol::hex(statement),
        ));
        assert_eq!(signed["ok"], true, "{signed}");
        let signature = signed["signature"].as_str().unwrap();
        assert!(signature.starts_with("-----BEGIN SSH SIGNATURE-----"));
        // The node's own verifier accepts what the app produced.
        let matched = harmion_node::owner::verify_ssh(
            std::slice::from_ref(&public_key),
            statement,
            signature.as_bytes(),
        )
        .unwrap();
        assert_eq!(matched, public_key);

        assert_eq!(
            parse(&owner_sign("zz", ""))["error"]["code"],
            "invalid_seed"
        );
        let _ = stop(handle);
    }

    fn parse(reply: &str) -> serde_json::Value {
        serde_json::from_str(reply).unwrap()
    }
}
