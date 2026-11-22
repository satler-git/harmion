//! End-to-end stdio test: spawn the real `wiki-lsp` binary against a fake
//! Unix-socket server and exchange `Content-Length` framed JSON-RPC.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Cursor, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use harmion_wiki_lsp::framing::{read_message, write_message};
use serde_json::{Value, json};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct FakeServer {
    path: PathBuf,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl FakeServer {
    fn start() -> Self {
        let path = std::env::temp_dir().join(format!(
            "harmion-wiki-lsp-stdio-{}-{}.sock",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).expect("bind fake server");
        listener.set_nonblocking(true).expect("nonblocking listener");
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        thread::spawn(move || handle_connection(stream));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            path,
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

fn handle_connection(stream: UnixStream) {
    let reader = match stream.try_clone() {
        Ok(stream) => BufReader::new(stream),
        Err(_) => return,
    };
    let mut writer = stream;
    for line in reader.lines() {
        let line = match line {
            Ok(line) => line,
            Err(_) => break,
        };
        let request: Value = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(_) => break,
        };
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let method = request.get("method").and_then(Value::as_str).unwrap_or("");
        let result = match method {
            "list" => json!({
                "items": [
                    {
                        "id": "p-home",
                        "type": "page",
                        "title": "家計の記録",
                        "format": null,
                        "language": null,
                        "kinds": ["wiki"],
                        "rev": 1,
                        "excerpt": "毎月の支出"
                    },
                    {
                        "id": "p-goals",
                        "type": "page",
                        "title": "2026年の目標",
                        "format": null,
                        "language": null,
                        "kinds": ["wiki"],
                        "rev": 1,
                        "excerpt": "毎週見直す"
                    }
                ]
            }),
            "search" => json!({ "items": [] }),
            "open" => json!({
                "id": "p-goals",
                "type": "page",
                "title": "2026年の目標",
                "body": "# 2026年の目標\n",
                "kinds": ["wiki"],
                "rev": 1
            }),
            "workspace" => json!({ "links": { "backlinks": [] } }),
            _ => Value::Null,
        };
        let response = json!({ "id": id, "result": result });
        let mut bytes = serde_json::to_vec(&response).expect("response serializes");
        bytes.push(b'\n');
        if writer.write_all(&bytes).is_err() || writer.flush().is_err() {
            break;
        }
    }
}

#[test]
fn stdio_binary_frames_initialize_and_diagnostics() {
    let fake = FakeServer::start();
    let document = "# 家計の記録\n\n[[p-goals]] [[missing-page]]\n";

    let mut input = Vec::new();
    write_message(
        &mut input,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "processId": null, "capabilities": {} }
        }),
    )
    .unwrap();
    write_message(
        &mut input,
        &json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": "wiki://p-home",
                    "languageId": "markdown",
                    "version": 1,
                    "text": document
                }
            }
        }),
    )
    .unwrap();
    write_message(
        &mut input,
        &json!({ "jsonrpc": "2.0", "id": 2, "method": "shutdown", "params": null }),
    )
    .unwrap();
    write_message(
        &mut input,
        &json!({ "jsonrpc": "2.0", "method": "exit" }),
    )
    .unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_wiki-lsp"))
        .arg("--socket")
        .arg(&fake.path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn wiki-lsp");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(&input)
        .expect("write LSP input");
    let output = child.wait_with_output().expect("wait wiki-lsp");
    assert!(
        output.status.success(),
        "wiki-lsp failed: status={:?} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout.starts_with(b"Content-Length: "),
        "stdout is not framed: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );

    let mut reader = Cursor::new(output.stdout);
    let mut frames = Vec::new();
    while let Some(message) = read_message(&mut reader).expect("parse stdout frame") {
        frames.push(message);
    }

    let initialize = frames
        .iter()
        .find(|message| message.get("id").and_then(Value::as_u64) == Some(1))
        .expect("initialize response");
    assert_eq!(
        initialize["result"]["capabilities"]["definitionProvider"],
        true
    );

    let diagnostics = frames
        .iter()
        .rev()
        .find(|message| {
            message["method"] == "textDocument/publishDiagnostics"
                && message["params"]["uri"] == "wiki://p-home"
        })
        .and_then(|message| message["params"]["diagnostics"].as_array())
        .expect("diagnostics notification")
        .iter()
        .filter_map(|diagnostic| diagnostic["code"].as_str())
        .collect::<Vec<_>>();
    assert!(
        diagnostics.contains(&"unresolved-link"),
        "expected an unresolved link, got {diagnostics:?} in {frames:#?}"
    );

    let shutdown = frames
        .iter()
        .find(|message| message.get("id").and_then(Value::as_u64) == Some(2))
        .expect("shutdown response");
    assert!(shutdown["result"].is_null());
}
