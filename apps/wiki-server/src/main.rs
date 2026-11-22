//! Wiki local-first JSON-RPC server.
//!
//! One JSON object per newline over a Unix socket. The M0 methods (`list`,
//! `open`, `workspace`, `search`, `save`) keep their request/response shapes;
//! M1/M3 adds persistence and the `status`, `create_page`, `create_snippet`,
//! `convert_fence`, `delete`, `transclude`, `export` and `conflicts` methods.

mod backend;
mod error;
mod export;
mod model;
mod provider;
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
use serde::Deserialize;
use serde_json::{Value, json};

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
        /// Dev-only: load a read snapshot of `kind=wiki` Contents from a real
        /// node directory (requires connection.json + admin.secret).
        #[arg(long, conflicts_with = "data")]
        node: Option<PathBuf>,
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
