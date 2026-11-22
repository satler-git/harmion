//! Wire model for the wiki JSON-RPC server.
//!
//! The `Workspace` and `Summary` shapes are the M0 contract used by
//! `apps/wiki-gui`; do not add or remove their keys. `Content` gained optional
//! `tags` / `aliases` fields, but they are omitted from JSON when empty so the
//! existing fixture payload is byte-shape compatible.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Content {
    pub id: String,
    #[serde(rename = "type")]
    pub content_type: String,
    pub title: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default)]
    pub kinds: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    pub rev: u64,
    pub created: String,
    pub updated_at: i64,
    pub updated_label: String,
}

impl Content {
    pub fn fixture(id: &str, content_type: &str, title: &str, body: String) -> Self {
        Self {
            id: id.to_owned(),
            content_type: content_type.to_owned(),
            title: title.to_owned(),
            body,
            format: None,
            language: None,
            kinds: vec!["wiki".to_owned()],
            tags: Vec::new(),
            aliases: Vec::new(),
            rev: 1,
            created: "2026-10-02".to_owned(),
            updated_at: 0,
            updated_label: String::new(),
        }
    }

    pub fn is_page(&self) -> bool {
        self.content_type == "page"
    }

    pub fn is_snippet(&self) -> bool {
        self.content_type == "snippet"
    }

    /// Approximate serialized Content size used for the 1 MiB save limit.
    pub fn size_hint(&self) -> usize {
        self.title.len()
            + self.body.len()
            + self.format.as_deref().map_or(0, str::len)
            + self.language.as_deref().map_or(0, str::len)
            + self.kinds.iter().map(String::len).sum::<usize>()
            + self.tags.iter().map(String::len).sum::<usize>()
            + self.aliases.iter().map(String::len).sum::<usize>()
            + self.created.len()
            + self.updated_label.len()
            + 128
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub id: String,
    #[serde(rename = "type")]
    pub content_type: String,
    pub title: String,
    pub format: Option<String>,
    pub language: Option<String>,
    pub kinds: Vec<String>,
    pub rev: u64,
    pub updated_label: String,
    pub excerpt: String,
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
            updated_label: content.updated_label.clone(),
            excerpt: excerpt(&content.body),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct OutlineView {
    pub level: u8,
    pub title: String,
    pub line: usize,
}

impl From<harmion_wiki_core::OutlineItem> for OutlineView {
    fn from(item: harmion_wiki_core::OutlineItem) -> Self {
        Self {
            level: item.level,
            title: item.title,
            line: item.line,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TwoHopView {
    pub via: Summary,
    pub target: Summary,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LinksView {
    pub outgoing: Vec<Summary>,
    pub backlinks: Vec<Summary>,
    pub two_hop: Vec<TwoHopView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PageInfo {
    pub created: String,
    pub updated: String,
    pub word_count: usize,
    pub tags: String,
}

/// The GUI contract. Keep the key set and value types stable.
#[derive(Debug, Clone, Serialize)]
pub struct EmbedView {
    pub before: Vec<String>,
    pub snippet: Content,
}

#[derive(Debug, Clone, Serialize)]
pub struct Workspace {
    pub page: Content,
    pub before: Vec<String>,
    pub after: Vec<String>,
    pub snippet: Option<Content>,
    pub embeds: Vec<EmbedView>,
    pub outline: Vec<OutlineView>,
    pub links: LinksView,
    pub related: Vec<Summary>,
    pub recent: Vec<Summary>,
    pub info: PageInfo,
}

/// Strip headings, fences and the legacy `関連:` suffix for excerpt-like text.
pub fn clean_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty()
                && !line.starts_with('#')
                && !line.starts_with("!```")
                && !line.starts_with("```")
                && !line.starts_with("関連:")
        })
        .map(str::to_owned)
        .collect()
}

pub fn excerpt(body: &str) -> String {
    let line = clean_lines(body).into_iter().next().unwrap_or_default();
    let mut chars = line.chars();
    let excerpt = chars.by_ref().take(80).collect::<String>();
    if chars.next().is_some() {
        format!("{excerpt}…")
    } else {
        excerpt
    }
}

/// Add `wiki` first, preserve every requested kind, and remove duplicates.
pub fn with_wiki_kind(kinds: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out = vec!["wiki".to_owned()];
    for kind in kinds {
        let kind = kind.trim();
        if kind.is_empty() || kind == "wiki" {
            continue;
        }
        if !out.iter().any(|existing| existing == kind) {
            out.push(kind.to_owned());
        }
    }
    out
}
