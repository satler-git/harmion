//! The stdio LSP event loop and feature dispatch.

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::completion;
use crate::diagnostics;
use crate::framing;
use crate::hover;
use crate::protocol::{Location, Position, Range};
use crate::providers;
use crate::server::{ContentSummary, ServerClient};
use crate::text;
use crate::uri;

/// Titles and kinds change rarely; avoid a socket round-trip on every
/// keystroke while still picking up external edits within a couple of seconds.
const INDEX_TTL: Duration = Duration::from_secs(2);

struct Document {
    wiki: uri::WikiUri,
    text: String,
    version: i64,
}

pub struct LspServer<R: BufRead, W: Write> {
    reader: R,
    writer: W,
    client: ServerClient,
    documents: BTreeMap<String, Document>,
    index: Vec<ContentSummary>,
    server_available: bool,
    last_index_refresh: Option<Instant>,
    shutdown_requested: bool,
    exit_requested: bool,
}

impl<R: BufRead, W: Write> LspServer<R, W> {
    pub fn new(reader: R, writer: W, socket: impl Into<PathBuf>) -> Self {
        Self {
            reader,
            writer,
            client: ServerClient::new(socket),
            documents: BTreeMap::new(),
            index: Vec::new(),
            server_available: false,
            last_index_refresh: None,
            shutdown_requested: false,
            exit_requested: false,
        }
    }

    /// Run until `exit`, EOF, or a framing/write error. Returns the LSP exit
    /// code (0 after `shutdown` + `exit`) and the writer for tests.
    pub fn run(mut self) -> std::io::Result<(i32, W)> {
        loop {
            match framing::read_message(&mut self.reader) {
                Ok(Some(message)) => {
                    self.handle_message(message)?;
                    if self.exit_requested {
                        break;
                    }
                }
                Ok(None) => break,
                Err(error) => return Err(std::io::Error::other(error.to_string())),
            }
        }
        let code = if self.shutdown_requested { 0 } else { 1 };
        Ok((code, self.writer))
    }

    fn handle_message(&mut self, message: Value) -> std::io::Result<()> {
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let params = message.get("params").cloned().unwrap_or(Value::Null);

        if method.is_empty() {
            if let Some(id) = message.get("id").cloned() {
                self.respond_error(id, -32600, "invalid request".to_owned())?;
            }
            return Ok(());
        }

        match message.get("id").cloned() {
            Some(id) => self.handle_request(&method, id, params),
            None => self.handle_notification(&method, params),
        }
    }

    fn handle_request(&mut self, method: &str, id: Value, params: Value) -> std::io::Result<()> {
        match method {
            "initialize" => {
                let result = self.initialize(&params);
                self.respond(id, result)
            }
            "initialized" => self.respond(id, Value::Null),
            "shutdown" => {
                self.shutdown_requested = true;
                self.respond(id, Value::Null)
            }
            "textDocument/completion" => {
                let result = self.completion(&params);
                self.respond(id, result)
            }
            "textDocument/hover" => {
                let result = self.hover(&params);
                self.respond(id, result)
            }
            "textDocument/definition" => {
                let result = self.definition(&params);
                self.respond(id, result)
            }
            "textDocument/references" => {
                let result = self.references(&params);
                self.respond(id, result)
            }
            "$/setTrace" => self.respond(id, Value::Null),
            other => self.respond_error(id, -32601, format!("method not found: {other}")),
        }
    }

