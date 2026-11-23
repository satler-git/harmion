//! Wiki local-first JSON-RPC server.
//!
//! One JSON object per newline over a Unix socket. The M0 methods (`list`,
//! `open`, `workspace`, `search`, `save`) keep their request/response shapes;
//! M1/M3 adds persistence and the `status`, `create_page`, `create_snippet`,
//! `convert_fence`, `delete`, `transclude`, `export` and `conflicts` methods.

mod auth;
mod backend;
mod error;
mod export;
mod import;
mod model;
mod provider;
mod search;
mod slug;
mod store;
mod transclude;

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

use clap::{Parser, Subcommand};
use harmion_client::Client;
use harmion_client::protocol::GrantSpec;
use harmion_client::protocol::connect::{
    ConnectionCompleteRequest, ConnectionOffer, ConnectionStatusRequest,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

use error::{
    RpcError, RpcResult, optional_bool, optional_str, optional_str_array, optional_u64,
    required_str, required_u64,
};
use store::{FenceSelection, State};

const SOCKET_DEFAULT: &str = "/tmp/harmion-wiki-m0.sock";

#[derive(Debug, Parser)]
#[command(name = "wiki-server", about = "Wiki local-first JSON-RPC server")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Serve the Unix-socket JSON-RPC endpoint.
    Serve {
        #[arg(long, default_value = SOCKET_DEFAULT)]
        socket: PathBuf,
        /// Persist all Contents to this JSON file (atomic, mode 0600).
        /// Omit for an in-memory server seeded with fixtures.
        #[arg(long, conflicts_with = "node")]
        data: Option<PathBuf>,
        /// Use OAuth credentials from `<DIR>/credentials.json` and read
        /// `kind=wiki` Contents from a real local node. Run
        /// `wiki-server connect --node DIR` once first.
        #[arg(long, conflicts_with = "data")]
        node: Option<PathBuf>,
    },
    /// Connect this Wiki app to a local node through the integrated OAuth flow.
    ///
    /// Prints the verification URL. Approve the request in Harmion Admin
    /// (Connect), then the command stores access/refresh tokens in
    /// `<DIR>/credentials.json` (mode 0600) without printing them.
    Connect {
        #[arg(long)]
        node: PathBuf,
        #[arg(long, default_value = "Harmion Wiki")]
        name: String,
        /// Loopback callback registered with the node. It is not opened or
        /// served by this command; owner approval happens in Harmion Admin.
        #[arg(long, default_value = "http://127.0.0.1:53682/wiki/callback")]
        redirect_uri: String,
        /// Read kind grants; defaults to `wiki`.
        #[arg(long = "read-kind", value_delimiter = ',')]
        read_kind: Vec<String>,
        /// Write kind grants; defaults to `wiki,snippet`. Snippet conversion
        /// kinds from `!` fences must be included here.
        #[arg(long = "write-kind", value_delimiter = ',')]
        write_kind: Vec<String>,
        #[arg(long)]
        read_all: bool,
        #[arg(long)]
        write_all: bool,
        /// Request Interpreter `call` (default; use --no-call to omit).
        #[arg(long)]
        no_call: bool,
        /// Request Provider registration (default; use --no-provide to omit).
        #[arg(long)]
        no_provide: bool,
        /// Provider names to request; defaults to the three built-in Wiki
        /// providers.
        #[arg(long = "provide-name", value_delimiter = ',')]
        provide_name: Vec<String>,
        /// Maximum wait for owner approval, in seconds.
        #[arg(long, default_value_t = 300)]
        timeout_secs: u64,
        #[arg(long, default_value_t = 1)]
        poll_secs: u64,
    },
    List {
        #[arg(long, default_value = SOCKET_DEFAULT)]
        socket: PathBuf,
    },
    Open {
        id: String,
        #[arg(long, default_value = SOCKET_DEFAULT)]
        socket: PathBuf,
    },
    Workspace {
        id: String,
        #[arg(long, default_value = SOCKET_DEFAULT)]
        socket: PathBuf,
    },
    Search {
        query: String,
        #[arg(long, default_value = SOCKET_DEFAULT)]
        socket: PathBuf,
    },
    Save {
        id: String,
        #[arg(long)]
        rev: u64,
        /// Convert loose `!` fences into snippets and canonical host fences.
        #[arg(long)]
        auto_convert: bool,
        #[arg(long, default_value = SOCKET_DEFAULT)]
        socket: PathBuf,
    },
    Status {
        #[arg(long, default_value = SOCKET_DEFAULT)]
        socket: PathBuf,
    },
    Conflicts {
        id: String,
        #[arg(long, default_value = SOCKET_DEFAULT)]
        socket: PathBuf,
    },
    Transclude {
        id: String,
        #[arg(long, default_value_t = 1)]
        depth: u32,
        #[arg(long, default_value = SOCKET_DEFAULT)]
        socket: PathBuf,
    },
    Export {
        #[arg(long)]
        dir: Option<PathBuf>,
        #[arg(long)]
        raw: bool,
        #[arg(long, default_value = SOCKET_DEFAULT)]
        socket: PathBuf,
    },
    Import {
        dir: PathBuf,
        #[arg(long, default_value = SOCKET_DEFAULT)]
        socket: PathBuf,
    },
    /// Create a page; the body is read from stdin.
    #[command(name = "create_page")]
    CreatePage {
        title: String,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        aliases: Vec<String>,
        #[arg(long, default_value = SOCKET_DEFAULT)]
        socket: PathBuf,
    },
    /// Create a snippet; the body is read from stdin.
    #[command(name = "create_snippet")]
    CreateSnippet {
        title: String,
        #[arg(long)]
        format: Option<String>,
        #[arg(long)]
        language: Option<String>,
        #[arg(long, value_delimiter = ',')]
        kinds: Vec<String>,
        #[arg(long, default_value = SOCKET_DEFAULT)]
        socket: PathBuf,
    },
    /// Convert one loose `!` fenced block into a snippet + canonical host body.
    #[command(name = "convert_fence")]
    ConvertFence {
        page_id: String,
        #[arg(long, conflicts_with_all = ["start", "end"])]
        fence_index: Option<u64>,
        #[arg(long, requires = "end", conflicts_with = "fence_index")]
        start: Option<u64>,
        #[arg(long, requires = "start", conflicts_with = "fence_index")]
        end: Option<u64>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long, default_value = SOCKET_DEFAULT)]
        socket: PathBuf,
    },
    #[command(name = "update_meta")]
    UpdateMeta {
        id: String,
        #[arg(long)]
        rev: Option<u64>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        aliases: Vec<String>,
        #[arg(long, default_value = SOCKET_DEFAULT)]
        socket: PathBuf,
    },
    Delete {
        id: String,
        #[arg(long, default_value = SOCKET_DEFAULT)]
        socket: PathBuf,
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    match Cli::parse().command {
        Command::Serve { socket, data, node } => serve(&socket, data, node),
        Command::Connect {
            node,
            name,
            redirect_uri,
            read_kind,
            write_kind,
            read_all,
            write_all,
            no_call,
            no_provide,
            provide_name,
            timeout_secs,
            poll_secs,
        } => connect(ConnectOptions {
            node,
            name,
            redirect_uri,
            read_kind,
            write_kind,
            read_all,
            write_all,
            call: !no_call,
            provide: !no_provide,
            provide_name,
            timeout_secs,
            poll_secs,
        }),
        Command::List { socket } => print_call(&socket, "list", json!({})),
        Command::Open { id, socket } => print_call(&socket, "open", json!({ "id": id })),
        Command::Workspace { id, socket } => print_call(&socket, "workspace", json!({ "id": id })),
        Command::Search { query, socket } => {
            print_call(&socket, "search", json!({ "query": query }))
        }
        Command::Save {
            id,
            rev,
            auto_convert,
            socket,
        } => {
            let body = read_stdin_body()?;
            print_call(
                &socket,
                "save",
                json!({
                    "id": id,
                    "rev": rev,
                    "body": body,
                    "auto_convert": auto_convert,
                }),
            )
        }
        Command::Status { socket } => print_call(&socket, "status", json!({})),
        Command::Conflicts { id, socket } => print_call(&socket, "conflicts", json!({ "id": id })),
        Command::Transclude { id, depth, socket } => {
            print_call(&socket, "transclude", json!({ "id": id, "depth": depth }))
        }
        Command::Export { dir, raw, socket } => print_call(
            &socket,
            "export",
            json!({ "dir": dir.map(|dir| dir.display().to_string()), "raw": raw }),
        ),
        Command::Import { dir, socket } => print_call(
            &socket,
            "import",
            json!({ "dir": dir.display().to_string() }),
        ),
        Command::CreatePage {
            title,
            tags,
            aliases,
            socket,
        } => {
            let body = read_stdin_body()?;
            print_call(
                &socket,
                "create_page",
                json!({ "title": title, "body": body, "tags": tags, "aliases": aliases }),
            )
        }
        Command::CreateSnippet {
            title,
            format,
            language,
            kinds,
            socket,
        } => {
            let body = read_stdin_body()?;
            print_call(
                &socket,
                "create_snippet",
                json!({
                    "title": title,
                    "body": body,
                    "format": format,
                    "language": language,
                    "kinds": kinds,
                }),
            )
        }
        Command::ConvertFence {
            page_id,
            fence_index,
            start,
            end,
            title,
            socket,
        } => print_call(
            &socket,
            "convert_fence",
            json!({
                "page_id": page_id,
                "fence_index": fence_index,
                "start": start,
                "end": end,
                "title": title,
            }),
        ),
        Command::UpdateMeta {
            id,
            rev,
            title,
            tags,
            aliases,
            socket,
        } => print_call(
            &socket,
            "update_meta",
            json!({ "id": id, "rev": rev, "title": title, "tags": tags, "aliases": aliases }),
        ),
        Command::Delete { id, socket } => print_call(&socket, "delete", json!({ "id": id })),
    }
}

