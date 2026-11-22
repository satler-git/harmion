//! Local transclusion: expand `![[...]]` references and canonical embedded
//! snippet fences for previews, the `transclude` RPC and Markdown export.
//!
//! Rules:
//! - recursion is limited to depth 3 (the RPC rejects larger values),
//! - a reference back into the current expansion path becomes a visible
//!   `<!-- transclude cycle: ... -->` placeholder and never recurses,
//! - a missing reference becomes `<!-- transclude missing: ... -->`,
//! - canonical snippet fences are rendered as fenced code with `language`,
//! - when the depth budget runs out, the remaining source syntax is left
//!   untouched instead of being invented or dropped.

use std::collections::BTreeMap;

use harmion_wiki_core::{OutlineItem, WikiLink, outline, parse_links, scan_embedded_blocks};
use serde::Serialize;

use crate::error::{RpcError, RpcResult};
use crate::model::{Content, OutlineView};

pub const MAX_DEPTH: u32 = 3;

#[derive(Debug, Clone, Serialize)]
pub struct ChildView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub target: String,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub depth: u32,
    pub resolved: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TranscludeResponse {
    pub content: Content,
    /// Root body with references expanded according to `depth`.
    pub body: String,
    pub outline: Vec<OutlineView>,
    /// Every reference encountered, in document order, with its nesting depth.
    pub children: Vec<ChildView>,
    pub depth: u32,
}

/// Resolve `id:<hex>`, a Content ID, a title, or an alias.
pub fn resolve_target<'a>(
    contents: &'a BTreeMap<String, Content>,
    target: &str,
) -> Option<&'a Content> {
    let target = target.trim();
    let target = target.strip_prefix("id:").unwrap_or(target);
    if let Some(content) = contents.get(target) {
        return Some(content);
    }
    contents
        .values()
        .find(|content| content.title == target)
        .or_else(|| {
            contents
                .values()
                .find(|content| content.aliases.iter().any(|alias| alias == target))
        })
}

pub fn transclude(
    contents: &BTreeMap<String, Content>,
    id: &str,
    depth: u32,
) -> RpcResult<TranscludeResponse> {
    if depth > MAX_DEPTH {
        return Err(RpcError::bad_request(format!(
            "transclusion depth must be between 0 and {MAX_DEPTH}"
        )));
    }
    let content = contents
        .get(id)
        .cloned()
        .or_else(|| {
            contents
                .values()
                .find(|content| content.title == id)
                .cloned()
        })
        .ok_or_else(|| RpcError::not_found(format!("content {id:?} not found")))?;

    let mut children = Vec::new();
    let body = if depth == 0 {
        content.body.clone()
    } else {
        let mut path = vec![content.id.clone()];
        expand_body(contents, &content.body, depth, &mut path, 1, &mut children)?
    };
    let outline = outline(&body).into_iter().map(OutlineView::from).collect();
    Ok(TranscludeResponse {
        content,
        body,
        outline,
        children,
        depth,
    })
}

/// Expand one content body with a depth budget, used by export as well as the
/// RPC. `path` starts with the root content id.
pub fn expanded_body(
    contents: &BTreeMap<String, Content>,
    content: &Content,
    depth: u32,
) -> RpcResult<(String, Vec<ChildView>)> {
    if depth == 0 {
        return Ok((content.body.clone(), Vec::new()));
    }
    let mut children = Vec::new();
    let mut path = vec![content.id.clone()];
    let body = expand_body(contents, &content.body, depth, &mut path, 1, &mut children)?;
    Ok((body, children))
}

