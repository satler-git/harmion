//! Dev-only node App API adapter.
//!
//! `serve --node DIR` uses `harmion-client::Client::from_directory` (which reads
//! the node's `admin.secret`) to load the `kind=wiki` read snapshot. Local reads
//! (`list`, `open`, `workspace`, `search`, `transclude`, `export`) then operate
//! on that snapshot.
//!
//! Writes are deliberately not wired in this M0/M3 slice. The adapter contains
//! compile-checked `get_content` / `save_body` calls for the future integration,
//! but every RPC mutation returns a 501 with an explicit reason. Converting a
//! local revision into a node edit token, grant/kind Write checks, request-id
//! replay and subscription/refresh are still missing; silently guessing would
//! risk data loss. Use the default fixture / `--data` backend for editing.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use harmion_client::protocol::{Command, EditToken, RequestId, SaveRequest, domain::ContentId};
use harmion_client::protocol::{domain::Field, domain::Value, query::Condition, unhex};
use harmion_client::{Client, random_id};
use serde_json::Value as Json;
use serde_json::json;

use crate::error::{RpcError, RpcResult};
use crate::model::Content;

pub struct NodeAdapter {
    dir: PathBuf,
    client: Client,
    runtime: tokio::runtime::Runtime,
}

impl NodeAdapter {
    pub fn connect(dir: impl AsRef<Path>) -> Result<Self, String> {
        let dir = dir.as_ref().to_path_buf();
        let client = Client::from_directory(&dir).map_err(|error| {
            format!(
                "cannot open node directory {} (connection.json + admin.secret): {error}",
                dir.display()
            )
        })?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("cannot build node runtime: {error}"))?;
        Ok(Self {
            dir,
            client,
            runtime,
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Load every `kind=wiki` Content visible to the admin credential. This is
    /// a startup snapshot; subscription catch-up is not implemented.
    pub fn load_contents(&self) -> Result<BTreeMap<String, Content>, String> {
        self.runtime.block_on(async {
            let mut contents = BTreeMap::new();
            let mut cursor = None;
            loop {
                let page = match cursor {
                    None => {
                        self.client
                            .query(Condition::Kind("wiki".to_owned()), 32)
                            .await
                    }
                    Some(cursor) => self.client.query_next(cursor, 32).await,
                }
                .map_err(|error| format!("node query failed: {error}"))?;
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
        })
    }

    /// Compile-checked read path for a single Content. Not called by the RPC
    /// layer yet because `load_contents` already materializes the snapshot.
    #[allow(dead_code)]
    pub fn get_content(&self, id: &str) -> RpcResult<Content> {
        let bytes = parse_content_id(id)?;
        let view = self
            .runtime
            .block_on(self.client.get(ContentId(bytes)))
            .map_err(|error| RpcError::new(502, format!("node get failed: {error}")))?;
        content_from_view(&view)
            .ok_or_else(|| RpcError::new(502, "node get returned an invalid Content view"))
    }

    /// Compile-checked write path. The future wiring must first decide how a
    /// local `rev` maps to the node's edit token and conflict set.
    #[allow(dead_code)]
    pub fn save_body(&self, id: &str, body: &str) -> RpcResult<Json> {
        let bytes = parse_content_id(id)?;
        let view = self
            .runtime
            .block_on(self.client.get(ContentId(bytes)))
            .map_err(|error| RpcError::new(502, format!("node get failed: {error}")))?;
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
            content_id: ContentId(bytes),
            command: Command::Edit {
                token: EditToken(token),
                fields,
            },
        };
        self.runtime
            .block_on(self.client.save(&request))
            .map_err(|error| {
                let code = match error {
                    harmion_client::Error::Http(409) => 409,
                    _ => 502,
                };
                RpcError::new(code, format!("node save failed: {error}"))
            })
    }

    pub fn write_unsupported(&self, method: &str) -> RpcError {
        RpcError::unsupported(format!(
            "node backend is a dev-only read snapshot; {method} is not wired"
        ))
        .with_data(json!({
            "backend": "node",
            "method": method,
            "node_dir": self.dir.display().to_string(),
            "reason": "local rev <-> node edit token mapping, grant/kind Write checks and subscription refresh are not implemented",
            "limitations": [
                "reads are a startup snapshot; no subscribe/resume catch-up",
                "create/convert/delete/save are disabled in node mode",
                "admin.secret is used directly; no OAuth refresh flow",
            ],
            "workaround": "run the fixture or --data backend for local editing",
        }))
    }
}

fn parse_content_id(id: &str) -> RpcResult<[u8; 32]> {
    unhex::<32>(id)
        .map_err(|_| RpcError::bad_request(format!("content id must be 64 hex characters: {id:?}")))
}

fn parse_32(value: &str) -> RpcResult<[u8; 32]> {
    unhex::<32>(value)
        .map_err(|_| RpcError::new(502, format!("node returned invalid 32-byte id: {value:?}")))
}

fn content_from_view(view: &Json) -> Option<Content> {
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
    }
}
