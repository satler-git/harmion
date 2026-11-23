//! Deterministic Markdown import. It accepts the `--raw` export layout:
//! frontmatter plus `pages/` and `snippets/` Markdown files. Imports are
//! keyed by a 64-hex frontmatter id, so re-running over the same tree updates
//! rather than duplicating Content.

use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::model::{Content, with_wiki_kind};
use crate::store::{new_content_id, now_micros, today_utc};

#[derive(Debug, Default, Serialize)]
pub struct ImportReport {
    pub imported: usize,
    pub updated: usize,
    pub skipped: usize,
    pub files: Vec<String>,
}

pub fn import(
    contents: &mut BTreeMap<String, Content>,
    dir: &Path,
) -> Result<ImportReport, String> {
    let mut files = Vec::new();
    collect(dir, &mut files).map_err(|error| format!("walk {}: {error}", dir.display()))?;
    files.sort();
    let mut report = ImportReport::default();
    for path in files {
        let raw = match fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(_) => {
                report.skipped += 1;
                continue;
            }
        };
        let Some(parsed) = parse(&raw, &path) else {
            report.skipped += 1;
            continue;
        };
        let path_string = path.display().to_string();
        report.files.push(path_string);
        if contents.contains_key(&parsed.id) {
            report.updated += 1;
        } else {
            report.imported += 1;
        }
        contents.insert(parsed.id.clone(), parsed);
    }
    Ok(report)
}

fn collect(dir: &Path, files: &mut Vec<std::path::PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect(&path, files)?;
        } else if path.file_name().is_some_and(|name| name != "README.md") {
            let is_markdown = path.extension().is_some_and(|extension| extension == "md");
            let is_snippet = path
                .parent()
                .and_then(|parent| parent.file_name())
                .is_some_and(|name| name == "snippets");
            if is_markdown || is_snippet {
                files.push(path);
            }
        }
    }
    Ok(())
}

fn parse(raw: &str, path: &Path) -> Option<Content> {
    let mut title = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("Imported")
        .to_owned();
    let mut id = None::<String>;
    let mut content_type = "page".to_owned();
    let mut tags = Vec::new();
    let mut aliases = Vec::new();
    let mut format = None::<String>;
    let mut language = None::<String>;
    let mut kinds = Vec::new();
    let mut body_start = 0usize;
    if let Some(rest) = raw.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---\n") {
            let frontmatter = &rest[..end];
            for line in frontmatter.lines() {
                let Some((key, value)) = line.split_once(':') else {
                    continue;
                };
                let value = value.trim().trim_matches('"').trim_matches('\'').to_owned();
                match key.trim() {
                    "id" | "harmion_id" if !value.is_empty() => id = Some(value),
                    "title" => title = value,
                    "type" => content_type = value,
                    "tags" => tags = list(&value),
                    "aliases" => aliases = list(&value),
                    "format" | "snippet_format" => format = Some(value),
                    "language" | "snippet_language" => language = Some(value),
                    "kinds" | "snippet_kinds" => kinds = list(&value),
                    _ => {}
                }
            }
            body_start = 4 + end + 5;
        }
    }
    let body = raw.get(body_start..).unwrap_or(raw).to_owned();
    if body.trim().is_empty() {
        return None;
    }
    let id = id.unwrap_or_else(new_content_id);
    let content_type = if content_type == "snippet" {
        "snippet"
    } else {
        "page"
    };
    let kinds = if content_type == "snippet" {
        with_wiki_kind(kinds)
    } else {
        vec!["wiki".to_owned()]
    };
    Some(Content {
        id,
        content_type: content_type.to_owned(),
        title,
        body,
        format,
        language,
        kinds,
        tags,
        aliases,
        rev: 1,
        created: today_utc(),
        updated_at: now_micros(),
        updated_label: "取り込み".to_owned(),
    })
}

fn list(value: &str) -> Vec<String> {
    value
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .map(|item| item.trim().trim_matches('"').trim_matches('\''))
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_frontmatter_and_ids() {
        let dir = std::env::temp_dir().join(format!("wiki-import-{}", std::process::id()));
        let pages = dir.join("pages");
        fs::create_dir_all(&pages).unwrap();
        fs::write(
            pages.join("Home.md"),
            "---\nid: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\ntitle: Home\ntags: [a, b]\n---\n# Home\n\nbody\n",
        )
        .unwrap();
        let mut contents = BTreeMap::new();
        let report = import(&mut contents, &dir).unwrap();
        assert_eq!(report.imported, 1);
        let content = contents.values().next().unwrap();
        assert_eq!(content.title, "Home");
        assert_eq!(content.tags, ["a", "b"]);
        assert_eq!(content.body, "# Home\n\nbody\n");
        let _ = fs::remove_dir_all(dir);
    }
}