fn expand_body(
    contents: &BTreeMap<String, Content>,
    body: &str,
    remaining: u32,
    path: &mut Vec<String>,
    depth: u32,
    children: &mut Vec<ChildView>,
) -> RpcResult<String> {
    if remaining == 0 {
        return Ok(body.to_owned());
    }

    let blocks = scan_embedded_blocks(body).map_err(|error| {
        RpcError::bad_request(format!(
            "invalid embedded fence during transclusion: {error}"
        ))
    })?;
    let links = parse_links(body)
        .into_iter()
        .filter(|link| link.embed)
        .collect::<Vec<_>>();

    enum Event {
        Block(harmion_wiki_core::EmbeddedBlock),
        Link(WikiLink),
    }
    let mut events = Vec::with_capacity(blocks.len() + links.len());
    events.extend(blocks.into_iter().map(Event::Block));
    events.extend(links.into_iter().map(Event::Link));
    events.sort_by_key(|event| match event {
        Event::Block(block) => block.start,
        Event::Link(link) => link.start,
    });

    let mut out = String::with_capacity(body.len());
    let mut cursor = 0usize;
    for event in events {
        let (start, end) = match &event {
            Event::Block(block) => (block.start, block.end),
            Event::Link(link) => (link.start, link.end),
        };
        if start < cursor || end > body.len() || start >= end {
            continue;
        }
        out.push_str(&body[cursor..start]);
        cursor = end;
        match event {
            Event::Block(block) => {
                let Some(id) = block.spec.id.clone() else {
                    // A loose `!` fence has not been converted yet. Leave the
                    // authored text exactly as it is.
                    continue;
                };
                let target = format!("id:{id}");
                let Some(snippet) = contents.get(&id) else {
                    children.push(ChildView {
                        id: Some(id.clone()),
                        target,
                        content_type: None,
                        title: None,
                        depth,
                        resolved: false,
                        reason: Some("missing snippet".to_owned()),
                    });
                    out.push_str(&format!("<!-- transclude missing snippet: {id} -->"));
                    continue;
                };
                children.push(ChildView {
                    id: Some(snippet.id.clone()),
                    target,
                    content_type: Some(snippet.content_type.clone()),
                    title: Some(snippet.title.clone()),
                    depth,
                    resolved: true,
                    reason: None,
                });
                out.push_str(&render_code(&snippet.body, block.spec.language.as_str()));
            }
            Event::Link(link) => {
                let (target, section) = match link.target.split_once('#') {
                    Some((target, section)) => (target.trim(), Some(section.trim())),
                    None => (link.target.trim(), None),
                };
                let Some(found) = resolve_target(contents, target) else {
                    children.push(ChildView {
                        id: None,
                        target: link.target.clone(),
                        content_type: None,
                        title: None,
                        depth,
                        resolved: false,
                        reason: Some("missing content".to_owned()),
                    });
                    out.push_str(&format!("<!-- transclude missing: {} -->", link.target));
                    continue;
                };
                if path.iter().any(|id| id == &found.id) {
                    children.push(ChildView {
                        id: Some(found.id.clone()),
                        target: link.target.clone(),
                        content_type: Some(found.content_type.clone()),
                        title: Some(found.title.clone()),
                        depth,
                        resolved: false,
                        reason: Some("cycle".to_owned()),
                    });
                    out.push_str(&format!("<!-- transclude cycle: {} -->", found.id));
                    continue;
                }
                let selected = match section {
                    None => Some(found.body.clone()),
                    Some(section) if section.starts_with('^') => None,
                    Some(section) => section_body(&found.body, section),
                };
                let Some(selected) = selected else {
                    children.push(ChildView {
                        id: Some(found.id.clone()),
                        target: link.target.clone(),
                        content_type: Some(found.content_type.clone()),
                        title: Some(found.title.clone()),
                        depth,
                        resolved: false,
                        reason: Some(if section.is_some_and(|s| s.starts_with('^')) {
                            "block references are not implemented".to_owned()
                        } else {
                            "section not found".to_owned()
                        }),
                    });
                    out.push_str(&format!(
                        "<!-- transclude missing section: {} -->",
                        link.target
                    ));
                    continue;
                };
                children.push(ChildView {
                    id: Some(found.id.clone()),
                    target: link.target.clone(),
                    content_type: Some(found.content_type.clone()),
                    title: Some(found.title.clone()),
                    depth,
                    resolved: true,
                    reason: None,
                });
                let found_id = found.id.clone();
                path.push(found_id.clone());
                let expanded = expand_body(
                    contents,
                    &selected,
                    remaining.saturating_sub(1),
                    path,
                    depth + 1,
                    children,
                )?;
                path.pop();
                out.push_str(&expanded);
            }
        }
    }
    out.push_str(&body[cursor..]);
    Ok(out)
}

fn render_code(body: &str, language: &str) -> String {
    let longest = body
        .lines()
        .map(|line| {
            line.trim_start()
                .bytes()
                .take_while(|byte| *byte == b'`')
                .count()
        })
        .max()
        .unwrap_or(0);
    let fence = "`".repeat((longest + 1).max(3));
    let mut out = format!("{fence}{language}\n");
    out.push_str(body);
    if !body.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&fence);
    out.push('\n');
    out
}