    fn handle_notification(&mut self, method: &str, params: Value) -> std::io::Result<()> {
        match method {
            "initialized" => {
                self.ensure_index(true);
                Ok(())
            }
            "textDocument/didOpen" => self.did_open(&params),
            "textDocument/didChange" => self.did_change(&params),
            "textDocument/didClose" => self.did_close(&params),
            "textDocument/didSave" => self.did_save(&params),
            "workspace/didChangeWatchedFiles" | "workspace/didChangeConfiguration" => {
                self.refresh_and_publish_all()
            }
            "exit" => {
                self.exit_requested = true;
                Ok(())
            }
            "$/cancelRequest" | "textDocument/willSave" | "textDocument/willSaveWaitUntil" => {
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn initialize(&mut self, params: &Value) -> Value {
        if let Some(socket) = params
            .pointer("/initializationOptions/socket")
            .and_then(Value::as_str)
        {
            if !socket.is_empty() {
                self.client = ServerClient::new(socket);
                self.index.clear();
                self.server_available = false;
                self.last_index_refresh = None;
            }
        }
        self.ensure_index(true);
        json!({
            "capabilities": {
                "positionEncoding": "utf-16",
                "textDocumentSync": {
                    "openClose": true,
                    "change": 1,
                    "save": false
                },
                "completionProvider": {
                    "triggerCharacters": ["[", "!", "`", "|", ","],
                    "resolveProvider": false
                },
                "hoverProvider": true,
                "definitionProvider": true,
                "referencesProvider": true
            },
            "serverInfo": {
                "name": "wiki-lsp",
                "version": env!("CARGO_PKG_VERSION")
            }
        })
    }

    fn did_open(&mut self, params: &Value) -> std::io::Result<()> {
        let Some((uri, wiki, text, version)) = document_open_params(params) else {
            return Ok(());
        };
        self.documents.insert(
            uri.clone(),
            Document {
                wiki,
                text,
                version,
            },
        );
        self.ensure_index(true);
        self.publish_diagnostics(&uri)
    }

    fn did_change(&mut self, params: &Value) -> std::io::Result<()> {
        let Some(uri) = params
            .pointer("/textDocument/uri")
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            return Ok(());
        };
        let version = params
            .pointer("/textDocument/version")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let changes = params
            .get("contentChanges")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let Some(document) = self.documents.get_mut(&uri) else {
            return Ok(());
        };
        for change in changes {
            let text = change.get("text").and_then(Value::as_str).unwrap_or("");
            let replacement = change.get("range").and_then(|range| {
                let start = range.get("start").and_then(parse_position)?;
                let end = range.get("end").and_then(parse_position)?;
                Some((start, end))
            });
            match replacement {
                Some((start, end)) => {
                    let mut start = text::offset_at(&document.text, start);
                    let mut end = text::offset_at(&document.text, end);
                    if start > end {
                        std::mem::swap(&mut start, &mut end);
                    }
                    document.text.replace_range(start..end, text);
                }
                None => {
                    document.text.clear();
                    document.text.push_str(text);
                }
            }
        }
        document.version = version;

        self.ensure_index(false);
        self.publish_diagnostics(&uri)
    }

    fn did_close(&mut self, params: &Value) -> std::io::Result<()> {
        let Some(uri) = params
            .pointer("/textDocument/uri")
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            return Ok(());
        };
        self.documents.remove(&uri);
        self.send(json!({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": { "uri": uri, "diagnostics": [] }
        }))
    }

    fn did_save(&mut self, params: &Value) -> std::io::Result<()> {
        let Some(uri) = params
            .pointer("/textDocument/uri")
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            return Ok(());
        };
        if !self.documents.contains_key(&uri) {
            return Ok(());
        }
        self.ensure_index(true);
        self.publish_diagnostics(&uri)
    }

    fn refresh_and_publish_all(&mut self) -> std::io::Result<()> {
        self.ensure_index(true);
        let uris = self.documents.keys().cloned().collect::<Vec<_>>();
        for uri in uris {
            self.publish_diagnostics(&uri)?;
        }
        Ok(())
    }

    fn completion(&mut self, params: &Value) -> Value {
        let Some((uri, position)) = document_position(params) else {
            return empty_completion();
        };
        self.ensure_index(false);
        let Some(document) = self.documents.get(&uri) else {
            return empty_completion();
        };
        let items = completion::complete(&document.text, position, &self.index);
        json!({ "isIncomplete": false, "items": items })
    }

