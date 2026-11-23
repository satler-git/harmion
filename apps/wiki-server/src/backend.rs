//! Node App API backend for the Wiki server.
//!
//! `serve --node DIR` authenticates with an OAuth access token from
//! `<node>/credentials.json` (created by `wiki-server connect`), refreshes it
//! against the node's OAuth origin when needed, and loads every `kind=wiki`
//! Content through the pinned App API certificate.
//!
//! Writes are real App API operations: `Create` and `Edit` with a fresh
//! `request_id`, plus `Delete` for removals. A `401` triggers one refresh and
//! retry; a `410 token_expired` re-fetches the edit token and retries once. The
//! local conversion orchestration (loose fence -> snippet + canonical host) is
//! performed in `store.rs`; this module only maps the resulting Content writes.
//!
//! Not implemented: `update_meta` (the Wiki RPC has no node-backed metadata
//! editor yet), subscriptions/resume, and conflict resolution.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use harmion_client::protocol::{
    Command, ConnectionInfo, EditToken, RequestId, SaveRequest, domain::AttributeValue,
    domain::ContentId, domain::Field, domain::Number, domain::Value, query::Condition, unhex,
};
use harmion_client::{Client, Error as ClientError, OAuthTokenClient, random_id};
use serde_json::Value as Json;
use serde_json::json;

use crate::auth::{self, Credentials};
use crate::error::{RpcError, RpcResult};
use crate::model::Content;

pub struct NodeAdapter {
    dir: PathBuf,
    info: ConnectionInfo,
    credentials: Credentials,
    oauth_root: Option<Vec<u8>>,
    client: Client,
    runtime: std::sync::Arc<tokio::runtime::Runtime>,
}

