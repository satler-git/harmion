//! Read-only client for `wiki-server`'s Unix-socket JSON-RPC line protocol.
//!
//! The LSP only uses `list`, `open`, `search` and `workspace`. There is no
//! `save` path here on purpose: saving belongs to the Neovim plugin and this
//! process never writes or handles credentials/tokens.

use std::fmt;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::uri::is_content_id;

pub const DEFAULT_SOCKET: &str = "/tmp/harmion-wiki-m0.sock";
const DEFAULT_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Debug, Clone)]
pub struct ServerError(pub String);

impl fmt::Display for ServerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ServerError {}

/// A Content summary as returned by `list` and `search`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ContentSummary {
    pub id: String,
    #[serde(rename = "type", default)]
    pub content_type: String,
    pub title: String,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub kinds: Vec<String>,
    #[serde(default)]
    pub excerpt: String,
    #[serde(default)]
    pub rev: u64,
}

/// A full Content as returned by `open`.
#[derive(Debug, Clone, Deserialize)]
pub struct Content {
    pub id: String,
    #[serde(rename = "type", default)]
    pub content_type: String,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub kinds: Vec<String>,
    #[serde(default)]
    pub rev: u64,
}

impl Content {
    fn into_summary(self) -> ContentSummary {
        ContentSummary {
            excerpt: excerpt(&self.body),
            id: self.id,
            content_type: self.content_type,
            title: self.title,
            format: self.format,
            language: self.language,
            kinds: self.kinds,
            rev: self.rev,
        }
    }
}

fn excerpt(body: &str) -> String {
    let line = body
        .lines()
        .map(str::trim)
        .find(|line| {
            !line.is_empty()
                && !line.starts_with('#')
                && !line.starts_with("!```")
                && !line.starts_with("```")
        })
        .unwrap_or_default();
    let mut chars = line.chars();
    let excerpt = chars.by_ref().take(120).collect::<String>();
    if chars.next().is_some() {
        format!("{excerpt}…")
    } else {
        excerpt
    }
}

struct Connection {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

/// A lazily connected, serialized JSON-RPC client.
///
/// Every call is bounded by a short read/write timeout. A failed call drops
/// the connection; the next call reconnects, so a restarted server is picked
/// up without restarting the editor.
pub struct ServerClient {
    socket: PathBuf,
    connection: Mutex<Option<Connection>>,
    next_id: AtomicU64,
    timeout: Duration,
}

impl ServerClient {
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self::with_timeout(socket, DEFAULT_TIMEOUT)
    }

    pub fn with_timeout(socket: impl Into<PathBuf>, timeout: Duration) -> Self {
        Self {
            socket: socket.into(),
            connection: Mutex::new(None),
            next_id: AtomicU64::new(1),
            timeout,
        }
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// Call one server method. The returned value is the JSON-RPC `result`.
    pub fn call(&self, method: &str, params: Value) -> Result<Value, ServerError> {
        let mut guard = self
            .connection
            .lock()
            .map_err(|_| ServerError("wiki-server client lock poisoned".to_owned()))?;

        if guard.is_none() {
            let stream = UnixStream::connect(&self.socket).map_err(|error| {
                ServerError(format!("connect {}: {error}", self.socket.display()))
            })?;
            stream
                .set_read_timeout(Some(self.timeout))
                .map_err(|error| ServerError(format!("set read timeout: {error}")))?;
            stream
                .set_write_timeout(Some(self.timeout))
                .map_err(|error| ServerError(format!("set write timeout: {error}")))?;
            let reader = BufReader::new(
                stream
                    .try_clone()
                    .map_err(|error| ServerError(format!("clone wiki-server socket: {error}")))?,
            );
            *guard = Some(Connection {
                reader,
                writer: stream,
            });
        }

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = json!({ "id": id, "method": method, "params": params });
        let result = (|| -> Result<Value, ServerError> {
            let connection = guard
                .as_mut()
                .ok_or_else(|| ServerError("wiki-server connection disappeared".to_owned()))?;

            let mut body = serde_json::to_vec(&request)
                .map_err(|error| ServerError(format!("encode request: {error}")))?;
            body.push(b'\n');
            connection
                .writer
                .write_all(&body)
                .map_err(|error| ServerError(format!("write request: {error}")))?;
            connection
                .writer
                .flush()
                .map_err(|error| ServerError(format!("flush request: {error}")))?;

            let mut line = String::new();
            let read = connection
                .reader
                .read_line(&mut line)
                .map_err(|error| ServerError(format!("read response: {error}")))?;
            if read == 0 {
                return Err(ServerError("wiki-server closed the connection".to_owned()));
            }

            let response: Value = serde_json::from_str(&line)
                .map_err(|error| ServerError(format!("decode response: {error}")))?;
            if let Some(error) = response.get("error") {
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown wiki-server error");
                return Err(ServerError(message.to_owned()));
            }
            Ok(response.get("result").cloned().unwrap_or(Value::Null))
        })();

        if result.is_err() {
            *guard = None;
        }
        result
    }

    pub fn list(&self) -> Result<Vec<ContentSummary>, ServerError> {
        parse_items(self.call("list", json!({}))?)
    }

    pub fn search(&self, query: &str) -> Result<Vec<ContentSummary>, ServerError> {
        parse_items(self.call("search", json!({ "query": query }))?)
    }

    pub fn open(&self, id: &str) -> Result<Content, ServerError> {
        serde_json::from_value(self.call("open", json!({ "id": id }))?)
            .map_err(|error| ServerError(format!("decode content: {error}")))
    }

    /// Backlinks from `workspace.links.backlinks`.
    pub fn workspace_backlinks(&self, id: &str) -> Result<Vec<ContentSummary>, ServerError> {
        let workspace = self.call("workspace", json!({ "id": id }))?;
        let items = workspace
            .pointer("/links/backlinks")
            .and_then(Value::as_array)
            .ok_or_else(|| ServerError("workspace response has no links.backlinks".to_owned()))?;
        serde_json::from_value(Value::Array(items.clone()))
            .map_err(|error| ServerError(format!("decode backlinks: {error}")))
    }

    /// Resolve a link target by Content id or title through the server.
    ///
    /// A 64-hex target is opened as an id. Anything else is searched by title:
    /// exact (case-insensitive) first, then a sole search result.
    pub fn resolve_target(&self, target: &str) -> Result<Option<ContentSummary>, ServerError> {
        if is_content_id(target) {
            return self
                .open(target)
                .map(|content| Some(content.into_summary()));
        }
        // Fixture and local IDs are not 64-hex Content IDs. Try an exact open
        // first; real App API IDs take the branch above.
        if let Ok(content) = self.open(target) {
            return Ok(Some(content.into_summary()));
        }

        let items = self.search(target)?;
        if let Some(found) = items.iter().find(|content| content.title == target) {
            return Ok(Some(found.clone()));
        }
        let lowered = target.to_lowercase();
        if let Some(found) = items
            .iter()
            .find(|content| content.title.to_lowercase() == lowered)
        {
            return Ok(Some(found.clone()));
        }
        if items.len() == 1 {
            return Ok(items.into_iter().next());
        }
        Ok(None)
    }
}

fn parse_items(value: Value) -> Result<Vec<ContentSummary>, ServerError> {
    #[derive(Deserialize)]
    struct Items {
        #[serde(default)]
        items: Vec<ContentSummary>,
    }
    serde_json::from_value::<Items>(value)
        .map(|parsed| parsed.items)
        .map_err(|error| ServerError(format!("decode list items: {error}")))
}
