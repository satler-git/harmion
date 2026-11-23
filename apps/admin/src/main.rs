mod api;
mod approval;
mod server;
mod transfer;

use approval::{ApprovalMode, Confirmer};
use clap::{Parser, Subcommand};
use harmion_client::{Client, ProtectedDir, SshSigner, protocol::unhex};
use harmion_local_web::BrowserTls;
use serde::{Deserialize, Serialize};
use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::net::TcpListener;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
const IDENTITY_FILE: &str = "web-tls.json";

#[derive(Parser)]
#[command(version, about = "Harmion browser administration")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Provision a browser TLS identity.
    Provision {
        /// State directory. Defaults to ~/.local/share/harmion/admin-ui.
        #[arg(long = "ui-dir", value_name = "DIR")]
        ui_dir: Option<String>,
    },
    /// Print the browser trust root as PEM.
    Root {
        /// State directory. Defaults to ~/.local/share/harmion/admin-ui.
        #[arg(long = "ui-dir", value_name = "DIR")]
        ui_dir: Option<String>,
    },
    /// Serve the administration UI on loopback HTTPS.
    Run {
        node_dir: String,
        content_id: Option<String>,
        /// State directory. Defaults to ~/.local/share/harmion/admin-ui.
        #[arg(long = "ui-dir", value_name = "DIR")]
        ui_dir: Option<String>,
        /// R2 approval source. Terminal is the safe default. Session skips the
        /// terminal prompt and relies on the browser session (less secure).
        #[arg(long, value_enum, default_value = "terminal")]
        approval: ApprovalMode,
        /// OpenSSH public key whose private half is held by ssh-agent, or a
        /// private key file. Defaults to the HARMION_OWNER_SSH_KEY environment
        /// variable or ~/.ssh/id_ed25519.pub.
        #[arg(long, value_name = "PATH")]
        owner_key: Option<String>,
    },
}

#[derive(Serialize, Deserialize)]
struct Identity {
    root: Vec<u8>,
    leaf: Vec<u8>,
    key: Vec<u8>,
}

fn provision(path: &Path) -> Result<()> {
    let state = ProtectedDir::create(path)?;
    let identity = BrowserTls::generate("Admin")?;
    let pem = identity.root_pem();
    state.write_new(
        IDENTITY_FILE,
        &serde_json::to_vec(&Identity {
            root: identity.root_der,
            leaf: identity.leaf_der,
            key: identity.private_key_der,
        })?,
    )?;
    print!("{pem}");
    Ok(())
}

fn identity(state: &ProtectedDir) -> Result<BrowserTls> {
    let identity: Identity = serde_json::from_slice(&state.read(IDENTITY_FILE, 65536)?)?;
    Ok(BrowserTls {
        root_der: identity.root,
        leaf_der: identity.leaf,
        private_key_der: identity.key,
    })
}

async fn run(
    node: &Path,
    state: &Path,
    content: Option<&str>,
    approval: ApprovalMode,
    owner_key: Option<&str>,
) -> Result<()> {
    if let Some(content) = content {
        unhex::<32>(content)?;
    }
    tracing::info!(
        node_dir = %node.display(),
        ui_dir = %state.display(),
        approval_mode = ?approval,
        owner_key_provided = owner_key.is_some(),
        content_requested = content.is_some(),
        "admin server starting"
    );
    let state = ProtectedDir::open(state)?;
    let _lock = state.lock()?;
    let config = identity(&state)?.server_config()?;
    let node = ProtectedDir::open(node)?;
    let client = Client::from_directory(node.path())?;
    let permissions = client.call("permissions", &[]).await?;
    if permissions["read"]["type"] != "all"
        || permissions["write"]["type"] != "all"
        || permissions["manage_apps"] != true
        || permissions["manage_nodes"] != true
    {
        tracing::warn!(
            reason = "insufficient_node_permissions",
            required = "read:all,write:all,manage_apps,manage_nodes",
            "node owner permissions required"
        );
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "node owner permissions required",
        )
        .into());
    }
    let owner_enforced = match client.owner_status().await {
        Ok(status) => status["enforce"] == true,
        Err(error) => {
            tracing::warn!(%error, "owner status unavailable; assuming enforcement off");
            false
        }
    };
    let signer = owner_key
        .map(PathBuf::from)
        .map(SshSigner::new)
        .or_else(|| {
            std::env::var_os("HARMION_OWNER_SSH_KEY")
                .map(PathBuf::from)
                .map(SshSigner::new)
        })
        .or_else(harmion_client::ssh_agent_signer)
        .or_else(default_owner_key);
    tracing::info!(
        owner_enforced,
        signer_configured = signer.is_some(),
        "owner enforcement configured"
    );
    if owner_enforced && signer.is_none() {
        tracing::warn!(
            owner_enforced = true,
            hint = "use --owner-key, HARMION_OWNER_SSH_KEY, or ssh-agent",
            "owner enforcement is on but no owner signer is configured"
        );
    }
    let confirmer = Confirmer::start(approval);
    if let Some(reason) = confirmer.unavailable() {
        tracing::warn!(
            approval_mode = ?approval,
            reason,
            "R2 approval channel unavailable"
        );
    }
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let (server, link) = server::State::new(
        client,
        node,
        listener.local_addr()?,
        content,
        signer,
        confirmer,
        owner_enforced,
    )?;
    tracing::info!(
        bootstrap_ttl_secs = 120,
        session_ttl_secs = 30 * 60,
        session_idle_secs = 10 * 60,
        "one-time launch link issued"
    );
    println!("{}", serde_json::json!({"url":link}));
    tracing::debug!("launch URL written to stdout");
    server::run(listener, Arc::new(server), config).await?;
    Ok(())
}

fn default_owner_key() -> Option<SshSigner> {
    let home = std::env::var_os("HOME")?;
    let path = PathBuf::from(home).join(".ssh").join("id_ed25519.pub");
    path.is_file().then(|| SshSigner::new(path))
}

fn resolve_ui_dir(value: Option<&str>) -> Result<PathBuf> {
    if let Some(value) = value {
        return Ok(PathBuf::from(value));
    }
    let home = std::env::var_os("HOME")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
    Ok(PathBuf::from(home)
        .join(".local/share/harmion")
        .join("admin-ui"))
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "harmion_admin=info".into()),
        )
        .with_writer(io::stderr)
        .with_ansi(false)
        .init();
    let result = match Cli::parse().command {
        Command::Provision { ui_dir } => {
            resolve_ui_dir(ui_dir.as_deref()).and_then(|dir| provision(&dir))
        }
        Command::Root { ui_dir } => resolve_ui_dir(ui_dir.as_deref())
            .and_then(|dir| ProtectedDir::open(&dir).map_err(Into::into))
            .and_then(|state| identity(&state))
            .map(|tls| print!("{}", tls.root_pem())),
        Command::Run {
            node_dir,
            content_id,
            ui_dir,
            approval,
            owner_key,
        } => match harmion_local::resolve_node(&node_dir) {
            Ok(node) => match resolve_ui_dir(ui_dir.as_deref()) {
                Ok(ui_dir) => {
                    run(
                        &node,
                        &ui_dir,
                        content_id.as_deref(),
                        approval,
                        owner_key.as_deref(),
                    )
                    .await
                }
                Err(error) => Err(error),
            },
            Err(error) => Err(error.into()),
        },
    };
    if let Err(error) = result {
        tracing::error!(%error, "harmion-admin failed");
        std::process::exit(1);
    }
}
