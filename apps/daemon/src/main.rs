use harmion_domain::{ClusterId, NodeId};
use harmion_local::ProtectedDir;
use harmion_net_native::{
    NativeTransport, SharedStore, SystemHost, app, iroh_peer, peer, sync, tls::TlsIdentity,
};
use harmion_node::{
    application::{self, Host},
    storage::{Identity, Store},
};
use harmion_protocol::{ConnectionInfo, LogEpoch, PeerBinding, PeerRegistration, hex, unhex};
use harmion_store_sqlite::SqliteStore;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
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
fn initialize(path: &str, cluster: Option<&str>) -> Result<()> {
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

async fn periodic(
    store: SharedStore<SqliteStore>,
    transport: Arc<NativeTransport>,
    seconds: u64,
) -> Result<()> {
    let base = Duration::from_secs(seconds);
    let mut retries: BTreeMap<NodeId, (u32, Instant)> = BTreeMap::new();
    let mut inflight = BTreeSet::new();
    let mut rounds = tokio::task::JoinSet::new();
    loop {
        let peers = store.run(|s| s.transaction(|tx| tx.peers())).await?;
        retries.retain(|id, _| peers.iter().any(|p| p.node == *id));
        // Retired/contested peers may exchange signed control evidence only.
        // sync_once checks current membership before any data operation.
        for peer in peers {
            if rounds.len() >= 4 {
                break;
            }
            if inflight.contains(&peer.node)
                || retries
                    .get(&peer.node)
                    .is_some_and(|(_, next)| *next > Instant::now())
            {
                continue;
            }
            inflight.insert(peer.node);
            let store = store.clone();
            let transport = transport.clone();
            rounds.spawn(async move {
                (
                    peer.node,
                    sync::sync_once(&store, transport.as_ref(), peer.node).await,
                )
            });
        }
        tokio::select! {
            completed=rounds.join_next(), if !rounds.is_empty() => {
                let (node,result)=completed.ok_or("sync task missing")??;
                inflight.remove(&node);
                let previous=retries.get(&node).map_or(0,|(failures,_)|*failures);
                let failed=match result {
                    Ok(_) => {if previous!=0 {eprintln!("peer {} reconnected",hex(&node.0));} 0},
                    Err(harmion_net_native::Error::Node(harmion_node::Error::PeerRetired | harmion_node::Error::MembershipConflict)) => 0,
                    Err(_) => {if previous==0 {eprintln!("peer {} unavailable; retrying",hex(&node.0));} previous.saturating_add(1)},
                };
                let delay=base.saturating_mul(1u32<<failed.min(5)).min(Duration::from_secs(3600));
                let jitter=u16::from_be_bytes(SystemHost.random_bytes()?[..2].try_into()?) as u64 % 1000;
                retries.insert(node,(failed,Instant::now()+delay+Duration::from_millis(jitter)));
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
    directory.verify_file("node.db")?;
    for sidecar in ["node.db-wal", "node.db-shm", "node.db-journal"] {
        match directory.verify_file(sidecar) {
            Ok(()) => (),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
    }
    let store = SharedStore::new(SqliteStore::open(
        directory.path().join("node.db"),
        identity,
    )?);
    let app_listener = TcpListener::bind(config.app_listen).await?;
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
        background.spawn(periodic(store, transport, config.sync_seconds));
    }
    println!(
        "{}",
        serde_json::json!({"ready":true,"app_endpoint":config.app_listen.to_string(),"peer":registration})
    );
    io::stdout().flush()?;
    let outcome: Result<()> = tokio::select! {
        signal = shutdown_signal() => signal.map_err(Into::into),
        ended = tasks.join_next() => match ended { Some(Ok(Err(e))) => Err(e.into()), _ => Err("HTTP server stopped unexpectedly".into()) },
        ended = background.join_next(), if !background.is_empty() => match ended { Some(Ok(Err(e))) => Err(e), _ => Err("periodic sync stopped unexpectedly".into()) },
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
        [command, path] if command == "init" => initialize(path, None),
        [command, path, flag, cluster] if command == "init" && flag == "--cluster" => initialize(path, Some(cluster)),
        [command, path, options @ ..] if command == "run" => run(path, options).await,
        _ => Err("usage: harmiond init DIR [--cluster HEX] | run DIR [--app-listen IP:PORT] [--peer-listen IP:PORT] [--sync-seconds N] [--peer-transport iroh|https] [--relay-url URL] [--discovery public|none] [--bootstrap-cluster-root true|false]".into()),
    };
    if let Err(error) = result {
        eprintln!("harmiond: {error}");
        std::process::exit(1);
    }
}
