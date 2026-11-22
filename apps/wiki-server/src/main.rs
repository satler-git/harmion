use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const SOCKET_DEFAULT: &str = "/tmp/harmion-wiki-m0.sock";

#[derive(Debug, Parser)]
#[command(name = "wiki-server", about = "M0 fake Wiki server")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the Unix-socket JSON-RPC server.
    Serve {
        #[arg(long, default_value = SOCKET_DEFAULT)]
        socket: PathBuf,
    },
    /// List pages and snippets.
    List {
        #[arg(long, default_value = SOCKET_DEFAULT)]
        socket: PathBuf,
    },
    /// Open one Content as JSON.
    Open {
        id: String,
        #[arg(long, default_value = SOCKET_DEFAULT)]
        socket: PathBuf,
    },
    /// Save one Content body. The body is read from stdin.
    Save {
        id: String,
        #[arg(long)]
        rev: u64,
        #[arg(long, default_value = SOCKET_DEFAULT)]
        socket: PathBuf,
    },
}

#[derive(Debug, Clone, Serialize)]
struct Content {
    id: String,
    #[serde(rename = "type")]
    content_type: String,
    title: String,
    body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<String>,
    kinds: Vec<String>,
    rev: u64,
}

#[derive(Debug, Clone, Serialize)]
struct Summary {
    id: String,
    #[serde(rename = "type")]
    content_type: String,
    title: String,
    format: Option<String>,
    language: Option<String>,
    kinds: Vec<String>,
    rev: u64,
}

impl From<&Content> for Summary {
    fn from(content: &Content) -> Self {
        Self {
            id: content.id.clone(),
            content_type: content.content_type.clone(),
            title: content.title.clone(),
            format: content.format.clone(),
            language: content.language.clone(),
            kinds: content.kinds.clone(),
            rev: content.rev,
        }
    }
}

#[derive(Debug)]
struct State {
    contents: BTreeMap<String, Content>,
}

impl State {
    fn fixtures() -> Self {
        let journal_id = "3f2a9c1e00000000000000000000000000000000000000000000000000000001";
        let budget_id = "3f2a9c1e00000000000000000000000000000000000000000000000000000002";
        let home_body = format!(
            "# 家計の記録\n\n毎月の支出は hledger のジャーナルを埋め込んで管理している。埋め込まれたジャーナルは、このページから直接編集できる。\n\n!```hledger|ledger,free||{journal_id}\n```\n\n残高は ledger/balance の解釈を埋め込んで表示している。ジャーナルを直すと、次の更新で再計算される。\n\n関連: [[2026年の目標]] [[レシート処理]]\n"
        );
        let goals_body = "# 2026年の目標\n\n家計の記録を毎週見直す。\n\n- 固定費を月1回確認する\n- レシートを貯めすぎない\n";
        let mut contents = BTreeMap::new();
        for content in [
            Content {
                id: "p-home".to_owned(),
                content_type: "page".to_owned(),
                title: "家計の記録".to_owned(),
                body: home_body,
                format: None,
                language: None,
                kinds: vec!["wiki".to_owned()],
                rev: 1,
            },
            Content {
                id: "p-goals".to_owned(),
                content_type: "page".to_owned(),
                title: "2026年の目標".to_owned(),
                body: goals_body.to_owned(),
                format: None,
                language: None,
                kinds: vec!["wiki".to_owned()],
                rev: 1,
            },
            Content {
                id: journal_id.to_owned(),
                content_type: "snippet".to_owned(),
                title: "買い物ジャーナル".to_owned(),
                body: "2026-11-16 * 買い物\n    支出:食費   1,200 JPY\n    資産:現金  -1,200 JPY\n\n2026-11-17 * 電車\n    支出:交通     420 JPY\n    資産:現金    -420 JPY\n".to_owned(),
                format: Some("hledger".to_owned()),
                language: Some("ledger".to_owned()),
                kinds: vec!["wiki".to_owned(), "ledger".to_owned(), "free".to_owned()],
                rev: 1,
            },
            Content {
                id: budget_id.to_owned(),
                content_type: "snippet".to_owned(),
                title: "予算クエリ".to_owned(),
                body: "2026-11-01 * 予算\n    支出:食費   30,000 JPY\n".to_owned(),
                format: Some("hledger".to_owned()),
                language: Some("ledger".to_owned()),
                kinds: vec!["wiki".to_owned(), "ledger".to_owned()],
                rev: 1,
            },
        ] {
            contents.insert(content.id.clone(), content);
        }
        Self { contents }
    }
}

#[derive(Debug, Deserialize)]
struct Request {
    id: u64,
    method: String,
    #[serde(default)]
    params: Value,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    match Cli::parse().command {
        Command::Serve { socket } => serve(&socket),
        Command::List { socket } => {
            let value = call(&socket, "list", json!({}))?;
            println!("{}", serde_json::to_string_pretty(&value)?);
            Ok(())
        }
        Command::Open { id, socket } => {
            let value = call(&socket, "open", json!({ "id": id }))?;
            println!("{}", serde_json::to_string_pretty(&value)?);
            Ok(())
        }
        Command::Save { id, rev, socket } => {
            let mut body = String::new();
            std::io::stdin().read_to_string(&mut body)?;
            let value = call(
                &socket,
                "save",
                json!({ "id": id, "rev": rev, "body": body }),
            )?;
            println!("{}", serde_json::to_string_pretty(&value)?);
            Ok(())
        }
    }
}

fn serve(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let _ = fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    eprintln!("wiki-server listening on {}", path.display());
    let state = Arc::new(Mutex::new(State::fixtures()));

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

fn handle_client(mut stream: UnixStream, state: Arc<Mutex<State>>) -> std::io::Result<()> {
    let reader = BufReader::new(stream.try_clone()?);
    for line in reader.lines() {
        let line = line?;
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
    let result: Result<Value, (u16, String)> = match request.method.as_str() {
        "list" => {
            let state = state.lock().expect("state lock");
            let items: Vec<Summary> = state.contents.values().map(Summary::from).collect();
            Ok(json!({ "items": items }))
        }
        "open" => {
            let id = request.params.get("id").and_then(Value::as_str);
            match id.and_then(|id| state.lock().expect("state lock").contents.get(id).cloned()) {
                Some(content) => Ok(serde_json::to_value(content).expect("content serializes")),
                None => Err((404, "content not found".to_owned())),
            }
        }
        "save" => {
            let id = request.params.get("id").and_then(Value::as_str);
            let rev = request.params.get("rev").and_then(Value::as_u64);
            let body = request.params.get("body").and_then(Value::as_str);
            match (id, rev, body) {
                (Some(id), Some(rev), Some(body)) => {
                    let mut state = state.lock().expect("state lock");
                    match state.contents.get_mut(id) {
                        Some(content) if content.rev == rev => {
                            content.body = body.to_owned();
                            content.rev += 1;
                            Ok(serde_json::to_value(content.clone()).expect("content serializes"))
                        }
                        Some(_) => Err((409, "revision mismatch".to_owned())),
                        None => Err((404, "content not found".to_owned())),
                    }
                }
                _ => Err((400, "save requires id, rev and body".to_owned())),
            }
        }
        other => Err((404, format!("unknown method {other}"))),
    };

    match result {
        Ok(result) => json!({ "id": request.id, "result": result }),
        Err((code, message)) => json!({
            "id": request.id,
            "error": { "code": code, "message": message }
        }),
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
