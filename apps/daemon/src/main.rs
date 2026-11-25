mod cli;

use clap::Parser;
use cli::{Cli, Command, Discovery, PeerTransport, RunOptions};
use harmion_local::ProtectedDir;
use harmion_net_native::SystemHost;
use harmion_node::{application::Host, https_peer::ChallengeStore, storage::Store};
use harmion_protocol::hex;
use harmion_runtime::{
    ConfigOverrides, InitializeOptions, NodeRuntime, RuntimeOptions, initialize, restore, storage,
};
use harmion_store_sqlite::SqliteStore;
use std::io::{self, Write};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn initialize_node(path: &str, cluster: Option<&str>) -> Result<()> {
    let node = initialize(
        path,
        &InitializeOptions {
            cluster_id: cluster.map(str::to_owned),
            ..InitializeOptions::default()
        },
    )?;
    println!(
        "{}",
        serde_json::json!({
            "initialized": true,
            "cluster_id": node.config.cluster_id,
            "node_id": node.config.node_id,
        })
    );
    Ok(())
}

fn restore_node(backup: &str, destination: &str) -> Result<()> {
    let node = restore(backup, destination, &InitializeOptions::default())?;
    println!(
        "{}",
        serde_json::json!({
            "initialized": true,
            "restored": true,
            "cluster_id": node.config.cluster_id,
            "node_id": node.config.node_id,
        })
    );
    Ok(())
}

fn reset_peer_v2(path: &str) -> Result<()> {
    let directory = storage::open_node(path)?;
    let _lock = directory.lock()?;
    let config = storage::read_config(&directory)?;
    let identity = config.identity()?;
    storage::verify_database_files(&directory)?;
    let mut store = SqliteStore::open(directory.path().join(storage::DATABASE_FILE), identity)?;
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
    let directory = storage::open_node(path)?;
    let config = storage::read_config(&directory)?;
    config.identity()?;
    // A manual daemon run may leave -wal/-shm at 0644. Snapshot reads through
    // SQLite (VACUUM INTO) and never copies sidecars, so only the main DB file
    // needs the owner-only check here.
    directory.verify_file(storage::DATABASE_FILE)?;
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
        &directory.path().join(storage::DATABASE_FILE),
        &output.path().join(storage::DATABASE_FILE),
    )?;
    for name in [
        storage::CONFIG_FILE,
        storage::ADMIN_SECRET_FILE,
        storage::CERTIFICATE_FILE,
        storage::PRIVATE_KEY_FILE,
        storage::IROH_SECRET_FILE,
        storage::CONNECTION_FILE,
        storage::PEER_FILE,
        storage::OAUTH_IDENTITY_FILE,
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

fn provision_oauth(path: &str, replace: bool) -> Result<()> {
    let directory = storage::open_node(path)?;
    let _lock = directory.lock()?;
    let config = storage::read_config(&directory)?;
    config.identity()?;
    if replace {
        let label = format!("OAuth {}", &config.node_id[..8.min(config.node_id.len())]);
        let tls = harmion_net_native::tls::BrowserTls::generate(&label)?;
        storage::write_oauth_identity(&directory, &tls, true)?;
        print!("{}", tls.root_pem());
    } else {
        let (_tls, pem) = storage::oauth_tls(&directory)?;
        print!("{pem}");
    }
    Ok(())
}

fn print_oauth_root(path: &str) -> Result<()> {
    let directory = storage::open_node(path)?;
    let (_tls, pem) = storage::oauth_tls(&directory)?;
    print!("{pem}");
    Ok(())
}

fn enroll_owner(path: &str, key_path: &str, label: &str) -> Result<()> {
    let directory = storage::open_node(path)?;
    // Hold the daemon lock: enrollment writes the DB directly and must not race
    // a running node.
    let _lock = directory.lock()?;
    let mut store = {
        let config = storage::read_config(&directory)?;
        let identity = config.identity()?;
        storage::verify_database_files(&directory)?;
        SqliteStore::open(directory.path().join(storage::DATABASE_FILE), identity)?
    };
    let secret = storage::admin_secret(&directory)?;
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

fn read_trust_root(path: &std::path::Path) -> Result<Vec<u8>> {
    let bytes = std::fs::read(path)?;
    if bytes.is_empty() || bytes.len() > 65536 {
        return Err(harmion_runtime::Error::InvalidTrustRoot.into());
    }
    Ok(bytes)
}

fn run_options(options: &RunOptions) -> Result<RuntimeOptions> {
    let overrides = ConfigOverrides {
        app_listen: options.app_listen,
        oauth_listen: options.oauth_listen.map(|listen| listen.address()),
        peer_listen: options.peer_listen,
        sync_seconds: options.sync_seconds,
        use_iroh: options
            .peer_transport
            .map(|transport| matches!(transport, PeerTransport::Iroh)),
        relay_urls: options.relay_url.clone().map(|url| vec![url]),
        public_lookup: options
            .discovery
            .map(|discovery| matches!(discovery, Discovery::Public)),
        founder: options.bootstrap_cluster_root,
    };
    let signed_https_roots = options
        .signed_https_roots
        .iter()
        .map(|path| read_trust_root(path))
        .collect::<Result<Vec<_>>>()?;
    Ok(RuntimeOptions {
        overrides,
        signed_https_roots,
        sync_gate: None,
        sync_blocked: None,
    })
}

async fn run(path: &str, options: RunOptions) -> Result<()> {
    let runtime = NodeRuntime::start(path, run_options(&options)?).await?;
    let ready = runtime.ready();
    println!(
        "{}",
        serde_json::json!({
            "ready": true,
            "app_endpoint": ready.app_endpoint,
            "peer": ready.peer,
            "oauth_origin": ready.oauth_origin,
        })
    );
    io::stdout().flush()?;
    // The runtime owns listeners, the scheduler and maintenance. The daemon only
    // decides when to stop: a signal, or a listener that ended unexpectedly.
    let outcome: Result<()> = tokio::select! {
        signal = shutdown_signal() => signal.map_err(Into::into),
        failure = runtime.failure() => Err(failure.into()),
    };
    let shutdown = runtime.shutdown().await;
    match outcome {
        Err(error) => Err(error),
        Ok(()) => shutdown.map_err(Into::into),
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
        Command::Init { dir, cluster } => initialize_node(&dir, cluster.as_deref()),
        Command::Restore {
            backup_dir,
            new_dir,
        } => restore_node(&backup_dir, &new_dir),
        Command::ResetPeerV2 { dir } => reset_peer_v2(&dir),
        Command::Snapshot { dir, out } => snapshot(&dir, &out),
        Command::OauthProvision { dir, replace } => provision_oauth(&dir, replace),
        Command::OauthRoot { dir } => print_oauth_root(&dir),
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
