//! Built-in Wiki providers. These are the local implementations behind the
//! `wiki/backlinks`, `wiki/outline` and `wiki/transclude` interpreter names.
//!
//! The platform-side registration/attach and the streaming v1 input contract
//! live in `provider_loop`; this module is the single implementation shared by
//! the local `provider_call` RPC and the attached Provider WebSocket loop.
//!
//! Callers pass the Wiki Content ID (and, for transclusion, the depth) in the
//! explicit invocation `input`. The node's selected Content snapshots are not
//! needed: the Wiki server already holds the startup snapshot of its own store,
//! and a Provider must never be handed read or write permission implicitly.

use serde_json::{Value, json};

use crate::error::{RpcError, RpcResult, optional_u64, required_str};
use crate::store::State;

/// The three provider names this app can serve locally.
pub const NAMES: [&str; 3] = ["wiki/backlinks", "wiki/outline", "wiki/transclude"];

/// The display contract for one provider call. The local RPC wraps this in its
/// JSON shape while the Provider WebSocket sends `data` + `Display` directly.
#[derive(Clone, Debug, PartialEq)]
pub struct ProviderOutput {
    pub data: Value,
    pub display: String,
    pub media_type: &'static str,
}

fn md_text(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace(['\n', '\r'], " ")
}

pub fn list() -> Value {
    json!({
        "providers": [
            {
                "name": "wiki/backlinks",
                "description": "Pages and snippets that link to a Wiki Content.",
                "params": [{"name": "id", "type": "string", "required": true}],
                "display": "text/markdown"
            },
            {
                "name": "wiki/outline",
                "description": "Headings of a Wiki Content with line numbers.",
                "params": [{"name": "id", "type": "string", "required": true}],
                "display": "text/markdown"
            },
            {
                "name": "wiki/transclude",
                "description": "Resolve a page's transclusions up to depth 3.",
                "params": [
                    {"name": "id", "type": "string", "required": true},
                    {"name": "depth", "type": "integer", "required": false, "default": 1}
                ],
                "display": "text/markdown"
            }
        ]
    })
}

/// Run one Wiki provider against the in-memory startup snapshot. Both the local
/// `provider_call` RPC and an attached Provider invocation use this function, so
/// their results cannot drift.
pub fn invoke(state: &State, name: &str, params: &Value) -> RpcResult<ProviderOutput> {
    let id = required_str(params, "id")?;
    match name {
        "wiki/backlinks" => {
            let workspace = state.workspace(&id)?;
            let items = workspace.links.backlinks;
            let mut display = format!(
                "### {} へのバックリンク\n\n",
                md_text(&workspace.page.title)
            );
            if items.is_empty() {
                display.push_str("(なし)\n");
            } else {
                for item in &items {
                    display.push_str(&format!(
                        "- [[{}|{}]] — {}\n",
                        item.id, item.title, item.excerpt
                    ));
                }
            }
            Ok(ProviderOutput {
                data: json!(items),
                display,
                media_type: "text/markdown",
            })
        }
        "wiki/outline" => {
            let workspace = state.workspace(&id)?;
            let items = workspace.outline;
            let mut display = format!("### {} のアウトライン\n\n", md_text(&workspace.page.title));
            for item in &items {
                display.push_str(&format!(
                    "{}- {} (line {})\n",
                    "  ".repeat(item.level.saturating_sub(1) as usize),
                    md_text(&item.title),
                    item.line
                ));
            }
            Ok(ProviderOutput {
                data: json!(items),
                display,
                media_type: "text/markdown",
            })
        }
        "wiki/transclude" => {
            let depth = optional_u64(params, "depth")?.unwrap_or(1).min(3) as u32;
            let response = state.transclude(&id, depth)?;
            let display = response.body.clone();
            Ok(ProviderOutput {
                data: json!(response),
                display,
                media_type: "text/markdown",
            })
        }
        other => Err(RpcError::not_found(format!(
            "unknown Wiki provider {other:?}"
        ))),
    }
}

/// Local `provider_call` result shape, kept for the existing Unix-socket API.
pub fn call(state: &State, name: &str, params: &Value) -> RpcResult<Value> {
    let output = invoke(state, name, params)?;
    Ok(json!({
        "display": output.display,
        "media_type": output.media_type,
        "data": output.data,
        "partial": false
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_the_three_wiki_providers() {
        let value = list();
        let names = value["providers"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|provider| provider["name"].as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, ["wiki/backlinks", "wiki/outline", "wiki/transclude"]);
    }

    #[test]
    fn outlines_a_fixture_page() {
        let state = State::load(None).unwrap();
        let value = call(&state, "wiki/outline", &json!({"id": "p-home"})).unwrap();
        assert!(
            value["display"]
                .as_str()
                .unwrap()
                .contains("買い物ジャーナル")
        );
        assert!(
            value["data"]
                .as_array()
                .is_some_and(|items| !items.is_empty())
        );
    }

    #[test]
    fn unknown_provider_is_not_found() {
        let state = State::load(None).unwrap();
        let error = call(&state, "wiki/nope", &json!({"id": "p-home"})).unwrap_err();
        assert_eq!(error.code, 404);
    }
}