/// Read the whole body from stdin exactly once. Split out so `save` and the
/// create commands cannot accidentally consume stdin twice.
fn read_stdin_body() -> std::io::Result<String> {
    let mut body = String::new();
    std::io::stdin().read_to_string(&mut body)?;
    Ok(body)
}

struct ConnectOptions {
    node: PathBuf,
    name: String,
    redirect_uri: String,
    read_kind: Vec<String>,
    write_kind: Vec<String>,
    read_all: bool,
    write_all: bool,
    call: bool,
    provide: bool,
    provide_name: Vec<String>,
    timeout_secs: u64,
    poll_secs: u64,
}

fn connect(options: ConnectOptions) -> Result<(), Box<dyn std::error::Error>> {
    let info = auth::connection_info(&options.node)?;
    if info.oauth_origin.is_none() {
        eprintln!(
            "warning: this node has no OAuth origin; connect will still issue tokens, \
             but they cannot be refreshed and `serve --node` will require a new connect"
        );
    }
    let client = Client::anonymous(info.clone())?;
    let pkce = auth::generate_pkce()?;
    let state = auth::random_state()?;

    let read_kind = if options.read_kind.is_empty() {
        vec!["wiki".to_owned()]
    } else {
        options.read_kind
    };
    let write_kind = if options.write_kind.is_empty() {
        vec!["wiki".to_owned(), "snippet".to_owned()]
    } else {
        options.write_kind
    };
    let provide_name = if options.provide_name.is_empty() {
        vec![
            "wiki/backlinks".to_owned(),
            "wiki/outline".to_owned(),
            "wiki/transclude".to_owned(),
        ]
    } else {
        options.provide_name
    };
    let grants = GrantSpec {
        read: auth::filter_spec(options.read_all, read_kind.clone()),
        write: auth::filter_spec(options.write_all, write_kind.clone()),
        call: options.call,
        provide: options.provide,
        provide_names: if options.provide {
            provide_name
        } else {
            Vec::new()
        },
        manage_apps: false,
        manage_nodes: false,
    };
    let offer = ConnectionOffer {
        version: 1,
        name: options.name,
        redirect_uri: options.redirect_uri,
        web_origin: None,
        client_id: None,
        challenge: pkce.challenge,
        state,
        grants,
        provider: None,
        provider_replace: false,
    };
    // Fail before creating a pending request if the offer violates the same
    // validation the node applies.
    harmion_client::protocol::connect::validate_offer(&offer)
        .map_err(|error| format!("invalid connection offer: {error}"))?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("cannot build connect runtime: {error}"))?;
    let completion = runtime.block_on(connect_async(
        &client,
        &offer,
        &pkce.verifier,
        options.timeout_secs,
        options.poll_secs,
    ))?;

    let now = auth::now_seconds();
    let expires_in = completion
        .get("expires_in")
        .and_then(Value::as_i64)
        .filter(|seconds| *seconds > 0)
        .ok_or("node connect-complete returned no valid expires_in")?;
    let credentials = auth::Credentials {
        version: 1,
        client_id: json_string(&completion, "client_id")?,
        access_token: json_string(&completion, "access_token")?,
        refresh_token: json_string(&completion, "refresh_token")?,
        expires_at: now.saturating_add(expires_in),
        scope: completion
            .get("scope")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        oauth_origin: info.oauth_origin.clone(),
    };
    credentials.save(&options.node)?;
    let missing = credentials.missing_write_kinds(write_kind.iter());
    if !missing.is_empty() {
        eprintln!(
            "warning: the approved scope does not allow Write for kind(s): {}; \
             re-run connect with matching --write-kind values",
            missing.join(", ")
        );
    }
    println!("connected {} to {}", offer.name, info.endpoint);
    println!(
        "credentials: {} (mode 0600)",
        options.node.join(auth::CREDENTIALS_FILE).display()
    );
    println!("client_id: {}", credentials.client_id);
    println!(
        "access token expires at {} (unix seconds); it will refresh automatically",
        credentials.expires_at
    );
    if credentials.oauth_origin.is_none() {
        eprintln!("warning: no OAuth origin was published; automatic refresh is unavailable");
    }
    Ok(())
}