/// Extract a Markdown section by heading title, including the heading line.
pub fn section_body(body: &str, heading: &str) -> Option<String> {
    let items: Vec<OutlineItem> = outline(body);
    let position = items.iter().position(|item| item.title == heading)?;
    let level = items[position].level;
    let start_line = items[position].line;
    let end_line = items[position + 1..]
        .iter()
        .find(|item| item.level <= level)
        .map(|item| item.line)
        .unwrap_or(usize::MAX);
    let lines = body.split_inclusive('\n').collect::<Vec<_>>();
    let start = start_line.saturating_sub(1).min(lines.len());
    let end = end_line.saturating_sub(1).min(lines.len()).max(start);
    Some(lines[start..end].concat())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content(id: &str, kind: &str, title: &str, body: &str) -> Content {
        Content::fixture(id, kind, title, body.to_owned())
    }

    fn contents(items: Vec<Content>) -> BTreeMap<String, Content> {
        items
            .into_iter()
            .map(|content| (content.id.clone(), content))
            .collect()
    }

    #[test]
    fn expands_canonical_snippet_fence_as_code() {
        let snippet_id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let mut snippet = content(snippet_id, "snippet", "Journal", "2026-11-16 * x\n");
        snippet.format = Some("hledger".to_owned());
        snippet.language = Some("ledger".to_owned());
        let page = content(
            "page",
            "page",
            "Page",
            &format!("# Page\n\n!```hledger|ledger||{snippet_id}\n```\n"),
        );
        let map = contents(vec![page, snippet]);
        let response = transclude(&map, "page", 3).unwrap();
        assert!(response.body.contains("```ledger\n2026-11-16 * x\n```"));
        assert_eq!(response.children.len(), 1);
        assert_eq!(
            response.children[0].content_type.as_deref(),
            Some("snippet")
        );
    }

    #[test]
    fn cycle_is_cut_with_a_placeholder() {
        let a = content("a", "page", "A", "A start\n![[b]]\nA end\n");
        let b = content("b", "page", "B", "B start\n![[a]]\nB end\n");
        let map = contents(vec![a, b]);
        let response = transclude(&map, "a", 3).unwrap();
        assert!(response.body.contains("B start"));
        assert!(response.body.contains("transclude cycle: a"));
        assert!(response.children.iter().any(|child| {
            child.id.as_deref() == Some("a") && child.reason.as_deref() == Some("cycle")
        }));
    }

    #[test]
    fn depth_one_leaves_nested_reference_source() {
        let a = content("a", "page", "A", "![[b]]\n");
        let b = content("b", "page", "B", "B body\n![[c]]\n");
        let c = content("c", "page", "C", "C body\n");
        let map = contents(vec![a, b, c]);
        let response = transclude(&map, "a", 1).unwrap();
        assert!(response.body.contains("B body"));
        assert!(response.body.contains("![[c]]"));
        assert_eq!(response.children.len(), 1);
    }

    #[test]
    fn depth_three_reaches_third_level() {
        let a = content("a", "page", "A", "![[b]]\n");
        let b = content("b", "page", "B", "B\n![[c]]\n");
        let c = content("c", "page", "C", "C\n![[d]]\n");
        let d = content("d", "page", "D", "D body\n");
        let map = contents(vec![a, b, c, d]);
        let response = transclude(&map, "a", 3).unwrap();
        assert!(response.body.contains("D body"));
        assert_eq!(response.children.len(), 3);
        assert!(response.depth == 3);
    }

    #[test]
    fn depth_zero_and_missing_references_are_safe() {
        let a = content("a", "page", "A", "![[missing]]\n");
        let map = contents(vec![a]);
        assert_eq!(transclude(&map, "a", 0).unwrap().body, "![[missing]]\n");
        let response = transclude(&map, "a", 1).unwrap();
        assert!(response.body.contains("transclude missing: missing"));
    }

    #[test]
    fn heading_transclusion_extracts_only_that_section() {
        let source = content(
            "source",
            "page",
            "Source",
            "# Top\n\n## One\n\none body\n\n## Two\n\ntwo body\n",
        );
        let page = content("page", "page", "Page", "![[source#One]]\n");
        let map = contents(vec![source, page]);
        let response = transclude(&map, "page", 1).unwrap();
        assert!(response.body.contains("one body"));
        assert!(!response.body.contains("two body"));
    }

    #[test]
    fn rejects_depth_above_three() {
        let map = contents(vec![content("a", "page", "A", "")]);
        assert_eq!(transclude(&map, "a", 4).unwrap_err().code, 400);
    }
}
