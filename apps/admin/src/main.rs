mod api;
mod server;
mod transfer;

use clap::{Parser, Subcommand};
use harmion_client::{Client, ProtectedDir, protocol::unhex};
use harmion_local_web::BrowserTls;
use serde::{Deserialize, Serialize};
use std::{io, sync::Arc};
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
    Provision { ui_dir: String },
    /// Print the browser trust root as PEM.
    Root { ui_dir: String },
    /// Serve the administration UI on loopback HTTPS.
    Run {
        node_dir: String,
        ui_dir: String,
        content_id: Option<String>,
    },
}

#[derive(Serialize, Deserialize)]
struct Identity {
    root: Vec<u8>,
    leaf: Vec<u8>,
    key: Vec<u8>,
}

fn provision(path: &str) -> Result<()> {
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

async fn run(node: &str, state: &str, content: Option<&str>) -> Result<()> {
    if let Some(content) = content {
        unhex::<32>(content)?;
    }
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
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "node owner permissions required",
        )
        .into());
    }
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let (server, link) = server::State::new(client, node, listener.local_addr()?, content)?;
    println!("{}", serde_json::json!({"url":link}));
    server::run(listener, Arc::new(server), config).await?;
    Ok(())
}

#[tokio::main]
async fn main() {
    let result = match Cli::parse().command {
        Command::Provision { ui_dir } => provision(&ui_dir),
        Command::Root { ui_dir } => ProtectedDir::open(&ui_dir)
            .map_err(Into::into)
            .and_then(|state| identity(&state))
            .map(|tls| print!("{}", tls.root_pem())),
        Command::Run {
            node_dir,
            ui_dir,
            content_id,
        } => run(&node_dir, &ui_dir, content_id.as_deref()).await,
    };
    if let Err(error) = result {
        eprintln!("harmion-admin: {error}");
        std::process::exit(1);
    }
}