async fn connect_async(
    client: &Client,
    offer: &ConnectionOffer,
    verifier: &str,
    timeout_secs: u64,
    poll_secs: u64,
) -> Result<Value, Box<dyn std::error::Error>> {
    let started = client
        .connect_start(offer)
        .await
        .map_err(|error| format!("connect-start failed: {error}"))?;
    let connection_id = json_string(&started, "connection_id")?;
    let request_secret = json_string(&started, "request_secret")?;
    println!("connection: {connection_id}");
    match started.get("verification_url").and_then(Value::as_str) {
        Some(url) => {
            println!("Open this URL, then approve the request in Harmion Admin > Connect:");
            println!("  {url}");
        }
        None => println!(
            "Approve this request in Harmion Admin > Connect (no OAuth verification URL was published)."
        ),
    }
    println!(
        "Waiting for owner approval (up to {timeout_secs}s; no browser is opened automatically)..."
    );
    let deadline = Instant::now() + Duration::from_secs(timeout_secs.max(1));
    let mut last_status = String::new();
    loop {
        let request = ConnectionStatusRequest {
            connection_id: connection_id.clone(),
            request_secret: request_secret.clone(),
        };
        let view = client
            .connect_status(&request)
            .await
            .map_err(|error| format!("connect-status failed: {error}"))?;
        let status = view
            .get("status")
            .and_then(Value::as_str)
            .ok_or("connect-status returned no status")?
            .to_owned();
        if status != last_status {
            println!("status: {status}");
            last_status = status.clone();
        }
        match status.as_str() {
            "approved" => break,
            "denied" => return Err("the owner denied the connection request".into()),
            "expired" => {
                return Err(
                    "the connection request expired before approval; run connect again".into(),
                );
            }
            "completed" => {
                return Err(
                    "the connection was already completed without the tokens being saved; run connect again"
                        .into(),
                );
            }
            "pending" => {}
            other => return Err(format!("unexpected connection status {other:?}").into()),
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "timed out after {timeout_secs}s waiting for owner approval; the request expires after 5 minutes"
            )
            .into());
        }
        tokio::time::sleep(Duration::from_secs(poll_secs.max(1))).await;
    }
    let request = ConnectionCompleteRequest {
        connection_id,
        request_secret,
        code_verifier: verifier.to_owned(),
    };
    client
        .connect_complete(&request)
        .await
        .map_err(|error| format!("connect-complete failed: {error}").into())
}

