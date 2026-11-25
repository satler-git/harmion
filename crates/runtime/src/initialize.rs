//! First-run initialization of a node directory.
//!
//! The file order is the existing contract: `node.json` is written last and is
//! the only initialization marker, so a crash mid-initialization never yields a
//! runnable node with partial keys.
use crate::{
    Result,
    config::{
        ADMIN_SECRET_FILE, CERTIFICATE_FILE, CONFIG_FILE, DATABASE_FILE, IROH_SECRET_FILE,
        NodeConfig, OAUTH_IDENTITY_FILE, PRIVATE_KEY_FILE,
    },
    storage::{self, OAuthIdentity},
};
use harmion_domain::{ClusterId, NodeId};
use harmion_local::ProtectedDir;
use harmion_net_native::{
    SystemHost,
    tls::{BrowserTls, TlsIdentity},
};
use harmion_node::{application, application::Host, storage::Identity};
use harmion_store_sqlite::SqliteStore;
use std::{net::SocketAddr, path::Path};

#[derive(Clone, Debug)]
pub struct InitializeOptions {
    /// Existing cluster ID (hex) to reference. `None` creates a founder cluster.
    /// A joining node still needs an out-of-band membership approval; this only
    /// sets the cluster it will ask to join.
    pub cluster_id: Option<String>,
    pub use_iroh: bool,
    pub sync_seconds: u64,
    pub relay_urls: Option<Vec<String>>,
    pub public_lookup: bool,
    pub app_listen: SocketAddr,
    pub peer_listen: SocketAddr,
    pub oauth_listen: Option<SocketAddr>,
}

impl Default for InitializeOptions {
    fn default() -> Self {
        Self {
            cluster_id: None,
            use_iroh: true,
            sync_seconds: 30,
            relay_urls: None,
            public_lookup: true,
            app_listen: "127.0.0.1:0".parse().expect("literal address"),
            peer_listen: "127.0.0.1:0".parse().expect("literal address"),
            oauth_listen: Some("127.0.0.1:0".parse().expect("literal address")),
        }
    }
}

#[derive(Clone, Debug)]
pub struct InitializedNode {
    pub config: NodeConfig,
}

/// True when `root` already contains a completed initialization.
pub fn is_initialized(root: impl AsRef<Path>) -> bool {
    root.as_ref().join(CONFIG_FILE).is_file()
}

pub fn initialize(root: impl AsRef<Path>, options: &InitializeOptions) -> Result<InitializedNode> {
    let founder = options.cluster_id.is_none();
    let cluster = match &options.cluster_id {
        Some(id) => harmion_protocol::unhex(id)?,
        None => SystemHost.random_bytes()?,
    };
    let config = NodeConfig {
        version: crate::config::CONFIG_VERSION,
        cluster_id: harmion_protocol::hex(&cluster),
        node_id: harmion_protocol::hex(&SystemHost.random_bytes()?),
        log_epoch: harmion_protocol::hex(&SystemHost.random_bytes()?),
        app_listen: options.app_listen,
        oauth_listen: options.oauth_listen,
        peer_listen: options.peer_listen,
        sync_seconds: options.sync_seconds,
        use_iroh: options.use_iroh,
        relay_urls: options.relay_urls.clone(),
        public_lookup: options.public_lookup,
        founder,
    };
    config.validate()?;
    let identity = config.identity()?;
    let tls = TlsIdentity::generate()?;
    let oauth_tls = BrowserTls::generate(&format!("OAuth {}", &config.node_id[..4]))?;
    let directory = ProtectedDir::create(root)?;
    let _lock = directory.lock()?;
    storage::create_database_file(&directory)?;
    let mut store = SqliteStore::open(directory.path().join(DATABASE_FILE), identity)?;
    let (_, credential) = application::bootstrap_admin(&mut store, &mut SystemHost, i64::MAX)?;
    directory.write_new(CERTIFICATE_FILE, tls.certificate())?;
    directory.write_new(PRIVATE_KEY_FILE, tls.private_key_der())?;
    directory.write_new(ADMIN_SECRET_FILE, credential.expose_bytes())?;
    directory.write_new(IROH_SECRET_FILE, &SystemHost.random_bytes()?)?;
    directory.write_new(
        OAUTH_IDENTITY_FILE,
        &serde_json::to_vec(&OAuthIdentity {
            root_der: oauth_tls.root_der.clone(),
            leaf_der: oauth_tls.leaf_der.clone(),
            private_key_der: oauth_tls.private_key_der.clone(),
        })?,
    )?;
    // Last file is the initialization marker. Partial initialization never opens as a valid node.
    directory.write_new(CONFIG_FILE, &serde_json::to_vec(&config)?)?;
    Ok(InitializedNode { config })
}

