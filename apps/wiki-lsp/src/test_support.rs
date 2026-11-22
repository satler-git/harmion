//! In-process fake `wiki-server` for unit tests.
//!
//! It speaks the same newline-delimited JSON-RPC as `apps/wiki-server` and
//! serves a small fixture set. Tests get a real Unix socket without spawning a
//! separate process.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use harmion_wiki_core::parse_links;
use serde_json::{Value, json};

pub const PAGE_ID: &str = "p-home";
pub const PAGE_TITLE: &str = "家計の記録";
pub const GOALS_ID: &str = "p-goals";
pub const GOALS_TITLE: &str = "2026年の目標";
pub const RECEIPTS_ID: &str = "p-receipts";
pub const RECEIPTS_TITLE: &str = "レシート処理";
pub const SNIPPET_ID: &str = "3f2a9c1e00000000000000000000000000000000000000000000000000000001";
pub const SNIPPET_TITLE: &str = "買い物ジャーナル";
pub const BUDGET_ID: &str = "3f2a9c1e00000000000000000000000000000000000000000000000000000002";
pub const DUP_A_ID: &str = "p-dup-a";
pub const DUP_B_ID: &str = "p-dup-b";
pub const DUP_TITLE: &str = "重複ページ";
pub const MISSING_ID: &str = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";

static COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct FakeServer {
    path: PathBuf,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl FakeServer {
    pub fn start() -> Self {
        Self::start_with(contents())
    }

    pub fn start_with(contents: Vec<Value>) -> Self {
        let path = std::env::temp_dir().join(format!(
            "harmion-wiki-lsp-test-{}-{}.sock",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_file(&path);

        let listener = UnixListener::bind(&path).expect("bind fake wiki-server");
        listener
            .set_nonblocking(true)
            .expect("set fake listener nonblocking");
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let contents = Arc::new(contents);
        let handle = thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let contents = Arc::clone(&contents);
                        thread::spawn(move || handle_connection(stream, contents));
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

    pub fn path(&self) -> &Path {
        &self.path
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

pub fn contents() -> Vec<Value> {
    vec![
        content(
            PAGE_ID,
            "page",
            PAGE_TITLE,
            &format!(
                "# {PAGE_TITLE}\n\n毎月の支出を hledger のジャーナルで管理している。\n\n関連: [[{GOALS_ID}|2026年の目標]] [[{RECEIPTS_ID}|レシート処理]]\n\n!```hledger|ledger,free||{SNIPPET_ID}\n```\n"
            ),
            None,
            None,
            &["wiki", "ledger"],
        ),
        content(
            GOALS_ID,
            "page",
            GOALS_TITLE,
            &format!(
                "# {GOALS_TITLE}\n\n家計の記録を毎週見直す。\n\n- [[{PAGE_ID}|家計の記録]]\n- [[{RECEIPTS_ID}|レシート処理]]\n"
            ),
            None,
            None,
            &["wiki", "ledger"],
        ),
        content(
            RECEIPTS_ID,
            "page",
            RECEIPTS_TITLE,
            &format!(
                "# {RECEIPTS_TITLE}\n\nレシートは週末にまとめて処理する。\n\n- [[{PAGE_ID}|家計の記録]]\n"
            ),
            None,
            None,
            &["wiki", "ledger"],
        ),
        content(
            SNIPPET_ID,
            "snippet",
            SNIPPET_TITLE,
            "2026-11-16 * 買い物\n    支出:食費   1,200 JPY\n    資産:現金  -1,200 JPY\n",
            Some("hledger"),
            Some("ledger"),
            &["wiki", "ledger", "free"],
        ),
        content(
            BUDGET_ID,
            "snippet",
            "予算クエリ",
            "2026-11-01 * 予算\n    支出:食費   30,000 JPY\n",
            Some("hledger"),
            Some("ledger"),
            &["wiki", "ledger"],
        ),
        content(
            DUP_A_ID,
            "page",
            DUP_TITLE,
            "# 重複ページ\n\n1つめ。\n",
            None,
            None,
            &["wiki"],
        ),
        content(
            DUP_B_ID,
            "page",
            DUP_TITLE,
            "# 重複ページ\n\n2つめ。\n",
            None,
            None,
            &["wiki"],
        ),
    ]
}

fn content(
    id: &str,
    content_type: &str,
    title: &str,
    body: &str,
    format: Option<&str>,
    language: Option<&str>,
    kinds: &[&str],
) -> Value {
    json!({
        "id": id,
        "type": content_type,
        "title": title,
        "body": body,
        "format": format,
        "language": language,
        "kinds": kinds,
        "rev": 1,
        "created": "2026-10-02",
        "updated_at": 202611221431u64,
        "updated_label": "14:31"
    })
}

fn handle_connection(stream: UnixStream, contents: Arc<Vec<Value>>) {
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
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(request) => dispatch(&request, &contents),
            Err(error) => json!({
                "id": null,
                "error": { "code": 400, "message": format!("invalid request: {error}") }
            }),
        };
        let mut bytes = serde_json::to_vec(&response).expect("fake response serializes");
        bytes.push(b'\n');
        if writer.write_all(&bytes).is_err() || writer.flush().is_err() {
            break;
        }
    }
}

fn dispatch(request: &Value, contents: &[Value]) -> Value {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();

    let result: Result<Value, String> = match method {
        "list" => Ok(json!({
            "items": contents.iter().map(summary).collect::<Vec<_>>()
        })),
        "search" => {
            let query = request
                .pointer("/params/query")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_lowercase();
            let items = contents
                .iter()
                .filter(|content| {
                    content["title"]
                        .as_str()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&query)
                        || content["body"]
                            .as_str()
                            .unwrap_or("")
                            .to_lowercase()
                            .contains(&query)
                })
                .map(summary)
                .collect::<Vec<_>>();
            Ok(json!({ "items": items }))
        }
        "open" => {
            let requested = request
                .pointer("/params/id")
                .and_then(Value::as_str)
                .unwrap_or("");
            match contents
                .iter()
                .find(|content| content["id"].as_str() == Some(requested))
            {
                Some(content) => Ok(content.clone()),
                None => Err("content not found".to_owned()),
            }
        }
        "workspace" => {
            let requested = request
                .pointer("/params/id")
                .and_then(Value::as_str)
                .unwrap_or("");
            let Some(page) = contents
                .iter()
                .find(|content| content["id"].as_str() == Some(requested))
            else {
                return json!({
                    "id": id,
                    "error": { "code": 404, "message": "content not found" }
                });
            };
            let title = page["title"].as_str().unwrap_or("");
            let backlinks = contents
                .iter()
                .filter(|candidate| {
                    candidate["body"].as_str().is_some_and(|body| {
                        parse_links(body)
                            .iter()
                            .any(|link| link.target == requested || link.target == title)
                    })
                })
                .map(summary)
                .collect::<Vec<_>>();
            Ok(json!({
                "page": page,
                "links": { "outgoing": [], "backlinks": backlinks, "two_hop": [] }
            }))
        }
        other => Err(format!("unknown method {other}")),
    };

    match result {
        Ok(result) => json!({ "id": id, "result": result }),
        Err(message) => json!({
            "id": id,
            "error": { "code": 404, "message": message }
        }),
    }
}

fn summary(content: &Value) -> Value {
    json!({
        "id": content["id"],
        "type": content["type"],
        "title": content["title"],
        "format": content["format"],
        "language": content["language"],
        "kinds": content["kinds"],
        "rev": content["rev"],
        "updated_label": content["updated_label"],
        "excerpt": excerpt(content["body"].as_str().unwrap_or(""))
    })
}

fn excerpt(body: &str) -> String {
    body.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .unwrap_or("")
        .chars()
        .take(120)
        .collect()
}