fn json_string(value: &Value, key: &str) -> Result<String, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("node response is missing string field {key:?}"))
}

fn print_call(
    socket: &Path,
    method: &str,
    params: Value,
) -> Result<(), Box<dyn std::error::Error>> {
    let value = call(socket, method, params)?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn serve(
    path: &Path,
    data: Option<PathBuf>,
    node: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let state = match node {
        Some(dir) => State::open_node(dir),
        None => State::load(data),
    }
    .map_err(|error| -> Box<dyn std::error::Error> { error.into() })?;
    let status = state.status();
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_socket() => {
            fs::remove_file(path)?;
        }
        Ok(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("refusing to remove non-socket path {}", path.display()),
            )
            .into());
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let listener = UnixListener::bind(path)?;
    // The local IPC endpoint carries the same data as the store; keep it
    // owner-only even when the process umask is permissive.
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    eprintln!(
        "wiki-server listening on {} (backend={}, persistence={})",
        path.display(),
        status["backend"].as_str().unwrap_or("?"),
        status["persistence"].as_str().unwrap_or("-")
    );
    let state = Arc::new(Mutex::new(state));

    for stream in listener.incoming() {
        let stream = match stream {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!("accept failed: {error}");
                continue;
            }
        };
        let state = Arc::clone(&state);
        thread::spawn(move || {
            if let Err(error) = handle_client(stream, state) {
                eprintln!("client failed: {error}");
            }
        });
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct Request {
    id: u64,
    method: String,
    #[serde(default)]
    params: Value,
}

fn handle_client(mut stream: UnixStream, state: Arc<Mutex<State>>) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        if line.len() > 2 * 1024 * 1024 {
            let response = json!({
                "id": null,
                "error": { "code": 413, "message": "request line exceeds 2 MiB" }
            });
            let mut bytes = serde_json::to_vec(&response).expect("response serializes");
            bytes.push(b'\n');
            stream.write_all(&bytes)?;
            stream.flush()?;
            break;
        }
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(request) => dispatch(&state, request),
            Err(error) => json!({
                "id": null,
                "error": { "code": 400, "message": format!("invalid request: {error}") }
            }),
        };
        let mut bytes = serde_json::to_vec(&response).expect("response serializes");
        bytes.push(b'\n');
        stream.write_all(&bytes)?;
        stream.flush()?;
    }
    Ok(())
}

