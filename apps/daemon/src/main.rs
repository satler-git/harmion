mod cli;

use clap::Parser;
use cli::{Cli, Command, Discovery, PeerTransport, RunOptions};
use harmion_domain::{ClusterId, NodeId};
use harmion_local::ProtectedDir;
use harmion_net_native::{
    NativeTransport, SharedStore, SystemHost, app, iroh_peer, oauth_browser, peer, sync,
    tls::{BrowserTls, TlsIdentity},
};
use harmion_node::{
    application::{self, Host},
    https_peer::ChallengeStore,
    storage::{Identity, Store},
};
use harmion_protocol::{ConnectionInfo, LogEpoch, PeerBinding, PeerRegistration, hex, unhex};
use harmion_store_sqlite::SqliteStore;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io::{self, Write},
    net::SocketAddr,
    sync::Arc,
    time::Duration,
};
use tokio::{net::TcpListener, sync::watch, time::Instant};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
/// Edit tokens and request receipts older than this are reclaimed. Rows that
/// predate the retention policy keep created_at=0 and are grandfathered.
const TOKEN_RETENTION_MICROS: i64 = 30 * 24 * 60 * 60 * 1_000_000;
const TOKEN_GC_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
#[derive(Debug, thiserror::Error)]
enum DaemonError {
    #[error("unsupported node config version: {0}")]
    UnsupportedConfig(u32),
    #[error("App must be loopback; Peer needs a concrete IP; sync seconds must be 0..3600")]
    InvalidListenerConfig,
    #[error("OAuth must listen on 127.0.0.1")]
    InvalidOAuthListener,
    #[error("discovery none requires at least one explicit relay URL")]
    MissingRelay,
    #[error("invalid {0} key length")]
    InvalidKeyLength(&'static str),
    #[error("sync task missing")]
    MissingSyncTask,
    #[error("{0} stopped unexpectedly")]
    ServiceStopped(&'static str),
    #[error("invalid signed-HTTPS trust root")]
    InvalidTrustRoot,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    version: u32,
    cluster_id: String,
    node_id: String,
    log_epoch: String,
    app_listen: SocketAddr,
    #[serde(default)]
    oauth_listen: Option<SocketAddr>,
    peer_listen: SocketAddr,
    sync_seconds: u64,
    #[serde(default)]
    use_iroh: bool,
    #[serde(default)]
    relay_urls: Option<Vec<String>>,
    #[serde(default = "enabled")]
    public_lookup: bool,
    #[serde(default)]
    founder: bool,
}
fn enabled() -> bool {
    true
}
impl Config {
    fn identity(&self) -> Result<Identity> {
        if self.version != 1 {
            return Err(DaemonError::UnsupportedConfig(self.version).into());
        }
        Ok(Identity {
            cluster: ClusterId(unhex(&self.cluster_id)?),
            node: NodeId(unhex(&self.node_id)?),
            epoch: LogEpoch(unhex(&self.log_epoch)?),
        })
    }
}
fn initialize(
    path: &str,
    cluster: Option<&str>,
    backup: Option<(&ProtectedDir, Identity)>,
) -> Result<()> {
    let founder = cluster.is_none();
    let cluster = match cluster {
        Some(id) => unhex(id)?,
        None => SystemHost.random_bytes()?,
    };
    let config = Config {
        version: 1,
        cluster_id: hex(&cluster),
        node_id: hex(&SystemHost.random_bytes()?),
        log_epoch: hex(&SystemHost.random_bytes()?),
        app_listen: "127.0.0.1:0".parse()?,
        oauth_listen: None,
        peer_listen: "127.0.0.1:0".parse()?,
        sync_seconds: 30,
        use_iroh: true,
        relay_urls: None,
        public_lookup: true,
        founder,
    };
    let tls = TlsIdentity::generate()?;
    let directory = ProtectedDir::create(path)?;
    let _lock = directory.lock()?;
    directory.write_new("node.db", &[])?;
    let mut store = SqliteStore::open(directory.path().join("node.db"), config.identity()?)?;
    if let Some((backup, identity)) = backup {
        store.restore_backup(backup.path().join("node.db"), identity)?;
    }
    let (_, credential) = application::bootstrap_admin(&mut store, &mut SystemHost, i64::MAX)?;
    directory.write_new("certificate.der", tls.certificate())?;
    directory.write_new("private-key.der", tls.private_key_der())?;
    directory.write_new("admin.secret", credential.expose_bytes())?;
    directory.write_new("iroh.secret", &SystemHost.random_bytes()?)?;
    // Last file is the initialization marker. Partial initialization never opens as a valid node.
    directory.write_new("node.json", &serde_json::to_vec(&config)?)?;
    println!(
        "{}",
        serde_json::json!({"initialized":true,"cluster_id":config.cluster_id,"node_id":config.node_id})
    );
    Ok(())
}

fn verify_database_files(directory: &ProtectedDir) -> Result<()> {
    directory.verify_file("node.db")?;
    for sidecar in ["node.db-wal", "node.db-shm", "node.db-journal"] {
        match directory.verify_file(sidecar) {
            Ok(()) => (),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

fn restore(backup: &str, destination: &str) -> Result<()> {
    let backup = ProtectedDir::open(backup)?;
    // Lock an existing daemon.lock so a live node directory is rejected, but do
    // not create a lock in a static snapshot (which may be offsite/read-only).
    let _lock = backup.try_lock_existing()?;
    let config: Config = serde_json::from_slice(&backup.read("node.json", 4096)?)?;
    verify_database_files(&backup)?;
    initialize(
        destination,
        Some(&config.cluster_id),
        Some((&backup, config.identity()?)),
    )
}

fn reset_peer_v2(path: &str) -> Result<()> {
    let directory = ProtectedDir::open(path)?;
    let _lock = directory.lock()?;
    let config: Config = serde_json::from_slice(&directory.read("node.json", 4096)?)?;
    let identity = config.identity()?;
    verify_database_files(&directory)?;
    let mut store = SqliteStore::open(directory.path().join("node.db"), identity)?;
    let mut host = SystemHost;
    let mut random = || host.random_bytes();
    let generation = store.challenge_transaction(|tx| tx.reset_generation(&mut random))?;
    println!(
        "{}",
        serde_json::json!({"reset":true,"generation":hex(&generation)})
    );
    Ok(())
}

fn snapshot(path: &str, destination: &str) -> Result<()> {
    let directory = ProtectedDir::open(path)?;
    let config: Config = serde_json::from_slice(&directory.read("node.json", 4096)?)?;
    config.identity()?;
    // A manual daemon run may leave -wal/-shm at 0644. Snapshot reads through
    // SQLite (VACUUM INTO) and never copies sidecars, so only the main DB file
    // needs the owner-only check here.
    directory.verify_file("node.db")?;
    let output = ProtectedDir::create(destination)?;
    // Mark first: a crash mid-snapshot must leave a directory that `run`
    // refuses instead of a runnable node with a partial database.
    output.write_new(
        "snapshot.json",
        &serde_json::to_vec(&serde_json::json!({
            "snapshot": true,
            "node_id": config.node_id,
        }))?,
    )?;
    harmion_store_sqlite::snapshot_database(
        &directory.path().join("node.db"),
        &output.path().join("node.db"),
    )?;
    for name in [
        "node.json",
        "admin.secret",
        "certificate.der",
        "private-key.der",
        "iroh.secret",
        "connection.json",
        "peer.json",
        OAUTH_IDENTITY_FILE,
    ] {
        match directory.read(name, 16 * 1024 * 1024) {
            Ok(bytes) => output.write_new(name, &bytes)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    println!(
        "{}",
        serde_json::json!({"snapshot":true,"path":output.path().to_string_lossy()})
    );
    Ok(())
}

const OAUTH_IDENTITY_FILE: &str = "oauth-identity.json";

#[derive(Serialize, Deserialize)]
struct OAuthIdentity {
    root_der: Vec<u8>,
    leaf_der: Vec<u8>,
    private_key_der: Vec<u8>,
}

fn provision_oauth(path: &str, replace: bool) -> Result<()> {
    let directory = ProtectedDir::open(path)?;
    let _lock = directory.lock()?;
    let config: Config = serde_json::from_slice(&directory.read("node.json", 4096)?)?;
    let identity = config.identity()?;
    let tls = BrowserTls::generate(&format!("OAuth {}", hex(&identity.node.0[..4])))?;
    let pem = tls.root_pem();
    let bytes = serde_json::to_vec(&OAuthIdentity {
        root_der: tls.root_der,
        leaf_der: tls.leaf_der,
        private_key_der: tls.private_key_der,
    })?;
    if replace {
        directory.replace(OAUTH_IDENTITY_FILE, &bytes)?;
    } else {
        directory.write_new(OAUTH_IDENTITY_FILE, &bytes)?;
    }
    print!("{pem}");
    Ok(())
}

fn browser_tls(directory: &ProtectedDir) -> Result<BrowserTls> {
    let identity: OAuthIdentity =
        serde_json::from_slice(&directory.read(OAUTH_IDENTITY_FILE, 65_536)?)?;
    let tls = BrowserTls {
        root_der: identity.root_der,
        leaf_der: identity.leaf_der,
        private_key_der: identity.private_key_der,
    };
    tls.server_config()?;
    Ok(tls)
}

#[derive(Default)]
struct SyncSchedule {
    retries: BTreeMap<NodeId, (u32, Instant)>,
    inflight: BTreeSet<NodeId>,
    pending_saves: BTreeSet<NodeId>,
    pending_recovery: BTreeSet<NodeId>,
    recovery_due: Option<Instant>,
    last_recovery: Option<Instant>,
    ready: VecDeque<NodeId>,
}

impl SyncSchedule {
    /// A hint is not proof of connectivity. Coalesce it, allow interfaces two
    /// seconds to settle, and admit at most one backoff-bypassing wave per 30s.
    /// Preserve failure counts so a failed recovery attempt resumes backoff.
    fn request_recovery(&mut self, now: Instant) {
        let due = self
            .last_recovery
            .map_or(now + Duration::from_secs(2), |last| {
                (last + Duration::from_secs(30)).max(now + Duration::from_secs(2))
            });
        self.recovery_due.get_or_insert(due);
    }

    fn start_ready(&mut self, peers: BTreeSet<NodeId>, saved: bool, now: Instant) -> Vec<NodeId> {
        const MAX_ROUNDS: usize = 4;
        self.retries.retain(|node, _| peers.contains(node));
        self.pending_saves.retain(|node| peers.contains(node));
        self.pending_recovery.retain(|node| peers.contains(node));
        self.ready.retain(|node| peers.contains(node));
        if self.recovery_due.is_some_and(|due| due <= now) {
            self.pending_recovery.extend(&peers);
            self.last_recovery = Some(now);
            self.recovery_due = None;
        }
        if saved {
            self.pending_saves.extend(&peers);
        }

        // A new save can make a peer ready again, behind peers already waiting.
        for node in peers {
            if self.inflight.contains(&node)
                || self.ready.contains(&node)
                || self.retries.get(&node).is_some_and(|(failures, next)| {
                    *next > now
                        && !self.pending_recovery.contains(&node)
                        && (*failures != 0 || !self.pending_saves.contains(&node))
                })
            {
                continue;
            }
            self.ready.push_back(node);
        }
        let count = (MAX_ROUNDS - self.inflight.len()).min(self.ready.len());
        let starting: Vec<_> = self.ready.drain(..count).collect();
        for node in &starting {
            self.inflight.insert(*node);
            // A later save can schedule a follow-up to this fixed-high round.
            self.pending_saves.remove(node);
            self.pending_recovery.remove(node);
        }
        starting
    }

    fn complete(&mut self, node: NodeId, failures: u32, next: Instant) {
        self.inflight.remove(&node);
        self.retries.insert(node, (failures, next));
    }
}

async fn automatic_sync(
    store: SharedStore<SqliteStore>,
    transport: Arc<NativeTransport>,
    seconds: u64,
    mut recovery: watch::Receiver<Option<harmion_net_native::recovery::Reason>>,
) -> Result<()> {
    let base = Duration::from_secs(seconds);
    let mut schedule = SyncSchedule::default();
    let mut local_save = false;
    let mut failures = 0u32;
    let mut rounds = tokio::task::JoinSet::new();
    loop {
        let peers = match store
            .run(|s| {
                s.transaction(|tx| Ok(tx.peers()?.into_iter().map(|peer| peer.node).collect()))
            })
            .await
        {
            Ok(peers) => {
                failures = 0;
                peers
            }
            Err(error) => {
                // A transient SQLite error must not take down the HTTP
                // services. Keep the daemon alive and retry with bounded
                // exponential backoff instead of a hot 5-second loop.
                failures = failures.saturating_add(1);
                let delay =
                    Duration::from_secs(1u64 << failures.min(6)).min(Duration::from_secs(60));
                tracing::warn!(%error, retry_seconds = delay.as_secs(), "peer list unavailable; retrying automatic sync");
                tokio::time::sleep(delay).await;
                continue;
            }
        };
        // Retired/contested peers may exchange signed control evidence only.
        // sync_once checks current membership before any data operation.
        for node in schedule.start_ready(peers, std::mem::take(&mut local_save), Instant::now()) {
            let store = store.clone();
            let transport = transport.clone();
            rounds.spawn(async move {
                (
                    node,
                    sync::sync_once(&store, transport.as_ref(), node).await,
                )
            });
        }
        tokio::select! {
            completed=rounds.join_next(), if !rounds.is_empty() => {
                let (node, result) = completed.ok_or(DaemonError::MissingSyncTask)??;
                let previous = schedule.retries.get(&node).map_or(0, |(failures, _)| *failures);
                let failed = match result {
                    Ok(_) => {
                        if previous != 0 {
                            tracing::info!(peer = %hex(&node.0), "peer reconnected");
                        }
                        0
                    }
                    Err(harmion_net_native::Error::Node(
                        harmion_node::Error::PeerRetired | harmion_node::Error::MembershipConflict,
                    )) => 0,
                    Err(error) => {
                        if previous == 0 {
                            tracing::warn!(peer = %hex(&node.0), %error, "peer unavailable; retrying");
                        }
                        previous.saturating_add(1)
                    }
                };
                let delay = base.saturating_mul(1u32 << failed.min(5)).min(Duration::from_secs(3600));
                let jitter = u16::from_be_bytes(SystemHost.random_bytes()?[..2].try_into()?) as u64 % 1000;
                tracing::debug!(peer = %hex(&node.0), failures = failed, delay_ms = delay.as_millis() + u128::from(jitter), "next sync scheduled");
                schedule.complete(node, failed, Instant::now() + delay + Duration::from_millis(jitter));
            },
            _ = store.local_save_notified() => local_save = true,
            changed = recovery.changed() => {
                if changed.is_err() {
                    return Err(DaemonError::ServiceStopped("recovery monitor").into());
                }
                let reason = *recovery.borrow_and_update();
                tracing::debug!(?reason, "recovery hint; scheduling bounded sync retry");
                schedule.request_recovery(Instant::now());
            },
            _=tokio::time::sleep(base.min(Duration::from_secs(1))) => {},
        }
    }
}
async fn shutdown_signal() -> io::Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! { result = tokio::signal::ctrl_c() => result, _ = terminate.recv() => Ok(()) }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await
    }
}
async fn run(path: &str, options: RunOptions) -> Result<()> {
    let directory = ProtectedDir::open(path)?;
    if directory.path().join("snapshot.json").is_file() {
        return Err("snapshot directory cannot be run; restore it to a new node first".into());
    }
    let _lock = directory.lock()?;
    let mut config: Config = serde_json::from_slice(&directory.read("node.json", 4096)?)?;
    if let Some(address) = options.app_listen {
        config.app_listen = address;
    }
    if let Some(address) = options.oauth_listen {
        config.oauth_listen = address.address();
    }
    if let Some(address) = options.peer_listen {
        config.peer_listen = address;
    }
    if let Some(seconds) = options.sync_seconds {
        config.sync_seconds = seconds;
    }
    if let Some(founder) = options.bootstrap_cluster_root {
        config.founder = founder;
    }
    if let Some(transport) = options.peer_transport {
        config.use_iroh = matches!(transport, PeerTransport::Iroh);
    }
    if let Some(url) = options.relay_url {
        config.relay_urls = Some(vec![url]);
    }
    if let Some(discovery) = options.discovery {
        config.public_lookup = matches!(discovery, Discovery::Public);
    }
    if !config.app_listen.ip().is_loopback()
        || config.peer_listen.ip().is_unspecified()
        || config.sync_seconds > 3600
    {
        return Err(DaemonError::InvalidListenerConfig.into());
    }
    if config
        .oauth_listen
        .is_some_and(|address| address.ip() != std::net::Ipv4Addr::LOCALHOST)
    {
        return Err(DaemonError::InvalidOAuthListener.into());
    }
    let identity = config.identity()?;
    if config.use_iroh
        && !config.public_lookup
        && config
            .relay_urls
            .as_ref()
            .is_none_or(|urls| urls.is_empty())
    {
        return Err(DaemonError::MissingRelay.into());
    }
    let tls = Arc::new(TlsIdentity::from_der(
        directory.read("certificate.der", 65536)?,
        directory.read("private-key.der", 65536)?,
    )?);
    verify_database_files(&directory)?;
    let store = SharedStore::new(SqliteStore::open(
        directory.path().join("node.db"),
        identity,
    )?);
    let app_listener = TcpListener::bind(config.app_listen).await?;
    let oauth_listener = if let Some(address) = config.oauth_listen {
        let tls = browser_tls(&directory)?;
        let listener = TcpListener::bind(address).await?;
        let actual = listener.local_addr()?;
        let origin = oauth_browser::configure(&store, actual)?;
        config.oauth_listen = Some(actual);
        Some((listener, tls, origin))
    } else {
        None
    };
    let peer_listener = if config.use_iroh {
        None
    } else {
        Some(TcpListener::bind(config.peer_listen).await?)
    };
    let membership_seed: [u8; 32] = directory
        .read("iroh.secret", 32)?
        .try_into()
        .map_err(|_| DaemonError::InvalidKeyLength("membership"))?;
    let membership_signer =
        Arc::new(harmion_node::membership_state::MembershipSigner::from_bytes(&membership_seed));
    let iroh_endpoint = if config.use_iroh {
        // A missing key must not silently give an existing Node ID a new identity.
        let secret = directory
            .read("iroh.secret", 32)?
            .try_into()
            .map_err(|_| DaemonError::InvalidKeyLength("iroh"))?;
        Some(iroh_peer::bind(secret, config.relay_urls.clone(), config.public_lookup).await?)
    } else {
        None
    };
    config.app_listen = app_listener.local_addr()?;
    if let Some(listener) = &peer_listener {
        config.peer_listen = listener.local_addr()?;
    }
    let connection = ConnectionInfo {
        version: 1,
        cluster_id: config.cluster_id.clone(),
        node_id: config.node_id.clone(),
        certificate: hex(tls.certificate()),
        endpoint: config.app_listen.to_string(),
        oauth_origin: oauth_listener.as_ref().map(|(_, _, origin)| origin.clone()),
    };
    let mut registration = PeerRegistration {
        version: 3,
        signing_key: Some(hex(&membership_signer.public_key())),
        cluster_root: None,
        cluster_id: config.cluster_id.clone(),
        node_id: config.node_id.clone(),
        binding: match &iroh_endpoint {
            Some(endpoint) => PeerBinding::Iroh {
                endpoint_id: hex(endpoint.id().as_bytes()),
                relay_url: config
                    .relay_urls
                    .as_ref()
                    .and_then(|urls| urls.first())
                    .cloned(),
            },
            None => PeerBinding::Https {
                certificate: hex(tls.certificate()),
                endpoint: config.peer_listen.to_string(),
            },
        },
    };
    let own_member = harmion_protocol::membership::Member {
        node: identity.node,
        signing_key: membership_signer.public_key(),
        binding: match &iroh_endpoint {
            Some(endpoint) => {
                harmion_protocol::membership::Binding::Iroh(*endpoint.id().as_bytes())
            }
            None => harmion_protocol::membership::Binding::Https(tls.certificate().to_vec()),
        },
        endpoint: match &registration.binding {
            PeerBinding::Https { endpoint, .. } => endpoint.clone(),
            PeerBinding::Iroh { relay_url, .. } => relay_url.clone().unwrap_or_default(),
            PeerBinding::SignedHttps { origin } => origin.clone(),
        },
    };
    let signer = membership_signer.clone();
    let founder = config.founder;
    store
        .run(move |s| harmion_node::membership_state::initialize(s, &signer, own_member, founder))
        .await?;
    registration.cluster_root = store
        .run(|s| s.transaction(|tx| Ok(tx.membership_root()?.map(|root| hex(root.bytes())))))
        .await?;
    directory.replace("node.json", &serde_json::to_vec(&config)?)?;
    directory.replace("connection.json", &serde_json::to_vec(&connection)?)?;
    directory.replace("peer.json", &serde_json::to_vec(&registration)?)?;
    let (stop, receiver) = watch::channel(false);
    let mut tasks = tokio::task::JoinSet::new();
    let oauth_origin = if let Some((listener, tls, origin)) = oauth_listener {
        tasks.spawn(oauth_browser::serve(
            listener,
            store.clone(),
            tls,
            receiver.clone(),
        ));
        Some(origin)
    } else {
        None
    };
    let trust_roots = options
        .signed_https_roots
        .iter()
        .map(|path| {
            let bytes = std::fs::read(path).map_err(|_| DaemonError::InvalidTrustRoot)?;
            if bytes.is_empty() || bytes.len() > 65536 {
                return Err(DaemonError::InvalidTrustRoot);
            }
            Ok(bytes)
        })
        .collect::<std::result::Result<Vec<_>, DaemonError>>()?;
    let transport = Arc::new(NativeTransport {
        membership_signer: Some(membership_signer),
        tls: tls.clone(),
        iroh: iroh_endpoint
            .clone()
            .map(|endpoint| iroh_peer::IrohTransport { endpoint }),
        signed_https_roots: trust_roots,
    });
    tasks.spawn(app::serve_with_transport(
        app_listener,
        store.clone(),
        transport.clone(),
        registration.clone(),
        receiver.clone(),
    ));
    if let Some(listener) = peer_listener {
        tasks.spawn(peer::serve(listener, store.clone(), tls, receiver.clone()));
    }
    if let Some(endpoint) = &iroh_endpoint {
        tasks.spawn(iroh_peer::serve(endpoint.clone(), store.clone(), receiver));
    }
    let mut background = tokio::task::JoinSet::new();
    {
        // Maintenance runs detached: a failure must not stop the HTTP services.
        let store = store.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(TOKEN_GC_INTERVAL);
            loop {
                ticker.tick().await;
                let cutoff = SystemHost.now_micros() - TOKEN_RETENTION_MICROS;
                match store.run(move |s| s.gc_tokens(cutoff)).await {
                    Ok(deleted) if deleted > 0 => {
                        tracing::info!(deleted, "reclaimed expired edit tokens");
                    }
                    Ok(_) => {}
                    Err(error) => tracing::warn!(%error, "edit token cleanup failed"),
                }
            }
        });
    }
    if config.sync_seconds != 0 {
        let (recovery, hints) = watch::channel(None);
        let endpoint = iroh_endpoint.clone();
        background.spawn(async move {
            harmion_net_native::recovery::monitor(recovery, endpoint).await;
            Ok(())
        });
        background.spawn(automatic_sync(store, transport, config.sync_seconds, hints));
    }
    println!(
        "{}",
        serde_json::json!({"ready":true,"app_endpoint":config.app_listen.to_string(),"peer":registration,"oauth_origin":oauth_origin})
    );
    io::stdout().flush()?;
    let outcome: Result<()> = tokio::select! {
        signal = shutdown_signal() => signal.map_err(Into::into),
        ended = tasks.join_next() => match ended { Some(Ok(Err(e))) => Err(e.into()), Some(Err(e)) => Err(e.into()), _ => Err(DaemonError::ServiceStopped("HTTP server").into()) },
        ended = background.join_next(), if !background.is_empty() => match ended { Some(Ok(Err(e))) => Err(e), Some(Err(e)) => Err(e.into()), _ => Err(DaemonError::ServiceStopped("automatic sync").into()) },
    };
    let _ = stop.send(true);
    background.abort_all();
    while background.join_next().await.is_some() {}
    while let Some(task) = tasks.join_next().await {
        task??;
    }
    if let Some(endpoint) = iroh_endpoint {
        endpoint.close().await;
    }
    outcome
}
fn enroll_owner(path: &str, key_path: &str, label: &str) -> Result<()> {
    let directory = ProtectedDir::open(path)?;
    // Hold the daemon lock: enrollment writes the DB directly and must not race
    // a running node.
    let _lock = directory.lock()?;
    let config: Config = serde_json::from_slice(&directory.read("node.json", 4096)?)?;
    let identity = config.identity()?;
    verify_database_files(&directory)?;
    let mut store = SqliteStore::open(directory.path().join("node.db"), identity)?;
    let secret: [u8; 32] = directory
        .read("admin.secret", 32)?
        .try_into()
        .map_err(|_| DaemonError::InvalidKeyLength("admin"))?;
    let credential = harmion_node::access::Credential::from_bytes(secret);
    let now = SystemHost.now_micros();
    let grants = harmion_node::access::permissions(&mut store, &credential, now)?;
    if !grants.is_administrator() {
        return Err(harmion_node::Error::Forbidden.into());
    }
    let public_key = std::fs::read_to_string(key_path)?;
    if public_key.len() > 16 * 1024 {
        return Err("SSH public key is too large".into());
    }
    let mut host = SystemHost;
    let id = host.random_bytes()?;
    let recovery = host.random_bytes()?;
    let now = host.now_micros();
    let enrolled = store.transaction(|tx| {
        harmion_node::owner::enroll_initial(tx, &public_key, label, &recovery, id, now)
    })?;
    eprintln!("harmiond: store this one-time recovery code offline; it will not be shown again");
    println!(
        "{}",
        serde_json::json!({
            "enrolled": true,
            "fingerprint": enrolled.fingerprint,
            "label": enrolled.label,
            "recovery_code": hex(&recovery),
        })
    );
    Ok(())
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "harmiond=info".into()),
        )
        .with_writer(io::stderr)
        .with_ansi(false)
        .init();
    let result = match cli.command {
        Command::Init { dir, cluster } => initialize(&dir, cluster.as_deref(), None),
        Command::Restore {
            backup_dir,
            new_dir,
        } => restore(&backup_dir, &new_dir),
        Command::ResetPeerV2 { dir } => reset_peer_v2(&dir),
        Command::Snapshot { dir, out } => snapshot(&dir, &out),
        Command::OauthProvision { dir, replace } => provision_oauth(&dir, replace),
        Command::OauthRoot { dir } => ProtectedDir::open(&dir)
            .map_err(Into::into)
            .and_then(|directory| browser_tls(&directory))
            .map(|tls| print!("{}", tls.root_pem())),
        Command::OwnerEnroll {
            dir,
            ssh_key,
            label,
        } => enroll_owner(&dir, &ssh_key, &label),
        Command::Run { dir, options } => run(&dir, options).await,
    };
    if let Err(error) = result {
        tracing::error!(%error, "harmiond failed");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waiting_peer_gets_next_slot_during_continuous_saves() {
        let [a, b, c, d, e] = [1, 2, 3, 4, 5].map(|id| NodeId([id; 32]));
        let peers = BTreeSet::from([a, b, c, d, e]);
        let now = Instant::now();
        let periodic_due = now + Duration::from_secs(120);
        let mut schedule = SyncSchedule::default();

        assert_eq!(
            schedule.start_ready(peers.clone(), false, now),
            [a, b, c, d]
        );
        assert!(schedule.start_ready(peers.clone(), true, now).is_empty());
        schedule.complete(a, 0, periodic_due);
        assert_eq!(schedule.start_ready(peers.clone(), true, now), [e]);

        schedule.complete(b, 0, periodic_due);
        assert_eq!(schedule.start_ready(peers, true, now), [a]);
    }

    #[test]
    fn recovery_bypasses_backoff_once_after_settling_and_preserves_failures() {
        let node = NodeId([1; 32]);
        let peers = BTreeSet::from([node]);
        let now = Instant::now();
        let mut schedule = SyncSchedule::default();
        assert_eq!(schedule.start_ready(peers.clone(), false, now), [node]);
        schedule.complete(node, 4, now + Duration::from_secs(3600));
        schedule.request_recovery(now);
        assert!(
            schedule
                .start_ready(peers.clone(), false, now + Duration::from_secs(1))
                .is_empty()
        );
        assert_eq!(
            schedule.start_ready(peers.clone(), false, now + Duration::from_secs(2)),
            [node]
        );
        assert_eq!(schedule.retries[&node].0, 4);
        schedule.complete(node, 5, now + Duration::from_secs(3600));
        assert!(
            schedule
                .start_ready(peers, true, now + Duration::from_secs(3))
                .is_empty()
        );
    }

    #[test]
    fn recovery_during_an_inflight_round_is_retained_for_one_followup() {
        let node = NodeId([1; 32]);
        let peers = BTreeSet::from([node]);
        let now = Instant::now();
        let mut schedule = SyncSchedule::default();
        assert_eq!(schedule.start_ready(peers.clone(), false, now), [node]);
        schedule.request_recovery(now);
        assert!(
            schedule
                .start_ready(peers.clone(), false, now + Duration::from_secs(2))
                .is_empty()
        );
        schedule.complete(node, 3, now + Duration::from_secs(3600));
        assert_eq!(
            schedule.start_ready(peers.clone(), false, now + Duration::from_secs(3)),
            [node]
        );
        schedule.complete(node, 4, now + Duration::from_secs(3600));
        assert!(
            schedule
                .start_ready(peers, false, now + Duration::from_secs(4))
                .is_empty()
        );
    }

    #[test]
    fn recovery_bursts_are_coalesced_but_a_later_reconnect_is_not_lost() {
        let node = NodeId([1; 32]);
        let peers = BTreeSet::from([node]);
        let now = Instant::now();
        let mut schedule = SyncSchedule::default();
        schedule.start_ready(peers.clone(), false, now);
        schedule.complete(node, 1, now + Duration::from_secs(3600));
        schedule.request_recovery(now);
        schedule.request_recovery(now + Duration::from_secs(1));
        assert_eq!(
            schedule.start_ready(peers.clone(), false, now + Duration::from_secs(2)),
            [node]
        );
        schedule.complete(node, 2, now + Duration::from_secs(3600));
        schedule.request_recovery(now + Duration::from_secs(3));
        schedule.request_recovery(now + Duration::from_secs(20));
        assert!(
            schedule
                .start_ready(peers.clone(), false, now + Duration::from_secs(31))
                .is_empty()
        );
        assert_eq!(
            schedule.start_ready(peers, false, now + Duration::from_secs(32)),
            [node]
        );
    }

    #[test]
    fn recovery_preserves_four_slot_limit_and_existing_wait_order() {
        let [a, b, c, d, e] = [1, 2, 3, 4, 5].map(|id| NodeId([id; 32]));
        let peers = BTreeSet::from([a, b, c, d, e]);
        let now = Instant::now();
        let mut schedule = SyncSchedule::default();
        assert_eq!(
            schedule.start_ready(peers.clone(), false, now),
            [a, b, c, d]
        );
        schedule.request_recovery(now);
        assert!(
            schedule
                .start_ready(peers.clone(), false, now + Duration::from_secs(2))
                .is_empty()
        );
        schedule.complete(a, 3, now + Duration::from_secs(3600));
        assert_eq!(
            schedule.start_ready(peers.clone(), false, now + Duration::from_secs(3)),
            [e]
        );
        schedule.complete(b, 2, now + Duration::from_secs(3600));
        assert_eq!(
            schedule.start_ready(peers, false, now + Duration::from_secs(4)),
            [a]
        );
    }

    #[test]
    fn pending_save_does_not_bypass_failure_backoff() {
        let node = NodeId([1; 32]);
        let peers = BTreeSet::from([node]);
        let now = Instant::now();
        let retry_due = now + Duration::from_secs(60);
        let mut schedule = SyncSchedule::default();

        assert_eq!(schedule.start_ready(peers.clone(), false, now), [node]);
        schedule.complete(node, 1, retry_due);
        assert!(schedule.start_ready(peers.clone(), true, now).is_empty());
        assert_eq!(schedule.start_ready(peers, false, retry_due), [node]);
    }
}
