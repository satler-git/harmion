use harmion_domain::{ClusterId, NodeId};
use harmion_local::ProtectedDir;
use harmion_net_native::{
    NativeTransport, SharedStore, SystemHost, app, iroh_peer, oauth_browser, peer, sync,
    tls::{BrowserTls, TlsIdentity},
};
use harmion_node::{
    application::{self, Host},
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
            return Err("unsupported node config".into());
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
    let _lock = backup.lock()?;
    let config: Config = serde_json::from_slice(&backup.read("node.json", 4096)?)?;
    verify_database_files(&backup)?;
    initialize(
        destination,
        Some(&config.cluster_id),
        Some((&backup, config.identity()?)),
    )
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
    let tls = BrowserTls::generate(&hex(&identity.node.0[..4]))?;
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
    ready: VecDeque<NodeId>,
}

impl SyncSchedule {
    fn start_ready(&mut self, peers: BTreeSet<NodeId>, saved: bool, now: Instant) -> Vec<NodeId> {
        const MAX_ROUNDS: usize = 4;
        self.retries.retain(|node, _| peers.contains(node));
        self.pending_saves.retain(|node| peers.contains(node));
        self.ready.retain(|node| peers.contains(node));
        if saved {
            self.pending_saves.extend(&peers);
        }

        // A new save can make a peer ready again, behind peers already waiting.
        for node in peers {
            if self.inflight.contains(&node)
                || self.ready.contains(&node)
                || self.retries.get(&node).is_some_and(|(failures, next)| {
                    *next > now && (*failures != 0 || !self.pending_saves.contains(&node))
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
) -> Result<()> {
    let base = Duration::from_secs(seconds);
    let mut schedule = SyncSchedule::default();
    let mut local_save = false;
    let mut rounds = tokio::task::JoinSet::new();
    loop {
        let peers = store
            .run(|s| {
                s.transaction(|tx| Ok(tx.peers()?.into_iter().map(|peer| peer.node).collect()))
            })
            .await?;
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
                let (node,result)=completed.ok_or("sync task missing")??;
                let previous=schedule.retries.get(&node).map_or(0,|(failures,_)|*failures);
                let failed=match result {
                    Ok(_) => {if previous!=0 {eprintln!("peer {} reconnected",hex(&node.0));} 0},
                    Err(harmion_net_native::Error::Node(harmion_node::Error::PeerRetired | harmion_node::Error::MembershipConflict)) => 0,
                    Err(_) => {if previous==0 {eprintln!("peer {} unavailable; retrying",hex(&node.0));} previous.saturating_add(1)},
                };
                let delay=base.saturating_mul(1u32<<failed.min(5)).min(Duration::from_secs(3600));
                let jitter=u16::from_be_bytes(SystemHost.random_bytes()?[..2].try_into()?) as u64 % 1000;
                schedule.complete(node,failed,Instant::now()+delay+Duration::from_millis(jitter));
            },
            _ = store.local_save_notified() => local_save = true,
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
async fn run(path: &str, options: &[String]) -> Result<()> {
    let directory = ProtectedDir::open(path)?;
    let _lock = directory.lock()?;
    let mut config: Config = serde_json::from_slice(&directory.read("node.json", 4096)?)?;
    if !options.len().is_multiple_of(2) {
        return Err("run options need values".into());
    }
    for pair in options.as_chunks::<2>().0 {
        match pair[0].as_str() {
            "--app-listen" => config.app_listen = pair[1].parse()?,
            "--oauth-listen" => {
                config.oauth_listen = if pair[1] == "off" {
                    None
                } else {
                    Some(pair[1].parse()?)
                };
            }
            "--peer-listen" => config.peer_listen = pair[1].parse()?,
            "--sync-seconds" => config.sync_seconds = pair[1].parse()?,
            "--bootstrap-cluster-root" => config.founder = pair[1].parse()?,
            "--peer-transport" => {
                config.use_iroh = match pair[1].as_str() {
                    "iroh" => true,
                    "https" => false,
                    _ => return Err("expected iroh or https transport".into()),
                }
            }
            "--relay-url" => config.relay_urls = Some(vec![pair[1].clone()]),
            "--discovery" => {
                config.public_lookup = match pair[1].as_str() {
                    "public" => true,
                    "none" => false,
                    _ => return Err("discovery must be public or none".into()),
                }
            }
            _ => return Err("unknown run option".into()),
        }
    }
    if !config.app_listen.ip().is_loopback()
        || config.peer_listen.ip().is_unspecified()
        || config.sync_seconds > 3600
    {
        return Err(
            "App must be loopback; Peer needs a concrete IP; sync seconds must be 0..3600".into(),
        );
    }
    if config
        .oauth_listen
        .is_some_and(|address| address.ip() != std::net::Ipv4Addr::LOCALHOST)
    {
        return Err("OAuth must listen on 127.0.0.1".into());
    }
    let identity = config.identity()?;
    if config.use_iroh
        && !config.public_lookup
        && config
            .relay_urls
            .as_ref()
            .is_none_or(|urls| urls.is_empty())
    {
        return Err("discovery none requires at least one explicit relay URL".into());
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
        .map_err(|_| "invalid membership key length")?;
    let membership_signer =
        Arc::new(harmion_node::membership_state::MembershipSigner::from_bytes(&membership_seed));
    let iroh_endpoint = if config.use_iroh {
        // A missing key must not silently give an existing Node ID a new identity.
        let secret = directory
            .read("iroh.secret", 32)?
            .try_into()
            .map_err(|_| "invalid iroh key length")?;
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
    let transport = Arc::new(NativeTransport {
        membership_signer: Some(membership_signer),
        tls: tls.clone(),
        iroh: iroh_endpoint
            .clone()
            .map(|endpoint| iroh_peer::IrohTransport { endpoint }),
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
    if config.sync_seconds != 0 {
        background.spawn(automatic_sync(store, transport, config.sync_seconds));
    }
    println!(
        "{}",
        serde_json::json!({"ready":true,"app_endpoint":config.app_listen.to_string(),"peer":registration,"oauth_origin":oauth_origin})
    );
    io::stdout().flush()?;
    let outcome: Result<()> = tokio::select! {
        signal = shutdown_signal() => signal.map_err(Into::into),
        ended = tasks.join_next() => match ended { Some(Ok(Err(e))) => Err(e.into()), _ => Err("HTTP server stopped unexpectedly".into()) },
        ended = background.join_next(), if !background.is_empty() => match ended { Some(Ok(Err(e))) => Err(e), _ => Err("automatic sync stopped unexpectedly".into()) },
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
#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [command, path] if command == "init" => initialize(path, None, None),
        [command, path, flag, cluster] if command == "init" && flag == "--cluster" => initialize(path, Some(cluster), None),
        [command, backup, destination] if command == "restore" => restore(backup, destination),
        [command, path] if command == "oauth-provision" => provision_oauth(path, false),
        [command, path, flag] if command == "oauth-provision" && flag == "--replace" => provision_oauth(path, true),
        [command, path] if command == "oauth-root" => ProtectedDir::open(path)
            .map_err(Into::into).and_then(|directory| browser_tls(&directory)).map(|tls| print!("{}", tls.root_pem())),
        [command, path, options @ ..] if command == "run" => run(path, options).await,
        _ => Err("usage: harmiond init DIR [--cluster HEX] | restore BACKUP_DIR NEW_DIR | oauth-provision DIR [--replace] | oauth-root DIR | run DIR [--app-listen IP:PORT] [--oauth-listen 127.0.0.1:PORT|off] [--peer-listen IP:PORT] [--sync-seconds N] [--peer-transport iroh|https] [--relay-url URL] [--discovery public|none] [--bootstrap-cluster-root true|false]".into()),
    };
    if let Err(error) = result {
        eprintln!("harmiond: {error}");
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