/// Restore a backed-up node directory into a new node.
///
/// Only the database content and the identity of the backup are carried over;
/// fresh transport keys and credentials are generated for the restored node.
pub fn restore(
    backup_root: impl AsRef<Path>,
    destination: impl AsRef<Path>,
    options: &InitializeOptions,
) -> Result<InitializedNode> {
    let backup = ProtectedDir::open(backup_root)?;
    // Reject a live node, but do not create a lock inside a static snapshot.
    let _lock = backup.try_lock_existing()?;
    let config = storage::read_config(&backup)?;
    storage::verify_database_files(&backup)?;
    let backup_identity = config.identity()?;
    let cluster: [u8; 32] = harmion_protocol::unhex(&config.cluster_id)?;
    // A restored node is a NEW node in the same cluster. The backup's own node
    // identity must never be reused: `restore_backup` rejects it, and reusing it
    // would clone an admitted member onto a second device.
    let restored_config = NodeConfig {
        version: crate::config::CONFIG_VERSION,
        cluster_id: config.cluster_id.clone(),
        node_id: harmion_protocol::hex(&SystemHost.random_bytes()?),
        log_epoch: harmion_protocol::hex(&SystemHost.random_bytes()?),
        app_listen: options.app_listen,
        oauth_listen: options.oauth_listen,
        peer_listen: options.peer_listen,
        sync_seconds: options.sync_seconds,
        use_iroh: options.use_iroh,
        relay_urls: options.relay_urls.clone(),
        public_lookup: options.public_lookup,
        founder: false,
    };
    restored_config.validate()?;
    let restored_identity = Identity {
        cluster: ClusterId(cluster),
        node: NodeId(harmion_protocol::unhex(&restored_config.node_id)?),
        epoch: restored_config.identity()?.epoch,
    };
    let tls = TlsIdentity::generate()?;
    let oauth_tls = BrowserTls::generate(&format!("OAuth {}", &restored_config.node_id[..8]))?;
    let directory = ProtectedDir::create(destination)?;
    let _lock = directory.lock()?;
    storage::create_database_file(&directory)?;
    let mut store = SqliteStore::open(directory.path().join(DATABASE_FILE), restored_identity)?;
    store.restore_backup(backup.path().join(DATABASE_FILE), backup_identity)?;
    let (_, credential) = application::bootstrap_admin(&mut store, &mut SystemHost, i64::MAX)?;
    directory.write_new(CERTIFICATE_FILE, tls.certificate())?;
    directory.write_new(PRIVATE_KEY_FILE, tls.private_key_der())?;
    directory.write_new(ADMIN_SECRET_FILE, credential.expose_bytes())?;
    directory.write_new(IROH_SECRET_FILE, &SystemHost.random_bytes()?)?;
    directory.write_new(
        OAUTH_IDENTITY_FILE,
        &serde_json::to_vec(&OAuthIdentity {
            root_der: oauth_tls.root_der.clone(),
            leaf_der: oauth_tls.leaf_der.clone(),
            private_key_der: oauth_tls.private_key_der.clone(),
        })?,
    )?;
    directory.write_new(CONFIG_FILE, &serde_json::to_vec(&restored_config)?)?;
    Ok(InitializedNode {
        config: restored_config,
    })
}
/// Convenience: current cluster/node identifiers for display before the runtime
/// starts. Returns `Ok(None)` when the directory is not initialized yet.
pub fn identifiers(root: impl AsRef<Path>) -> Result<Option<(ClusterId, NodeId)>> {
    if !is_initialized(root.as_ref()) {
        return Ok(None);
    }
    let directory = ProtectedDir::open(root)?;
    let config = storage::read_config(&directory)?;
    let identity = config.identity()?;
    Ok(Some((identity.cluster, identity.node)))
}