fn dispatch(state: &Arc<Mutex<State>>, request: Request) -> Value {
    let request_id = request.id;
    let result = dispatch_result(state, &request);
    match result {
        Ok(result) => json!({ "id": request_id, "result": result }),
        Err(error) => error.to_json(request_id),
    }
}

fn dispatch_result(state: &Arc<Mutex<State>>, request: &Request) -> RpcResult<Value> {
    let params = &request.params;
    match request.method.as_str() {
        "list" => {
            let filter = optional_str(params, "type")?;
            let state = state.lock().expect("state lock");
            let mut items = state.list();
            if let Some(filter) = filter {
                items.retain(|item| item.content_type == filter);
            }
            Ok(json!({ "items": items }))
        }
        "open" => {
            let id = required_str(params, "id")?;
            let state = state.lock().expect("state lock");
            Ok(serde_json::to_value(state.open(&id)?).expect("content serializes"))
        }
        "workspace" => {
            let id = required_str(params, "id")?;
            let state = state.lock().expect("state lock");
            Ok(serde_json::to_value(state.workspace(&id)?).expect("workspace serializes"))
        }
        "search" => {
            let query = optional_str(params, "query")?.unwrap_or_default();
            let state = state.lock().expect("state lock");
            Ok(json!({ "items": state.search(&query) }))
        }
        "save" => {
            let id = required_str(params, "id")?;
            let rev = required_u64(params, "rev")?;
            let body = required_str(params, "body")?;
            let auto_convert = optional_bool(params, "auto_convert")?.unwrap_or(false);
            let mut state = state.lock().expect("state lock");
            if auto_convert {
                Ok(
                    serde_json::to_value(state.save_with_conversion(&id, rev, body)?)
                        .expect("save report serializes"),
                )
            } else {
                Ok(serde_json::to_value(state.save(&id, rev, body)?).expect("content serializes"))
            }
        }
        "status" => {
            let state = state.lock().expect("state lock");
            Ok(state.status())
        }
        "provider_list" => Ok(provider::list()),
        "provider_call" => {
            let name = required_str(params, "name")?;
            let state = state.lock().expect("state lock");
            provider::call(&state, &name, params)
        }
        "update_meta" => {
            let id = required_str(params, "id")?;
            let rev = optional_u64(params, "rev")?;
            let title = optional_str(params, "title")?;
            let tags = if params.get("tags").is_some() {
                Some(optional_str_array(params, "tags")?)
            } else {
                None
            };
            let aliases = if params.get("aliases").is_some() {
                Some(optional_str_array(params, "aliases")?)
            } else {
                None
            };
            let mut state = state.lock().expect("state lock");
            Ok(
                serde_json::to_value(state.update_meta(&id, rev, title, tags, aliases)?)
                    .expect("content serializes"),
            )
        }
        "create_page" => {
            let title = required_str(params, "title")?;
            let body = required_str(params, "body")?;
            let tags = optional_str_array(params, "tags")?;
            let aliases = optional_str_array(params, "aliases")?;
            let mut state = state.lock().expect("state lock");
            Ok(
                serde_json::to_value(state.create_page(&title, body, tags, aliases)?)
                    .expect("content serializes"),
            )
        }
        "create_snippet" => {
            let title = required_str(params, "title")?;
            let body = required_str(params, "body")?;
            let format = optional_str(params, "format")?;
            let language = optional_str(params, "language")?;
            let kinds = optional_str_array(params, "kinds")?;
            let mut state = state.lock().expect("state lock");
            Ok(
                serde_json::to_value(state.create_snippet(&title, body, format, language, kinds)?)
                    .expect("content serializes"),
            )
        }
        "convert_fence" => {
            let page_id = required_str(params, "page_id")?;
            let fence_index = optional_u64(params, "fence_index")?;
            let start = optional_u64(params, "start")?;
            let end = optional_u64(params, "end")?;
            let selection = match (fence_index, start, end) {
                (Some(_), _, _) => {
                    if start.is_some() || end.is_some() {
                        return Err(RpcError::bad_request(
                            "use either fence_index or start+end, not both",
                        ));
                    }
                    FenceSelection::Index(
                        usize::try_from(fence_index.unwrap())
                            .map_err(|_| RpcError::bad_request("fence_index is out of range"))?,
                    )
                }
                (None, Some(start), Some(end)) => FenceSelection::Span {
                    start: usize::try_from(start)
                        .map_err(|_| RpcError::bad_request("start is out of range"))?,
                    end: usize::try_from(end)
                        .map_err(|_| RpcError::bad_request("end is out of range"))?,
                },
                (None, None, None) => {
                    return Err(RpcError::bad_request(
                        "convert_fence requires fence_index or start+end",
                    ));
                }
                (None, Some(_), None) | (None, None, Some(_)) => {
                    return Err(RpcError::bad_request(
                        "convert_fence span requires both start and end",
                    ));
                }
            };
            let title = optional_str(params, "title")?;
            let mut state = state.lock().expect("state lock");
            let report = state.convert_fence(&page_id, selection, title)?;
            Ok(serde_json::to_value(report).expect("convert report serializes"))
        }
        "delete" => {
            let id = required_str(params, "id")?;
            let mut state = state.lock().expect("state lock");
            let content = state.delete(&id)?;
            Ok(json!({ "deleted": true, "id": id, "content": content }))
        }
        "transclude" => {
            let id = required_str(params, "id")?;
            let depth = optional_u64(params, "depth")?.unwrap_or(1);
            let depth =
                u32::try_from(depth).map_err(|_| RpcError::bad_request("depth is out of range"))?;
            let state = state.lock().expect("state lock");
            Ok(serde_json::to_value(state.transclude(&id, depth)?)
                .expect("transclusion serializes"))
        }
        "export" => {
            let raw = optional_bool(params, "raw")?.unwrap_or(false);
            let dir = optional_str(params, "dir")?;
            let state = state.lock().expect("state lock");
            Ok(state.export(raw, dir.as_deref().map(Path::new))?)
        }
        "import" => {
            let dir = required_str(params, "dir")?;
            let mut state = state.lock().expect("state lock");
            Ok(serde_json::to_value(state.import_dir(Path::new(&dir))?)
                .expect("import report serializes"))
        }
        "conflicts" => {
            let id = required_str(params, "id")?;
            let state = state.lock().expect("state lock");
            state.conflicts(&id)
        }
        other => Err(RpcError::new(404, format!("unknown method {other}"))),
    }
}

fn call(socket: &Path, method: &str, params: Value) -> Result<Value, Box<dyn std::error::Error>> {
    let mut stream = UnixStream::connect(socket)?;
    let request = json!({ "id": 1, "method": method, "params": params });
    let mut bytes = serde_json::to_vec(&request)?;
    bytes.push(b'\n');
    stream.write_all(&bytes)?;
    stream.flush()?;

    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    let response: Value = serde_json::from_str(&line)?;
    if let Some(error) = response.get("error") {
        return Err(format!("server error: {error}").into());
    }
    Ok(response.get("result").cloned().unwrap_or(Value::Null))
}