impl NodeAdapter {
    /// Load credentials, refresh them if they are at or near expiry, and build
    /// the pinned App API client. A missing `credentials.json` is an actionable
    /// error: the caller must run `wiki-server connect --node DIR` first.
    pub fn connect(dir: impl AsRef<Path>) -> Result<Self, String> {
        let dir = dir.as_ref().to_path_buf();
        let info = auth::connection_info(&dir)?;
        let credentials = Credentials::load(&dir)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("cannot build node runtime: {error}"))?;
        let client = Client::new(info.clone(), credentials.access_bytes()?).map_err(|error| {
            format!(
                "cannot build the pinned App API client for {}: {error}",
                dir.display()
            )
        })?;
        let mut adapter = Self {
            dir,
            info,
            credentials,
            oauth_root: None,
            client,
            runtime: std::sync::Arc::new(runtime),
        };
        if adapter.credentials.needs_refresh(auth::now_seconds()) {
            adapter
                .refresh_credentials()
                .map_err(|error| error.message)?;
        }
        Ok(adapter)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Load every `kind=wiki` Content visible to the app credential. This is a
    /// startup snapshot; subscription catch-up is not implemented.
    pub fn load_contents(&mut self) -> Result<BTreeMap<String, Content>, String> {
        let runtime = self.runtime.clone();
        let mut contents = BTreeMap::new();
        let mut cursor = None;
        loop {
            let page = match cursor {
                None => self
                    .call(|client| {
                        runtime.block_on(client.query(Condition::Kind("wiki".to_owned()), 32))
                    })
                    .map_err(|error| error.message),
                Some(cursor) => self
                    .call(|client| runtime.block_on(client.query_next(cursor, 32)))
                    .map_err(|error| error.message),
            }?;
            for item in &page.items {
                if let Some(content) = content_from_view(item) {
                    contents.insert(content.id.clone(), content);
                }
            }
            match page.next {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        Ok(contents)
    }

    /// Body-only edit through `Edit` with a fresh `request_id`. The expected
    /// body is the caller's snapshot; a difference is reported as a conflict
    /// before any write. `410 token_expired` re-fetches the token once.
    pub fn save_body(&mut self, id: &str, body: &str, expected_body: &str) -> RpcResult<Json> {
        let content = parse_content_id(id)?;
        let mut token_retried = false;
        loop {
            let view = self.get_view(content)?;
            let current_body = view
                .pointer("/content/body")
                .and_then(Json::as_str)
                .unwrap_or_default();
            if current_body != expected_body {
                return Err(RpcError::conflict(
                    "the node Content changed since this snapshot; reload before saving",
                )
                .with_data(json!({
                    "type": "revision_conflict",
                    "id": id,
                    "field": {"type": "body"},
                    "reason": "node body differs from the local snapshot",
                    "resolution": "re-open the Content from the node snapshot and merge manually",
                })));
            }
            let token = view
                .get("edit_token")
                .and_then(Json::as_str)
                .ok_or_else(|| RpcError::new(502, "node view has no edit_token"))
                .and_then(parse_32)?;
            let mut fields = BTreeMap::new();
            fields.insert(Field::Body, Value::Body(body.to_owned()));
            let request = SaveRequest {
                request_id: RequestId(random_id().map_err(|error| {
                    RpcError::internal(format!("cannot create node request id: {error}"))
                })?),
                content_id: ContentId(content),
                command: Command::Edit {
                    token: EditToken(token),
                    fields,
                },
            };
            match self.save_request(&request) {
                Err(error) if error.code == 410 && !token_retried => {
                    // The edit token was reclaimed. Body is still checked against
                    // the caller's snapshot on the next loop iteration.
                    token_retried = true;
                    continue;
                }
                result => return result,
            }
        }
    }

    /// Create a page or snippet from the locally built Content. The local ID is
    /// the node Content ID; kind Write grants are checked before the request so
    /// a missing kind fails without creating any Content (§5.4, S29).
    pub fn create_content(&mut self, content: &Content) -> RpcResult<Content> {
        let content_id = parse_content_id(&content.id)?;
        let missing = self.credentials.missing_write_kinds(content.kinds.iter());
        if !missing.is_empty() {
            return Err(RpcError::new(
                403,
                format!(
                    "the node Write grant does not allow kind(s): {}",
                    missing.join(", ")
                ),
            )
            .with_data(json!({
                "type": "kind_write_denied",
                "missing_kinds": missing,
                "resolution": "re-run `wiki-server connect` with the required --write-kind values",
            })));
        }
        let fields = create_fields(content)?;
        let request = SaveRequest {
            request_id: RequestId(random_id().map_err(|error| {
                RpcError::internal(format!("cannot create node request id: {error}"))
            })?),
            content_id: ContentId(content_id),
            command: Command::Create { fields },
        };
        let receipt = self.save_request(&request)?;
        if let Some(view) = receipt.get("view").filter(|view| !view.is_null())
            && let Some(created) = content_from_view(view)
        {
            return Ok(created);
        }
        // A large response (view: null / view_error) is still a successful save.
        if let Some(view) = self.get_view(content_id).ok()
            && let Some(created) = content_from_view(&view)
        {
            return Ok(created);
        }
        Ok(content.clone())
    }

    /// Delete with the current edit token. The expected body is checked first so
    /// a stale snapshot cannot silently remove changed Content.
    pub fn delete_content(&mut self, id: &str, expected_body: &str) -> RpcResult<()> {
        let content = parse_content_id(id)?;
        let mut token_retried = false;
        loop {
            let view = self.get_view(content)?;
            let current_body = view
                .pointer("/content/body")
                .and_then(Json::as_str)
                .unwrap_or_default();
            if current_body != expected_body {
                return Err(RpcError::conflict(
                    "the node Content changed since this snapshot; reload before deleting",
                )
                .with_data(json!({
                    "type": "revision_conflict",
                    "id": id,
                    "field": {"type": "body"},
                    "reason": "node body differs from the local snapshot",
                })));
            }
            let token = view
                .get("edit_token")
                .and_then(Json::as_str)
                .ok_or_else(|| RpcError::new(502, "node view has no edit_token"))
                .and_then(parse_32)?;
            let request = SaveRequest {
                request_id: RequestId(random_id().map_err(|error| {
                    RpcError::internal(format!("cannot create node request id: {error}"))
                })?),
                content_id: ContentId(content),
                command: Command::Delete {
                    token: EditToken(token),
                },
            };
            match self.save_request(&request) {
                Err(error) if error.code == 410 && !token_retried => {
                    token_retried = true;
                    continue;
                }
                Ok(_) => return Ok(()),
                Err(error) => return Err(error),
            }
        }
    }

    pub fn update_meta(
        &mut self,
        id: &str,
        expected_body: &str,
        title: Option<&str>,
        tags: Option<&[String]>,
        aliases: Option<&[String]>,
    ) -> RpcResult<Json> {
        let content = parse_content_id(id)?;
        let view = self.get_view(content)?;
        let current_body = view
            .pointer("/content/body")
            .and_then(Json::as_str)
            .unwrap_or_default();
        if current_body != expected_body {
            return Err(RpcError::conflict(
                "the node Content changed since this snapshot",
            ));
        }
        let token = view
            .get("edit_token")
            .and_then(Json::as_str)
            .ok_or_else(|| RpcError::new(502, "node view has no edit_token"))
            .and_then(parse_32)?;
        let mut fields = BTreeMap::new();
        let mut attribute = |key: &str, value: AttributeValue| {
            fields.insert(
                Field::Attribute {
                    namespace: "wiki".to_owned(),
                    key: key.to_owned(),
                },
                Value::Attribute(value),
            );
        };
        if let Some(title) = title {
            attribute("title", AttributeValue::String(title.to_owned()));
        }
        if let Some(tags) = tags {
            attribute(
                "tags",
                AttributeValue::Array(tags.iter().cloned().map(AttributeValue::String).collect()),
            );
        }
        if let Some(aliases) = aliases {
            attribute(
                "aliases",
                AttributeValue::Array(
                    aliases
                        .iter()
                        .cloned()
                        .map(AttributeValue::String)
                        .collect(),
                ),
            );
        }
        let request = SaveRequest {
            request_id: RequestId(random_id().map_err(|error| {
                RpcError::internal(format!("cannot create node request id: {error}"))
            })?),
            content_id: ContentId(content),
            command: Command::Edit {
                token: EditToken(token),
                fields,
            },
        };
        self.save_request(&request)
    }

    pub fn write_unsupported(&self, method: &str) -> RpcError {
        RpcError::unsupported(format!("node backend does not implement {method} yet")).with_data(
            json!({
                "backend": "node",
                "method": method,
                "node_dir": self.dir.display().to_string(),
                "reason": "this method has no node-backed mapping in the Wiki JSON-RPC contract",
                "limitations": [
                    "reads are a startup snapshot; no subscribe/resume catch-up",
                    "update_meta has no node-backed metadata editor",
                    "conflict resolution is not exposed to apps",
                ],
            }),
        )
    }

    fn get_view(&mut self, content: [u8; 32]) -> RpcResult<Json> {
        let runtime = self.runtime.clone();
        self.call(move |client| runtime.block_on(client.get(ContentId(content))))
    }

    fn save_request(&mut self, request: &SaveRequest) -> RpcResult<Json> {
        let request = request.clone();
        let runtime = self.runtime.clone();
        self.call(move |client| runtime.block_on(client.save(&request)))
    }

    /// Run one App API call. A 401 refreshes the access token once and retries;
    /// every other error is mapped to an explicit JSON-RPC status.
    fn call<T>(&mut self, mut call: impl FnMut(&Client) -> Result<T, ClientError>) -> RpcResult<T> {
        // A long-running server must refresh before the 10-minute access token
        // expires, not only after a 401.
        if self.credentials.needs_refresh(auth::now_seconds()) {
            self.refresh_credentials()?;
        }
        let mut attempt = 0;
        loop {
            match call(&self.client) {
                Ok(value) => return Ok(value),
                Err(error) if auth::retry_after_unauthorized(attempt, &error) => {
                    attempt += 1;
                    self.refresh_credentials()?;
                }
                Err(error) => return Err(map_client_error(error, &self.dir)),
            }
        }
    }

    fn oauth_root(&mut self) -> RpcResult<Vec<u8>> {
        if self.oauth_root.is_none() {
            self.oauth_root = Some(
                auth::load_oauth_root(&self.dir).map_err(|message| RpcError::new(503, message))?,
            );
        }
        Ok(self.oauth_root.clone().unwrap_or_default())
    }

    /// Refresh and persist the rotated token pair before rebuilding the client.
    /// The refresh token changes on every successful use, so a failure to write
    /// the new credentials is fatal for this process.
    fn refresh_credentials(&mut self) -> RpcResult<()> {
        let origin = self.credentials.oauth_origin.clone().ok_or_else(|| {
            RpcError::new(
                401,
                format!(
                    "this node has no OAuth origin; enable `--oauth-listen` and run `wiki-server connect --node {}`",
                    self.dir.display()
                ),
            )
        })?;
        let root = self.oauth_root()?;
        let oauth = OAuthTokenClient::new(&origin, &root).map_err(|error| {
            RpcError::new(
                503,
                format!("cannot configure the node OAuth client: {error}"),
            )
        })?;
        let tokens = self
            .runtime
            .block_on(oauth.refresh(&self.credentials.client_id, &self.credentials.refresh_token))
            .map_err(|error| {
                let message = auth::refresh_failure_message(&self.dir, &error);
                let code = if matches!(
                    error,
                    ClientError::OAuth(ref code) if code == "invalid_grant"
                ) || matches!(error, ClientError::Http(400))
                {
                    401
                } else {
                    503
                };
                RpcError::new(code, message)
            })?;
        self.credentials.apply_refresh(tokens, auth::now_seconds());
        self.credentials.save(&self.dir).map_err(|message| {
            RpcError::new(
                500,
                format!("the node token was refreshed but could not be saved: {message}"),
            )
        })?;
        self.client = Client::new(
            self.info.clone(),
            self.credentials
                .access_bytes()
                .map_err(|message| RpcError::new(500, message))?,
        )
        .map_err(|error| RpcError::new(500, format!("cannot rebuild the node client: {error}")))?;
        Ok(())
    }
}

fn create_fields(content: &Content) -> RpcResult<BTreeMap<Field, Value>> {
    let mut fields = BTreeMap::new();
    fields.insert(Field::Body, Value::Body(content.body.clone()));
    for kind in &content.kinds {
        fields.insert(Field::Kind(kind.clone()), Value::Kind(true));
    }
    let mut attribute = |key: &str, value: AttributeValue| {
        fields.insert(
            Field::Attribute {
                namespace: "wiki".to_owned(),
                key: key.to_owned(),
            },
            Value::Attribute(value),
        );
    };
    attribute("type", AttributeValue::String(content.content_type.clone()));
    attribute("title", AttributeValue::String(content.title.clone()));
    if let Some(format) = &content.format {
        attribute("format", AttributeValue::String(format.clone()));
    }
    if let Some(language) = &content.language {
        attribute("language", AttributeValue::String(language.clone()));
    }
    attribute(
        "tags",
        AttributeValue::Array(
            content
                .tags
                .iter()
                .cloned()
                .map(AttributeValue::String)
                .collect(),
        ),
    );
    attribute(
        "aliases",
        AttributeValue::Array(
            content
                .aliases
                .iter()
                .cloned()
                .map(AttributeValue::String)
                .collect(),
        ),
    );
    attribute(
        "created_at",
        AttributeValue::String(content.created.clone()),
    );
    attribute(
        "updated_at",
        AttributeValue::Number(Number::from(content.updated_at)),
    );
    attribute(
        "updated_label",
        AttributeValue::String(content.updated_label.clone()),
    );
    Ok(fields)
}

fn parse_content_id(id: &str) -> RpcResult<[u8; 32]> {
    unhex::<32>(id)
        .map_err(|_| RpcError::bad_request(format!("content id must be 64 hex characters: {id:?}")))
}

fn parse_32(value: &str) -> RpcResult<[u8; 32]> {
    unhex::<32>(value)
        .map_err(|_| RpcError::new(502, format!("node returned invalid 32-byte id: {value:?}")))
}

fn map_client_error(error: ClientError, dir: &Path) -> RpcError {
    match error {
        ClientError::Http(401) => RpcError::new(
            401,
            format!(
                "node credentials expired or were revoked; run `wiki-server connect --node {}` again",
                dir.display()
            ),
        ),
        ClientError::Http(403) => RpcError::new(
            403,
            "the node denied this write; the app grant or kind Write permission is missing",
        ),
        ClientError::Http(409) => RpcError::new(
            409,
            "the node rejected the save (request_id reuse or a conflicting edit)",
        ),
        ClientError::Http(410) => RpcError::new(
            410,
            "the node edit token expired; reload the Content and retry",
        ),
        ClientError::Http(413) => RpcError::new(
            413,
            "the node rejected the Content: it exceeds the 1 MiB record limit",
        ),
        ClientError::Http(429) => {
            RpcError::new(429, "the node is at capacity; retry the operation shortly")
        }
        ClientError::Http(404) => RpcError::new(404, "the node Content was not found"),
        ClientError::Http(status) => {
            RpcError::new(502, format!("node App API returned HTTP {status}"))
        }
        ClientError::Timeout | ClientError::Transport => {
            RpcError::new(503, "the node is unreachable")
        }
        ClientError::OAuth(code) if code == "invalid_grant" => RpcError::new(
            401,
            format!(
                "node credentials expired or were revoked; run `wiki-server connect --node {}` again",
                dir.display()
            ),
        ),
        other => RpcError::new(502, format!("node App API call failed: {other}")),
    }
}

pub(crate) fn content_from_view(view: &Json) -> Option<Content> {
    let content = view.get("content")?;
    let id = content.get("id")?.as_str()?.to_owned();
    let body = content
        .get("body")
        .and_then(Json::as_str)
        .unwrap_or_default()
        .to_owned();
    let kinds = content
        .get("kinds")
        .and_then(Json::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Json::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let attributes = content
        .get("attributes")
        .and_then(Json::as_array)
        .cloned()
        .unwrap_or_default();
    let attr = |key: &str| {
        attributes.iter().find_map(|entry| {
            (entry.get("namespace").and_then(Json::as_str) == Some("wiki")
                && entry.get("key").and_then(Json::as_str) == Some(key))
            .then(|| entry.get("value"))
            .flatten()
        })
    };
    let attr_string = |key: &str| {
        attr(key).and_then(|value| match value {
            Json::String(text) => Some(text.clone()),
            Json::Number(number) => Some(number.to_string()),
            _ => None,
        })
    };
    let attr_strings = |key: &str| {
        attr(key)
            .and_then(Json::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Json::as_str)
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    let content_type = attr_string("type").unwrap_or_else(|| "page".to_owned());
    let title = attr_string("title").unwrap_or_else(|| id.clone());
    Some(Content {
        id,
        content_type,
        title,
        body,
        format: attr_string("format"),
        language: attr_string("language"),
        kinds,
        tags: attr_strings("tags"),
        aliases: attr_strings("aliases"),
        // The node has edit tokens, not local revisions. Reads are snapshot
        // only, so every loaded Content starts at revision 1.
        rev: 1,
        created: attr_string("created_at").unwrap_or_default(),
        updated_at: attr("updated_at").and_then(Json::as_i64).unwrap_or(0),
        updated_label: attr_string("updated_label").unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_wiki_attributes_from_a_read_view() {
        let view = json!({
            "content": {
                "id": "3f2a9c1e00000000000000000000000000000000000000000000000000000001",
                "body": "# Hi\n",
                "kinds": ["wiki", "ledger"],
                "attributes": [
                    {"namespace": "wiki", "key": "type", "value": "snippet"},
                    {"namespace": "wiki", "key": "title", "value": "Journal"},
                    {"namespace": "wiki", "key": "format", "value": "hledger"},
                    {"namespace": "wiki", "key": "language", "value": "ledger"},
                    {"namespace": "wiki", "key": "tags", "value": ["money"]},
                    {"namespace": "wiki", "key": "updated_at", "value": 123},
                    {"namespace": "other", "key": "title", "value": "ignored"}
                ]
            },
            "edit_token": "00",
            "conflicts": [],
            "deletion_conflict": false
        });
        let content = content_from_view(&view).unwrap();
        assert_eq!(content.content_type, "snippet");
        assert_eq!(content.title, "Journal");
        assert_eq!(content.format.as_deref(), Some("hledger"));
        assert_eq!(content.tags, ["money"]);
        assert_eq!(content.kinds, ["wiki", "ledger"]);
        assert_eq!(content.updated_at, 123);
    }

    #[test]
    fn create_fields_include_wiki_attributes_and_kinds() {
        let content = Content {
            id: "3f2a9c1e00000000000000000000000000000000000000000000000000000001".into(),
            content_type: "snippet".into(),
            title: "Journal".into(),
            body: "x".into(),
            format: Some("hledger".into()),
            language: Some("ledger".into()),
            kinds: vec!["wiki".into(), "ledger".into()],
            tags: vec!["money".into()],
            aliases: vec![],
            rev: 1,
            created: "2026-11-23".into(),
            updated_at: 5,
            updated_label: "now".into(),
        };
        let fields = create_fields(&content).unwrap();
        assert_eq!(fields[&Field::Body], Value::Body("x".into()));
        assert_eq!(fields[&Field::Kind("ledger".into())], Value::Kind(true));
        let title = Field::Attribute {
            namespace: "wiki".into(),
            key: "title".into(),
        };
        assert_eq!(
            fields[&title],
            Value::Attribute(AttributeValue::String("Journal".into()))
        );
        let tags = Field::Attribute {
            namespace: "wiki".into(),
            key: "tags".into(),
        };
        assert_eq!(
            fields[&tags],
            Value::Attribute(AttributeValue::Array(vec![AttributeValue::String(
                "money".into()
            )]))
        );
    }

    #[test]
    fn maps_explicit_http_failures_without_leaking_tokens() {
        let dir = Path::new("/tmp/node");
        assert_eq!(map_client_error(ClientError::Http(403), dir).code, 403);
        assert_eq!(map_client_error(ClientError::Http(410), dir).code, 410);
        assert_eq!(map_client_error(ClientError::Http(413), dir).code, 413);
        assert_eq!(map_client_error(ClientError::Http(429), dir).code, 429);
        assert_eq!(map_client_error(ClientError::Http(401), dir).code, 401);
        let error = map_client_error(ClientError::Transport, dir);
        assert_eq!(error.code, 503);
        assert!(!error.message.contains("1111"));
    }
}
