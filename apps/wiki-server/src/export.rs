//! Deterministic Markdown export.
//!
//! The same Content map and `raw` flag always produce identical paths and
//! bytes: no timestamps, stable ordering by Content ID, and collision suffixes
//! derived only from that ordering.
//!
//! - `raw=false` (default): transclusion and canonical embedded fences are
//!   expanded, `[[id:...]]` links become relative Markdown links, and snippets
//!   are also written as standalone files.
//! - `raw=true`: page bodies keep `![[...]]`, canonical fences and `[[id:...]]`
//!   so the tree can be re-imported; snippets are separate files.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use harmion_wiki_core::parse_links;
use serde_json::{Value, json};

use crate::error::{RpcError, RpcResult};
use crate::model::Content;
use crate::slug::slugify;
use crate::transclude::expanded_body;

#[derive(Debug, Clone)]
struct ExportFile {
    path: String,
    content: String,
}

pub fn export(
    contents: &BTreeMap<String, Content>,
    raw: bool,
    dir: Option<&Path>,
) -> RpcResult<Value> {
    let mut pages = contents
        .values()
        .filter(|c| c.is_page())
        .collect::<Vec<_>>();
    let mut snippets = contents
        .values()
        .filter(|c| c.is_snippet())
        .collect::<Vec<_>>();
    pages.sort_by(|a, b| a.id.cmp(&b.id));
    snippets.sort_by(|a, b| a.id.cmp(&b.id));

    let mut paths = BTreeMap::new();
    let mut used = BTreeSet::new();
    for content in pages.iter().copied() {
        let path = unique_path(
            format!("pages/{}.md", stem(&content.title, &content.id)),
            &mut used,
        );
        paths.insert(content.id.clone(), path);
    }
    for content in snippets.iter().copied() {
        let extension = extension_for(content.language.as_deref().or(content.format.as_deref()));
        let path = unique_path(
            format!(
                "snippets/{}.{}",
                stem(&content.title, &content.id),
                extension
            ),
            &mut used,
        );
        paths.insert(content.id.clone(), path);
    }

    let mut files = Vec::new();
    for content in pages.iter().copied() {
        let body = if raw {
            content.body.clone()
        } else {
            let (expanded, _) = expanded_body(contents, content, 3)?;
            rewrite_links(contents, &paths, &expanded)
        };
        files.push(ExportFile {
            path: paths[&content.id].clone(),
            content: format!("{}{}", frontmatter(content), body),
        });
    }
    for content in snippets.iter().copied() {
        files.push(ExportFile {
            path: paths[&content.id].clone(),
            content: format!("{}{}", frontmatter(content), content.body),
        });
    }

    let manifest = manifest(contents, &paths, raw);
    files.push(ExportFile {
        path: "manifest.json".to_owned(),
        content: format!(
            "{}\n",
            serde_json::to_string_pretty(&manifest).expect("manifest serializes")
        ),
    });
    files.push(ExportFile {
        path: "README.md".to_owned(),
        content: readme(raw, pages.len(), snippets.len()),
    });
    files.sort_by(|a, b| a.path.cmp(&b.path));

    if let Some(dir) = dir {
        for file in &files {
            let path = dir.join(&file.path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    RpcError::internal(format!(
                        "cannot create export directory {}: {error}",
                        parent.display()
                    ))
                })?;
            }
            fs::write(&path, &file.content).map_err(|error| {
                RpcError::internal(format!(
                    "cannot write export file {}: {error}",
                    path.display()
                ))
            })?;
        }
    }

    Ok(json!({
        "dir": dir.map(|dir| dir.display().to_string()),
        "raw": raw,
        "files": files
            .iter()
            .map(|file| json!({"path": file.path, "content": file.content}))
            .collect::<Vec<_>>(),
        "manifest": manifest,
    }))
}

fn manifest(
    contents: &BTreeMap<String, Content>,
    paths: &BTreeMap<String, String>,
    raw: bool,
) -> Value {
    let mut entries = contents
        .values()
        .filter_map(|content| {
            let path = paths.get(&content.id)?;
            Some(json!({
                "id": content.id,
                "type": content.content_type,
                "title": content.title,
                "path": path,
            }))
        })
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| {
        a["path"]
            .as_str()
            .cmp(&b["path"].as_str())
            .then_with(|| a["id"].as_str().cmp(&b["id"].as_str()))
    });
    json!({
        "format": "harmion-wiki-export",
        "version": 1,
        "raw": raw,
        "entries": entries,
    })
}

fn frontmatter(content: &Content) -> String {
    let mut out = String::from("---\n");
    out.push_str(&format!("harmion_id: {}\n", quoted(&content.id)));
    out.push_str(&format!("type: {}\n", content.content_type));
    out.push_str(&format!("title: {}\n", quoted(&content.title)));
    out.push_str(&format!("tags: {}\n", json!(content.tags)));
    out.push_str(&format!("aliases: {}\n", json!(content.aliases)));
    out.push_str(&format!("created_at: {}\n", quoted(&content.created)));
    out.push_str("schema: 1\n");
    if content.is_snippet() {
        if let Some(format) = &content.format {
            out.push_str(&format!("snippet_format: {}\n", quoted(format)));
        }
        if let Some(language) = &content.language {
            out.push_str(&format!("snippet_language: {}\n", quoted(language)));
        }
        out.push_str(&format!("snippet_kinds: {}\n", json!(content.kinds)));
    }
    out.push_str("---\n\n");
    out
}

fn quoted(value: &str) -> String {
    serde_json::to_string(value).expect("string serializes")
}

fn readme(raw: bool, pages: usize, snippets: usize) -> String {
    format!(
        "# Wiki export\n\n\
         - mode: {}\n\
         - pages: {pages}\n\
         - snippets: {snippets}\n\n\
         This tree was generated deterministically by `wiki-server`.\n\
         `manifest.json` records the Content ID of every file.\n",
        if raw { "raw" } else { "expanded" }
    )
}