    fn hover(&mut self, params: &Value) -> Value {
        let Some((uri, position)) = document_position(params) else {
            return Value::Null;
        };
        let Some(document) = self.documents.get(&uri) else {
            return Value::Null;
        };
        let byte = text::offset_at(&document.text, position);

        if let Some(link) = text::link_at(&document.text, byte) {
            let range = text::range_for_span(&document.text, link.start, link.end);
            let markdown = match self.lookup_summary(&link.target) {
                Some(summary) => hover::summary_markdown(&summary),
                None => format!(
                    "### `[[{}]]`\n\nNo Content matches this target, or `wiki-server` is unavailable.",
                    link.target.replace('`', "\\`")
                ),
            };
            return hover_response(markdown, range);
        }

        if let Some(call) = text::interpreter_at(&document.text, byte) {
            let range = text::range_for_span(&document.text, call.start, call.end);
            let markdown = match providers::find(&call.provider) {
                Some(provider) => hover::provider_markdown(provider),
                None => format!(
                    "### `{}`\n\nUnknown provider. Known providers: {}.",
                    call.provider,
                    providers::PROVIDERS
                        .iter()
                        .map(|provider| format!("`{}`", provider.name))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            };
            return hover_response(markdown, range);
        }

        if let Some(block) = text::fence_at(&document.text, byte) {
            let range = text::fence_info_range(&document.text, &block);
            let summary = block
                .spec
                .id
                .as_deref()
                .and_then(|id| self.lookup_summary(id));
            return hover_response(hover::fence_markdown(&block.spec, summary.as_ref()), range);
        }

        Value::Null
    }

    fn definition(&mut self, params: &Value) -> Value {
        let Some((uri, position)) = document_position(params) else {
            return json!([]);
        };
        let Some(document) = self.documents.get(&uri) else {
            return json!([]);
        };
        let byte = text::offset_at(&document.text, position);
        let Some(target) = text::link_at(&document.text, byte).map(|link| link.target) else {
            return json!([]);
        };
        match self.client.resolve_target(&target) {
            Ok(Some(summary)) => json!([Location {
                uri: uri::wiki_uri(&summary.id),
                range: Range::default(),
            }]),
            Ok(None) | Err(_) => json!([]),
        }
    }

    fn references(&mut self, params: &Value) -> Value {
        let Some((uri, position)) = document_position(params) else {
            return json!([]);
        };
        let Some(document) = self.documents.get(&uri) else {
            return json!([]);
        };
        let byte = text::offset_at(&document.text, position);

        let id = match text::link_at(&document.text, byte) {
            Some(link) => match self.client.resolve_target(&link.target) {
                Ok(Some(summary)) => summary.id,
                Ok(None) | Err(_) => return json!([]),
            },
            None => document.wiki.id.clone(),
        };

        match self.client.workspace_backlinks(&id) {
            Ok(backlinks) => {
                let locations = backlinks
                    .into_iter()
                    .map(|summary| Location {
                        uri: uri::wiki_uri(&summary.id),
                        range: Range::default(),
                    })
                    .collect::<Vec<_>>();
                json!(locations)
            }
            Err(_) => json!([]),
        }
    }

    fn lookup_summary(&self, target: &str) -> Option<ContentSummary> {
        let lowered = target.to_lowercase();
        if let Some(summary) = self.index.iter().find(|content| {
            content.id == target
                || content.title == target
                || content.title.to_lowercase() == lowered
        }) {
            return Some(summary.clone());
        }
        self.client.resolve_target(target).ok().flatten()
    }

    fn ensure_index(&mut self, force: bool) {
        if !force {
            if let Some(last) = self.last_index_refresh {
                if last.elapsed() < INDEX_TTL {
                    return;
                }
            }
        }
        self.last_index_refresh = Some(Instant::now());
        match self.client.list() {
            Ok(items) => {
                self.index = items;
                self.server_available = true;
            }
            Err(_) => {
                // Keep the previous index for completion, but never use it to
                // claim that a link is unresolved while the server is down.
                self.server_available = false;
            }
        }
    }

    fn publish_diagnostics(&mut self, uri: &str) -> std::io::Result<()> {
        let Some(document) = self.documents.get(uri) else {
            return Ok(());
        };
        let diagnostics = diagnostics::compute(
            &document.text,
            &document.wiki.id,
            &self.index,
            self.server_available,
        );
        let version = document.version;
        self.send(json!({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": {
                "uri": uri,
                "version": version,
                "diagnostics": diagnostics
            }
        }))
    }

    fn respond(&mut self, id: Value, result: Value) -> std::io::Result<()> {
        self.send(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
    }

    fn respond_error(&mut self, id: Value, code: i64, message: String) -> std::io::Result<()> {
        self.send(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": message }
        }))
    }

    fn send(&mut self, message: Value) -> std::io::Result<()> {
        framing::write_message(&mut self.writer, &message)
    }
}

fn hover_response(markdown: String, range: Range) -> Value {
    json!({
        "contents": { "kind": "markdown", "value": markdown },
        "range": range
    })
}

fn empty_completion() -> Value {
    json!({ "isIncomplete": false, "items": [] })
}

fn document_open_params(params: &Value) -> Option<(String, uri::WikiUri, String, i64)> {
    let text_document = params.get("textDocument")?;
    let uri = text_document.get("uri")?.as_str()?.to_owned();
    let wiki = uri::parse_wiki_uri(&uri)?;
    let text = text_document
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let version = text_document
        .get("version")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    Some((uri, wiki, text, version))
}

fn document_position(params: &Value) -> Option<(String, Position)> {
    let uri = params
        .pointer("/textDocument/uri")
        .and_then(Value::as_str)?
        .to_owned();
    let position = params.get("position").and_then(parse_position)?;
    Some((uri, position))
}

fn parse_position(value: &Value) -> Option<Position> {
    Some(Position {
        line: value.get("line")?.as_u64()? as u32,
        character: value.get("character")?.as_u64()? as u32,
    })
}