fn unique_path(candidate: String, used: &mut BTreeSet<String>) -> String {
    if used.insert(candidate.clone()) {
        return candidate;
    }
    let (base, extension) = match candidate.rsplit_once('.') {
        Some((base, extension)) => (base, format!(".{extension}")),
        None => (candidate.as_str(), String::new()),
    };
    let mut index = 2usize;
    loop {
        let path = format!("{base}-{index}{extension}");
        if used.insert(path.clone()) {
            return path;
        }
        index += 1;
    }
}

fn stem(title: &str, id: &str) -> String {
    let short = id.chars().take(8).collect::<String>();
    format!("{}-{short}", slugify(title))
}

fn extension_for(language: Option<&str>) -> &'static str {
    match language.unwrap_or_default().to_ascii_lowercase().as_str() {
        "rust" | "rs" => "rs",
        "ledger" | "hledger" => "ledger",
        "python" | "py" => "py",
        "javascript" | "js" => "js",
        "typescript" | "ts" => "ts",
        "json" => "json",
        "sql" => "sql",
        "sh" | "bash" | "shell" => "sh",
        "toml" => "toml",
        "yaml" | "yml" => "yml",
        "markdown" | "md" => "md",
        _ => "txt",
    }
}

/// Rewrite canonical `[[id:...]]` links to relative paths. Embed links were
/// already expanded; unresolved and title-form links are left untouched.
fn rewrite_links(
    contents: &BTreeMap<String, Content>,
    paths: &BTreeMap<String, String>,
    body: &str,
) -> String {
    let mut links = parse_links(body)
        .into_iter()
        .filter(|link| !link.embed)
        .filter_map(|link| {
            let target = link.target.strip_prefix("id:")?;
            let (target, fragment) = match target.split_once('#') {
                Some((target, fragment)) => (target, Some(fragment)),
                None => (target, None),
            };
            let content = contents.get(target)?;
            let path = paths.get(&content.id)?;
            let href = if let Some(page) = path.strip_prefix("pages/") {
                page.to_owned()
            } else {
                format!("../{path}")
            };
            let href = match fragment {
                Some(fragment) => format!("{href}#{fragment}"),
                None => href,
            };
            let label = link.label.clone().unwrap_or_else(|| content.title.clone());
            Some((link.start, link.end, format!("[{label}]({href})")))
        })
        .collect::<Vec<_>>();
    links.sort_by_key(|(start, _, _)| *start);
    let mut out = String::with_capacity(body.len());
    let mut cursor = 0usize;
    for (start, end, replacement) in links {
        if start < cursor || end > body.len() {
            continue;
        }
        out.push_str(&body[cursor..start]);
        out.push_str(&replacement);
        cursor = end;
    }
    out.push_str(&body[cursor..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SNIPPET: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn map() -> BTreeMap<String, Content> {
        let mut page = Content::fixture(
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "page",
            "家計の記録",
            format!(
                "# 家計\n\n[[id:{SNIPPET}|ジャーナル]]\n\n!```hledger|ledger||{SNIPPET}\n```\n"
            ),
        );
        page.tags = vec!["money".to_owned()];
        let mut snippet = Content::fixture(
            SNIPPET,
            "snippet",
            "買い物ジャーナル",
            "2026-11-16 * x\n".to_owned(),
        );
        snippet.format = Some("hledger".to_owned());
        snippet.language = Some("ledger".to_owned());
        snippet.kinds = vec!["wiki".to_owned(), "ledger".to_owned()];
        [(page.id.clone(), page), (snippet.id.clone(), snippet)]
            .into_iter()
            .collect()
    }

    #[test]
    fn export_is_deterministic_and_expands_canonical_fences() {
        let contents = map();
        let first = export(&contents, false, None).unwrap();
        let second = export(&contents, false, None).unwrap();
        assert_eq!(first, second);
        let page = first["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|file| file["path"].as_str().unwrap().starts_with("pages/"))
            .unwrap();
        assert!(page["content"].as_str().unwrap().contains("```ledger"));
        assert!(page["content"].as_str().unwrap().contains("[ジャーナル]"));
        assert!(!page["content"].as_str().unwrap().contains("!```hledger"));
        let snippet = first["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|file| file["path"].as_str().unwrap().ends_with(".ledger"))
            .unwrap();
        assert!(
            snippet["content"]
                .as_str()
                .unwrap()
                .contains("snippet_format")
        );
    }

    #[test]
    fn raw_export_keeps_canonical_fence_and_links() {
        let contents = map();
        let result = export(&contents, true, None).unwrap();
        let page = result["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|file| file["path"].as_str().unwrap().starts_with("pages/"))
            .unwrap();
        let body = page["content"].as_str().unwrap();
        assert!(body.contains(&format!("!```hledger|ledger||{SNIPPET}")));
        assert!(body.contains(&format!("[[id:{SNIPPET}|ジャーナル]]")));
    }

    #[test]
    fn writes_the_tree_to_a_directory() {
        let root = std::env::temp_dir().join(format!(
            "wiki-export-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let result = export(&map(), false, Some(&root)).unwrap();
        assert_eq!(result["dir"], root.display().to_string());
        assert!(root.join("manifest.json").is_file());
        assert!(root.join("README.md").is_file());
        let manifest: Value =
            serde_json::from_str(&fs::read_to_string(root.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest["entries"].as_array().unwrap().len(), 2);
        let page = fs::read_dir(root.join("pages"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        assert!(fs::read_to_string(page.path()).unwrap().contains("家計"));
        let _ = fs::remove_dir_all(root);
    }
}
