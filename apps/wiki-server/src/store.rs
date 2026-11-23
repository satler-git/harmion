//! In-memory Content store, optional atomic JSON persistence, CRUD, fence
//! conversion and the `Workspace` composition.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use harmion_wiki_core::{
    EmbeddedBlock, LinkIndex, canonical_block, outline, parse_links, scan_embedded_blocks,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::backend::NodeAdapter;
use crate::error::{RpcError, RpcResult};
use crate::export;
use crate::model::{
    Content, EmbedView, LinksView, OutlineView, PageInfo, Summary, TwoHopView, Workspace,
    clean_lines, excerpt, with_wiki_kind,
};
use crate::transclude;

const SAVE_LIMIT: usize = 1024 * 1024;
const STORE_VERSION: u32 = 1;

pub const JOURNAL_ID: &str = "3f2a9c1e00000000000000000000000000000000000000000000000000000001";
const BUDGET_ID: &str = "3f2a9c1e00000000000000000000000000000000000000000000000000000002";
const MONTHLY_ID: &str = "3f2a9c1e00000000000000000000000000000000000000000000000000000003";
const CREDIT_ID: &str = "3f2a9c1e00000000000000000000000000000000000000000000000000000004";
const POWER_ID: &str = "3f2a9c1e00000000000000000000000000000000000000000000000000000005";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FenceSelection {
    Span { start: usize, end: usize },
    Index(usize),
}

#[derive(Debug, Clone, Serialize)]
pub struct SaveReport {
    pub page: Content,
    pub converted: Vec<String>,
    pub failed: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConvertReport {
    pub snippet: Content,
    pub page: Content,
    pub before_body: String,
    pub after_body: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoreFile {
    version: u32,
    contents: Vec<Content>,
}

enum Backend {
    Memory,
    Json { path: PathBuf },
    Node(NodeAdapter),
}

pub struct State {
    contents: BTreeMap<String, Content>,
    backend: Backend,
    search: RefCell<Option<crate::search::SearchIndex>>,
    watch: Arc<(Mutex<u64>, Condvar)>,
    conflicts: RefCell<BTreeMap<String, Value>>,
}

impl State {
    /// Construct a server state from an optional `--data` path. The fallback is
    /// an in-memory fixture store; an existing file is loaded, and a new file is
    /// seeded atomically. (The per-Content `open` method below is the RPC read.)
    pub fn load(data: Option<PathBuf>) -> Result<Self, String> {
        let Some(path) = data else {
            return Ok(Self {
                contents: fixtures(),
                backend: Backend::Memory,
                search: RefCell::new(None),
                watch: Arc::new((Mutex::new(0), Condvar::new())),
                conflicts: RefCell::new(BTreeMap::new()),
            });
        };
        if path.exists() {
            let contents = load_store(&path)?;
            restrict_file(&path)
                .map_err(|error| format!("cannot set mode 0600 on {}: {error}", path.display()))?;
            return Ok(Self {
                contents,
                backend: Backend::Json { path },
                search: RefCell::new(None),
                watch: Arc::new((Mutex::new(0), Condvar::new())),
                conflicts: RefCell::new(BTreeMap::new()),
            });
        }
        let contents = fixtures();
        write_store(&path, &contents)
            .map_err(|error| format!("cannot seed {}: {error}", path.display()))?;
        Ok(Self {
            contents,
            backend: Backend::Json { path },
            search: RefCell::new(None),
            watch: Arc::new((Mutex::new(0), Condvar::new())),
            conflicts: RefCell::new(BTreeMap::new()),
        })
    }

    /// Node-backed store. Reads are a startup snapshot of `kind=wiki`; writes
    /// go through the App API with OAuth credentials from `connect`.
    pub fn open_node(dir: PathBuf) -> Result<Self, String> {
        let mut node = NodeAdapter::connect(&dir)?;
        let contents = node.load_contents()?;
        Ok(Self {
            contents,
            backend: Backend::Node(node),
            search: RefCell::new(None),
            watch: Arc::new((Mutex::new(0), Condvar::new())),
            conflicts: RefCell::new(BTreeMap::new()),
        })
    }

    #[cfg(test)]
    fn from_contents(contents: BTreeMap<String, Content>) -> Self {
        Self {
            contents,
            backend: Backend::Memory,
            search: RefCell::new(None),
            watch: Arc::new((Mutex::new(0), Condvar::new())),
            conflicts: RefCell::new(BTreeMap::new()),
        }
    }

    pub fn backend_name(&self) -> &'static str {
        match self.backend {
            Backend::Memory => "memory",
            Backend::Json { .. } => "json",
            Backend::Node(_) => "node",
        }
    }

    pub fn persistence_path(&self) -> Option<&Path> {
        match &self.backend {
            Backend::Json { path } => Some(path),
            _ => None,
        }
    }

    pub fn node_dir(&self) -> Option<&Path> {
        match &self.backend {
            Backend::Node(node) => Some(node.dir()),
            _ => None,
        }
    }

    pub fn status(&self) -> Value {
        let total = self.contents.len();
        let pages = self.contents.values().filter(|c| c.is_page()).count();
        let snippets = self.contents.values().filter(|c| c.is_snippet()).count();
        let snapshots = self
            .contents
            .values()
            .filter(|c| c.content_type == "snapshot")
            .count();
        json!({
            "version": env!("CARGO_PKG_VERSION"),
            "backend": self.backend_name(),
            "persistence": self.persistence_path().map(|path| path.display().to_string()),
            "node_dir": self.node_dir().map(|path| path.display().to_string()),
            "contents": {
                "total": total,
                "pages": pages,
                "snippets": snippets,
                "snapshots": snapshots,
            }
        })
    }

    pub fn list(&self) -> Vec<Summary> {
        let mut items = self
            .contents
            .values()
            .map(Summary::from)
            .collect::<Vec<_>>();
        items.sort_by(|a, b| {
            let a_updated = self
                .contents
                .get(&a.id)
                .map(|content| content.updated_at)
                .unwrap_or(0);
            let b_updated = self
                .contents
                .get(&b.id)
                .map(|content| content.updated_at)
                .unwrap_or(0);
            b_updated.cmp(&a_updated).then_with(|| a.id.cmp(&b.id))
        });
        items
    }

    pub fn open(&self, id: &str) -> RpcResult<Content> {
        self.contents
            .get(id)
            .cloned()
            .ok_or_else(|| RpcError::not_found(format!("content {id:?} not found")))
    }

    pub fn resolve(&self, target: &str) -> Option<&Content> {
        transclude::resolve_target(&self.contents, target)
    }

    pub fn workspace(&self, id: &str) -> RpcResult<Workspace> {
        let page = self.open(id)?;
        let blocks = scan_embedded_blocks(&page.body)
            .map_err(|error| RpcError::internal(format!("invalid page body: {error}")))?;
        let mut embeds = Vec::new();
        let mut cursor = 0usize;
        for block in blocks.iter().filter(|block| block.spec.id.is_some()) {
            let Some(snippet) = block
                .spec
                .id
                .as_deref()
                .and_then(|id| self.contents.get(id))
                .cloned()
            else {
                continue;
            };
            embeds.push(EmbedView {
                before: clean_lines(&page.body[cursor..block.start]),
                snippet,
            });
            cursor = block.end;
        }
        let before = embeds
            .first()
            .map(|embed| embed.before.clone())
            .unwrap_or_else(|| clean_lines(&page.body));
        let after = clean_lines(&page.body[cursor..]);
        let snippet = embeds.first().map(|embed| embed.snippet.clone());

        let index = LinkIndex::from_documents(
            self.contents
                .iter()
                .map(|(id, content)| (id.clone(), content.body.clone())),
        );
        let outgoing = parse_links(&page.body)
            .into_iter()
            .filter(|link| !link.embed)
            .filter_map(|link| self.resolve(&link.target).map(Summary::from))
            .collect::<Vec<_>>();
        let backlinks = index
            .backlinks(id)
            .iter()
            .filter_map(|id| self.contents.get(id).map(Summary::from))
            .collect::<Vec<_>>();
        let two_hop = index
            .two_hop(id)
            .into_iter()
            .filter_map(|hop| {
                let via = self.contents.get(&hop.via)?;
                let target = self.contents.get(&hop.target)?;
                Some(TwoHopView {
                    via: Summary::from(via),
                    target: Summary::from(target),
                    reason: excerpt(&target.body),
                })
            })
            .collect::<Vec<_>>();

        let outline = outline(&page.body)
            .into_iter()
            .map(OutlineView::from)
            .collect::<Vec<_>>();
        let page_kinds = page.kinds.iter().cloned().collect::<BTreeSet<_>>();
        let related = self
            .contents
            .values()
            .filter(|content| {
                content.content_type == "snippet"
                    && Some(&content.id) != snippet.as_ref().map(|snippet| &snippet.id)
                    && content
                        .kinds
                        .iter()
                        .any(|kind| kind != "wiki" && page_kinds.contains(kind))
            })
            .map(Summary::from)
            .collect::<Vec<_>>();
        let mut recent = self
            .contents
            .values()
            .map(Summary::from)
            .collect::<Vec<_>>();
        recent.sort_by(|a, b| {
            let a_updated = self.contents.get(&a.id).map(|c| c.updated_at).unwrap_or(0);
            let b_updated = self.contents.get(&b.id).map(|c| c.updated_at).unwrap_or(0);
            b_updated.cmp(&a_updated).then_with(|| a.id.cmp(&b.id))
        });
        recent.truncate(8);

        let tags = page.tags.join(", ");
        Ok(Workspace {
            before,
            after,
            snippet,
            embeds,
            outline,
            links: LinksView {
                outgoing,
                backlinks,
                two_hop,
            },
            related,
            recent,
            info: PageInfo {
                created: page.created.clone(),
                updated: page.updated_label.clone(),
                word_count: page.body.split_whitespace().count(),
                tags,
            },
            page,
        })
    }

    pub fn watch_since(&self, since: u64, timeout: Duration) -> u64 {
        let (generation, changed) = &*self.watch;
        let mut current = generation.lock().expect("watch lock");
        while *current == since {
            let (next, wait) = changed.wait_timeout(current, timeout).expect("watch lock");
            current = next;
            if wait.timed_out() {
                break;
            }
        }
        *current
    }

    pub fn watch_handle(&self) -> Arc<(Mutex<u64>, Condvar)> {
        self.watch.clone()
    }

    pub fn current_generation(&self) -> u64 {
        *self.watch.0.lock().expect("watch lock")
    }

    pub fn search(&self, query: &str) -> Vec<Summary> {
        if self.search.borrow().is_none() {
            if let Ok(index) = crate::search::SearchIndex::build(&self.contents) {
                *self.search.borrow_mut() = Some(index);
            }
        }
        if let Some(index) = self.search.borrow().as_ref() {
            if let Ok(ids) = index.search(query, 20) {
                let items = ids
                    .into_iter()
                    .filter_map(|id| self.contents.get(&id).map(Summary::from))
                    .collect::<Vec<_>>();
                if !items.is_empty() {
                    return items;
                }
            }
        }
        let needle = query.to_lowercase();
        let mut scored = self
            .contents
            .values()
            .filter_map(|content| {
                let score = search_score(content, &needle);
                (score > 0).then_some((score, content))
            })
            .collect::<Vec<_>>();
        scored.sort_by(|(a_score, a), (b_score, b)| {
            b_score
                .cmp(a_score)
                .then_with(|| b.updated_at.cmp(&a.updated_at))
                .then_with(|| a.id.cmp(&b.id))
        });
        scored
            .into_iter()
            .take(20)
            .map(|(_, content)| Summary::from(content))
            .collect()
    }

    pub fn create_page(
        &mut self,
        title: &str,
        body: String,
        tags: Vec<String>,
        aliases: Vec<String>,
    ) -> RpcResult<Content> {
        self.ensure_writable("create_page")?;
        let title = checked_title(title)?;
        let content = Content {
            id: self.fresh_id(),
            content_type: "page".to_owned(),
            title,
            body,
            format: None,
            language: None,
            kinds: vec!["wiki".to_owned()],
            tags: normalized_list(tags),
            aliases: normalized_list(aliases),
            rev: 1,
            created: today_utc(),
            updated_at: now_micros(),
            updated_label: "たった今".to_owned(),
        };
        check_size(&content)?;
        let id = content.id.clone();
        if let Backend::Node(node) = &mut self.backend {
            node.create_content(&content)?;
        }
        self.mutate(|contents| {
            contents.insert(id, content.clone());
            Ok(())
        })?;
        Ok(content)
    }

    pub fn create_snippet(
        &mut self,
        title: &str,
        body: String,
        format: Option<String>,
        language: Option<String>,
        kinds: Vec<String>,
    ) -> RpcResult<Content> {
        self.ensure_writable("create_snippet")?;
        let title = checked_title(title)?;
        let format = format.and_then(|value| {
            let value = value.trim();
            (!value.is_empty()).then(|| value.to_owned())
        });
        let mut requested_kinds = kinds;
        for kind in &requested_kinds {
            validate_kind(kind)?;
        }
        let (default_language, default_kinds) = match format.as_deref() {
            Some("hledger" | "ledger") => (Some("ledger".to_owned()), vec!["ledger".to_owned()]),
            Some(other) => (Some(other.to_owned()), Vec::new()),
            None => (None, Vec::new()),
        };
        if requested_kinds.is_empty() {
            requested_kinds = default_kinds;
        }
        let language = language
            .and_then(|value| {
                let value = value.trim();
                (!value.is_empty()).then(|| value.to_owned())
            })
            .or(default_language);
        let content = Content {
            id: self.fresh_id(),
            content_type: "snippet".to_owned(),
            title,
            body,
            format,
            language,
            kinds: with_wiki_kind(requested_kinds),
            tags: Vec::new(),
            aliases: Vec::new(),
            rev: 1,
            created: today_utc(),
            updated_at: now_micros(),
            updated_label: "たった今".to_owned(),
        };
        check_size(&content)?;
        let id = content.id.clone();
        if let Backend::Node(node) = &mut self.backend {
            node.create_content(&content)?;
        }
        self.mutate(|contents| {
            contents.insert(id, content.clone());
            Ok(())
        })?;
        Ok(content)
    }

    /// Convert exactly one loose `!` fenced block into a snippet + canonical
    /// host body. For the local JSON backend the page and snippet are committed
    /// together. On a node backend they are two real App API writes; if the
    /// snippet succeeds and the page edit fails, the snippet is kept and the
    /// error reports the partial result instead of silently rolling back a
    /// successful node write (§11, S24).
    pub fn convert_fence(
        &mut self,
        page_id: &str,
        selection: FenceSelection,
        title: Option<String>,
    ) -> RpcResult<ConvertReport> {
        self.ensure_writable("convert_fence")?;
        let before_page = self.open(page_id)?;
        if !before_page.is_page() {
            return Err(RpcError::bad_request(format!("{page_id:?} is not a page")));
        }
        let blocks = scan_embedded_blocks(&before_page.body).map_err(|error| {
            RpcError::bad_request(format!("invalid `!` fence in {page_id:?}: {error}"))
        })?;
        let block = match selection {
            FenceSelection::Span { start, end } => {
                let block = blocks
                    .iter()
                    .find(|block| block.start == start && block.end == end);
                match block {
                    Some(block) if block.spec.id.is_some() => {
                        return Err(RpcError::bad_request(
                            "selected fence is already canonical and cannot be converted again",
                        ));
                    }
                    Some(block) => block.clone(),
                    None => {
                        return Err(RpcError::not_found(format!(
                            "no `!` fenced block at bytes {start}..{end} in {page_id:?}"
                        )));
                    }
                }
            }
            FenceSelection::Index(index) => {
                let loose = blocks
                    .iter()
                    .filter(|block| block.spec.id.is_none())
                    .collect::<Vec<_>>();
                loose
                    .get(index)
                    .map(|block| (*block).clone())
                    .ok_or_else(|| {
                        RpcError::not_found(format!(
                            "no loose `!` fenced block at index {index} in {page_id:?}"
                        ))
                    })?
            }
        };

        let requested_kinds = with_wiki_kind(block.spec.kinds.clone());
        let existing = self
            .contents
            .values()
            .find(|content| {
                content.is_snippet()
                    && content.body == block.body
                    && content.format.as_deref() == Some(block.spec.preset.as_str())
                    && content.kinds == requested_kinds
            })
            .cloned();
        let snippet_title = title
            .and_then(|title| {
                let title = title.trim();
                (!title.is_empty()).then(|| title.to_owned())
            })
            .unwrap_or_else(|| derived_snippet_title(&block));
        let snippet_was_existing = existing.is_some();
        let snippet = existing.unwrap_or_else(|| Content {
            id: self.fresh_id(),
            content_type: "snippet".to_owned(),
            title: snippet_title,
            body: block.body.clone(),
            format: Some(block.spec.preset.clone()),
            language: Some(block.spec.language.clone()),
            // Every kind parsed from the fence is carried. The local backend
            // has no grant model, so it does not drop kinds; node mode rejects
            // the whole operation before reaching here.
            kinds: requested_kinds,
            tags: Vec::new(),
            aliases: Vec::new(),
            rev: 1,
            created: today_utc(),
            updated_at: now_micros(),
            updated_label: "たった今".to_owned(),
        });
        let canonical = canonical_block(&block.spec, &snippet.id);
        let after_body = format!(
            "{}{}{}",
            &before_page.body[..block.start],
            canonical,
            &before_page.body[block.end..]
        );
        let mut page = before_page.clone();
        page.body = after_body.clone();
        page.rev += 1;
        page.updated_at = now_micros();
        page.updated_label = "たった今".to_owned();
        check_size(&page)?;
        check_size(&snippet)?;

        let node_page_result = if let Backend::Node(node) = &mut self.backend {
            if !snippet_was_existing {
                node.create_content(&snippet)?;
            }
            Some(node.save_body(page_id, &page.body, &before_page.body))
        } else {
            None
        };
        if let Some(Err(error)) = node_page_result {
            if !snippet_was_existing {
                let snippet_id = snippet.id.clone();
                let kept = snippet.clone();
                // Keep the successful side visible; the host still holds the
                // loose fence so a later retry can reuse this snippet.
                self.mutate(|contents| {
                    contents.insert(snippet_id, kept);
                    Ok(())
                })?;
            }
            let mut data = error.data.clone().unwrap_or_else(|| json!({}));
            data["type"] = json!("partial_conversion");
            data["partial"] = json!(true);
            data["snippet_created"] = json!(!snippet_was_existing);
            data["snippet_id"] = json!(snippet.id);
            data["page_id"] = json!(page_id);
            data["node_error"] = json!(error.message);
            data["resolution"] =
                json!("the snippet was created; retry convert_fence to update the host page");
            return Err(RpcError::new(
                error.code,
                format!(
                    "snippet was saved but the host page update failed: {}",
                    error.message
                ),
            )
            .with_data(data));
        }

        let snippet_id = snippet.id.clone();
        self.mutate(|contents| {
            contents.insert(snippet_id, snippet.clone());
            contents.insert(page_id.to_owned(), page.clone());
            Ok(())
        })?;
        Ok(ConvertReport {
            snippet,
            page,
            before_body: before_page.body,
            after_body,
        })
    }

    fn set_conflict(&self, id: &str, value: Value) {
        self.conflicts.borrow_mut().insert(id.to_owned(), value);
    }

    fn clear_conflict(&self, id: &str) {
        self.conflicts.borrow_mut().remove(id);
    }

    pub fn save(&mut self, id: &str, rev: u64, body: String) -> RpcResult<Content> {
        self.ensure_writable("save")?;
        let current = self.open(id)?;
        if current.rev != rev {
            let conflict = json!({
                "type": "revision_conflict",
                "id": id,
                "provided_rev": rev,
                "current_rev": current.rev,
                "current_updated_label": current.updated_label,
                "fields": [{
                    "field": {"type": "body"},
                    "current": current.body,
                    "provided": body,
                }],
                "resolution": "review each field, merge manually, then save with current_rev",
                "ancestor_available": false,
            });
            self.set_conflict(id, conflict.clone());
            return Err(RpcError::conflict("revision conflict").with_data(conflict));
        }
        let mut content = current;
        let previous_body = content.body.clone();
        content.body = body;
        check_size(&content)?;
        if let Backend::Node(node) = &mut self.backend {
            node.save_body(id, &content.body, &previous_body)?;
        }
        content.rev += 1;
        content.updated_at = now_micros();
        content.updated_label = "たった今".to_owned();
        let saved = content.clone();
        self.mutate(|contents| {
            contents.insert(id.to_owned(), content.clone());
            Ok(())
        })?;
        self.clear_conflict(id);
        Ok(saved)
    }

    pub fn update_meta(
        &mut self,
        id: &str,
        rev: Option<u64>,
        title: Option<String>,
        tags: Option<Vec<String>>,
        aliases: Option<Vec<String>>,
    ) -> RpcResult<Content> {
        self.ensure_writable("update_meta")?;
        let mut content = self.open(id)?;
        if let Some(rev) = rev {
            if content.rev != rev {
                let fields = [
                    title.as_ref().map(|provided| {
                        json!({
                            "field": {"type": "title"},
                            "current": content.title,
                            "provided": provided,
                        })
                    }),
                    tags.as_ref().map(|provided| {
                        json!({
                            "field": {"type": "tags"},
                            "current": content.tags,
                            "provided": provided,
                        })
                    }),
                    aliases.as_ref().map(|provided| {
                        json!({
                            "field": {"type": "aliases"},
                            "current": content.aliases,
                            "provided": provided,
                        })
                    }),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
                let conflict = json!({
                    "type": "revision_conflict",
                    "id": id,
                    "provided_rev": rev,
                    "current_rev": content.rev,
                    "current_updated_label": content.updated_label,
                    "fields": fields,
                    "resolution": "review the changed fields, merge manually, then retry with current_rev",
                    "ancestor_available": false,
                });
                self.set_conflict(id, conflict.clone());
                return Err(RpcError::conflict("revision conflict").with_data(conflict));
            }
        }
        if let Backend::Node(node) = &mut self.backend {
            node.update_meta(
                id,
                &content.body,
                title.as_deref(),
                tags.as_deref(),
                aliases.as_deref(),
            )?;
        }
        if let Some(title) = title {
            content.title = checked_title(&title)?;
        }
        if let Some(tags) = tags {
            content.tags = normalized_list(tags);
        }
        if let Some(aliases) = aliases {
            content.aliases = normalized_list(aliases);
        }
        check_size(&content)?;
        content.rev += 1;
        content.updated_at = now_micros();
        content.updated_label = "たった今".to_owned();
        let saved = content.clone();
        self.mutate(|contents| {
            contents.insert(id.to_owned(), content.clone());
            Ok(())
        })?;
        self.clear_conflict(id);
        Ok(saved)
    }

    pub fn save_with_conversion(
        &mut self,
        id: &str,
        rev: u64,
        body: String,
    ) -> RpcResult<SaveReport> {
        let saved = self.save(id, rev, body)?;
        if !saved.is_page() {
            return Ok(SaveReport {
                page: saved,
                converted: Vec::new(),
                failed: Vec::new(),
            });
        }

        let mut converted = Vec::new();
        let mut failed = Vec::new();
        // Convert one loose fence at a time. Each conversion rewrites the host,
        // so re-scan instead of holding stale byte spans.
        loop {
            let page = self.open(id)?;
            let loose = match scan_embedded_blocks(&page.body) {
                Ok(blocks) => blocks
                    .into_iter()
                    .any(|block| block.spec.id.is_none() && !block.body.is_empty()),
                Err(error) => {
                    failed.push(format!("invalid `!` fence: {error}"));
                    break;
                }
            };
            if !loose {
                break;
            }
            match self.convert_fence(id, FenceSelection::Index(0), None) {
                Ok(report) => converted.push(report.snippet.id),
                Err(error) => {
                    failed.push(error.message);
                    break;
                }
            }
        }
        Ok(SaveReport {
            page: self.open(id)?,
            converted,
            failed,
        })
    }

    pub fn import_dir(&mut self, dir: &Path) -> RpcResult<crate::import::ImportReport> {
        self.ensure_writable("import")?;
        let mut candidate = self.contents.clone();
        let report = crate::import::import(&mut candidate, dir).map_err(RpcError::bad_request)?;
        self.mutate(|contents| {
            *contents = candidate;
            Ok(())
        })?;
        Ok(report)
    }

    pub fn delete(&mut self, id: &str) -> RpcResult<Content> {
        self.ensure_writable("delete")?;
        let content = self.open(id)?;
        if let Backend::Node(node) = &mut self.backend {
            node.delete_content(id, &content.body)?;
        }
        self.mutate(|contents| {
            contents.remove(id);
            Ok(())
        })?;
        self.clear_conflict(id);
        Ok(content)
    }

    pub fn delete_with_rev(&mut self, id: &str, rev: Option<u64>) -> RpcResult<Content> {
        if let Some(rev) = rev {
            let content = self.open(id)?;
            if content.rev != rev {
                return Err(RpcError::conflict("revision conflict").with_data(json!({
                    "type": "revision_conflict",
                    "id": id,
                    "field": {"type": "deletion"},
                    "provided_rev": rev,
                    "current_rev": content.rev,
                    "current_updated_label": content.updated_label,
                    "resolution": "reload the Content before deleting",
                })));
            }
        }
        self.delete(id)
    }

    pub fn conflicts(&self, id: &str) -> RpcResult<Value> {
        if let Some(conflict) = self.conflicts.borrow().get(id) {
            return Ok(conflict.clone());
        }
        let content = self
            .contents
            .get(id)
            .ok_or_else(|| RpcError::not_found(format!("content {id:?} not found")))?;
        Ok(json!({
            "id": id,
            "type": "content_snapshot",
            "rev": content.rev,
            "current_updated_label": content.updated_label,
            "supported": true,
            "deletion_conflict": false,
            "fields": [
                {"field": {"type": "body"}, "current": content.body},
                {"field": {"type": "title"}, "current": content.title},
                {"field": {"type": "tags"}, "current": content.tags},
                {"field": {"type": "aliases"}, "current": content.aliases},
            ],
            "note": "no conflict is pending; these are the current field values",
        }))
    }

    pub fn transclude(&self, id: &str, depth: u32) -> RpcResult<transclude::TranscludeResponse> {
        transclude::transclude(&self.contents, id, depth)
    }

    pub fn export(&self, raw: bool, dir: Option<&Path>) -> RpcResult<Value> {
        export::export(&self.contents, raw, dir)
    }

    fn ensure_writable(&self, method: &str) -> RpcResult<()> {
        match &self.backend {
            Backend::Node(node)
                if matches!(
                    method,
                    "save"
                        | "create_page"
                        | "create_snippet"
                        | "convert_fence"
                        | "delete"
                        | "update_meta"
                ) =>
            {
                Ok(())
            }
            Backend::Node(node) => Err(node.write_unsupported(method)),
            _ => Ok(()),
        }
    }

    fn mutate<T>(
        &mut self,
        operation: impl FnOnce(&mut BTreeMap<String, Content>) -> RpcResult<T>,
    ) -> RpcResult<T> {
        let mut candidate = self.contents.clone();
        let value = operation(&mut candidate)?;
        if let Backend::Json { path } = &self.backend {
            write_store(path, &candidate).map_err(|error| {
                RpcError::internal(format!(
                    "cannot persist {}: {error}; in-memory state was left unchanged",
                    path.display()
                ))
            })?;
        }
        self.contents = candidate;
        *self.search.borrow_mut() = None;
        let (generation, changed) = &*self.watch;
        *generation.lock().expect("watch lock") += 1;
        changed.notify_all();
        Ok(value)
    }

    fn fresh_id(&self) -> String {
        loop {
            let id = new_content_id();
            if !self.contents.contains_key(&id) {
                return id;
            }
        }
    }
}

fn checked_title(title: &str) -> RpcResult<String> {
    let title = title.trim();
    if title.is_empty() {
        return Err(RpcError::bad_request("title must not be empty"));
    }
    Ok(title.to_owned())
}

fn check_size(content: &Content) -> RpcResult<()> {
    if content.size_hint() > SAVE_LIMIT {
        return Err(RpcError::new(
            413,
            format!(
                "Content exceeds the 1 MiB save limit ({} bytes)",
                content.size_hint()
            ),
        ));
    }
    Ok(())
}

fn normalized_list(values: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for value in values {
        let value = value.trim();
        if !value.is_empty() && !out.iter().any(|existing| existing == value) {
            out.push(value.to_owned());
        }
    }
    out
}

fn validate_kind(kind: &str) -> RpcResult<()> {
    let kind = kind.trim();
    if kind.is_empty()
        || !kind
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'/'))
    {
        return Err(RpcError::bad_request(format!(
            "invalid kind {kind:?}; expected ASCII letters, digits, '_', '-', '.' or '/'"
        )));
    }
    Ok(())
}

fn search_score(content: &Content, needle: &str) -> u32 {
    if needle.is_empty() {
        return 1;
    }
    let title = content.title.to_lowercase();
    if title == needle {
        100
    } else if title.starts_with(needle) {
        80
    } else if title.contains(needle) {
        60
    } else if content
        .aliases
        .iter()
        .any(|alias| alias.to_lowercase().contains(needle))
    {
        50
    } else if content
        .tags
        .iter()
        .any(|tag| tag.to_lowercase().contains(needle))
        || content
            .kinds
            .iter()
            .any(|kind| kind.to_lowercase().contains(needle))
    {
        40
    } else if content
        .format
        .as_deref()
        .is_some_and(|format| format.to_lowercase().contains(needle))
        || content
            .language
            .as_deref()
            .is_some_and(|language| language.to_lowercase().contains(needle))
    {
        35
    } else if content.body.to_lowercase().contains(needle) {
        20
    } else {
        0
    }
}

fn derived_snippet_title(block: &EmbeddedBlock) -> String {
    let line = block
        .body
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    if line.is_empty() {
        format!("{} snippet", block.spec.preset)
    } else {
        let mut chars = line.chars();
        let title = chars.by_ref().take(60).collect::<String>();
        if chars.next().is_some() {
            format!("{title}…")
        } else {
            title
        }
    }
}

pub(crate) fn now_micros() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_micros().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

pub(crate) fn today_utc() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    let (year, month, day) = civil_from_days(seconds.div_euclid(86_400));
    format!("{year:04}-{month:02}-{day:02}")
}

/// Howard Hinnant's `civil_from_days`, avoiding a date dependency.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month as u32, day as u32)
}

fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

pub(crate) fn new_content_id() -> String {
    let mut bytes = [0u8; 32];
    if let Ok(mut file) = fs::File::open("/dev/urandom")
        && file.read_exact(&mut bytes).is_ok()
    {
        return to_hex(&bytes);
    }
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut seed = now_micros() as u64
        ^ u64::from(std::process::id())
        ^ COUNTER
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_mul(0x9e37_79b9_7f4a_7c15);
    for chunk in bytes.chunks_mut(8) {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        chunk.copy_from_slice(&seed.to_le_bytes()[..chunk.len()]);
    }
    to_hex(&bytes)
}

fn load_store(path: &Path) -> Result<BTreeMap<String, Content>, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let store: StoreFile = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid store {}: {error}", path.display()))?;
    if store.version != STORE_VERSION {
        return Err(format!(
            "unsupported store version {} in {} (expected {STORE_VERSION})",
            store.version,
            path.display()
        ));
    }
    let mut contents = BTreeMap::new();
    for content in store.contents {
        if contents.insert(content.id.clone(), content).is_some() {
            return Err(format!("duplicate Content id in {}", path.display()));
        }
    }
    Ok(contents)
}

fn restrict_file(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

fn write_store(path: &Path, contents: &BTreeMap<String, Content>) -> std::io::Result<()> {
    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
    let store = StoreFile {
        version: STORE_VERSION,
        contents: contents.values().cloned().collect(),
    };
    let mut bytes = serde_json::to_vec_pretty(&store).map_err(std::io::Error::other)?;
    bytes.push(b'\n');
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    if let Some(parent) = parent {
        fs::create_dir_all(parent)?;
    }
    let directory = parent.unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid data path")
        })?;
    let temp = directory.join(format!(
        ".{file_name}.tmp.{}.{}",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn fixtures() -> BTreeMap<String, Content> {
    let mut contents = BTreeMap::new();
    let mut push = |content: Content| {
        contents.insert(content.id.clone(), content);
    };

    let mut home = Content::fixture(
        "p-home",
        "page",
        "家計の記録",
        format!(
            "# 家計の記録\n\n> 固定費は先に把握する。 — 2026年の目標\n\n毎月の支出を hledger のジャーナルで管理している。\n\n## 買い物ジャーナル\n\n!```hledger|ledger,free||{JOURNAL_ID}\n```\n\n## 今月の残高\n\n固定費の見直しは月1回、日曜日にまとめて行う。\n\n## 固定費の見直し\n\n関連: [[p-goals|2026年の目標]] [[p-receipts|レシート処理]] [[p-fixed|固定費の見直し]]\n"
        ),
    );
    home.kinds = vec!["wiki".to_owned(), "ledger".to_owned()];
    home.tags = vec!["家計".to_owned(), "毎月".to_owned(), "ledger".to_owned()];
    home.created = "2026-10-02".to_owned();
    home.updated_at = 202611221431;
    home.updated_label = "14:31".to_owned();
    push(home);
    let mut demo = Content::fixture(
        "p-demo",
        "page",
        "Markdown 見本",
        "# Markdown 見本\n\n見出し・リスト・表・引用・コード・参照を確認するページ。\n\n## リスト\n\n- 見出し\n- リスト\n- 表\n\n## 表\n\n| 要素 | 状態 |\n|---|---|\n| Markdown | 描画 |\n| transclusion | 展開 |\n\n> 引用は本文から描画する。\n\n## コード\n\n```rust\nfn main() {\n    println!(\"wiki\");\n}\n```\n\n## 参照\n\n![[p-goals]]\n\n![wiki/outline()]\n".to_owned(),
    );
    demo.kinds = vec!["wiki".to_owned(), "reference".to_owned()];
    demo.tags = vec!["見本".to_owned(), "Markdown".to_owned()];
    demo.created = "2026-11-23".to_owned();
    demo.updated_at = 202611231200;
    demo.updated_label = "12:00".to_owned();
    push(demo);

    let mut goals = Content::fixture(
        "p-goals",
        "page",
        "2026年の目標",
        "# 2026年の目標\n\n家計の記録を毎週見直す。\n\n## 固定費\n\n- [[p-home|家計の記録]]\n- [[p-fixed|固定費の見直し]]\n- [[p-review|月次レビュー]]\n- [[p-reading|読書ノート]]\n- [[p-savings|貯金の目標]]\n".to_owned(),
    );
    goals.kinds = vec!["wiki".to_owned(), "ledger".to_owned()];
    goals.tags = vec!["目標".to_owned(), "家計".to_owned()];
    goals.created = "2026-10-03".to_owned();
    goals.updated_at = 202611211800;
    goals.updated_label = "昨日".to_owned();
    push(goals);

    let mut receipts = Content::fixture(
        "p-receipts",
        "page",
        "レシート処理",
        "# レシート処理\n\nレシートは週末にまとめて処理する。\n\n## 支払い方法\n\n- [[p-home|家計の記録]]\n- [[p-fixed|固定費の見直し]]\n- [[p-credit|クレジットカード]]\n- [[p-emoney|電子マネー]]\n- [[p-returns|返品・交換]]\n".to_owned(),
    );
    receipts.kinds = vec!["wiki".to_owned(), "ledger".to_owned()];
    receipts.tags = vec!["レシート".to_owned(), "家計".to_owned()];
    receipts.created = "2026-10-05".to_owned();
    receipts.updated_at = 202611201200;
    receipts.updated_label = "11/20".to_owned();
    push(receipts);

    let mut fixed = Content::fixture(
        "p-fixed",
        "page",
        "固定費の見直し",
        "# 固定費の見直し\n\n毎月の固定費を一覧にする。\n\n## 内訳\n\n- [[p-home|家計の記録]]\n- [[p-goals|2026年の目標]]\n- [[p-electricity|電気代]]\n- [[p-gas|ガス代]]\n- [[p-rent|家賃]]\n- [[p-phone|通信費]]\n- [[p-sub|サブスク]]\n".to_owned(),
    );
    fixed.kinds = vec!["wiki".to_owned(), "ledger".to_owned()];
    fixed.tags = vec!["固定費".to_owned(), "家計".to_owned()];
    fixed.created = "2026-10-06".to_owned();
    fixed.updated_at = 202611191100;
    fixed.updated_label = "11/19".to_owned();
    push(fixed);

    let mut review = Content::fixture(
        "p-review",
        "page",
        "月次レビュー",
        "# 月次レビュー\n\n1か月の支出を振り返り、翌月の予算を決める。\n\n- [[p-home|家計の記録]]\n- [[p-goals|2026年の目標]]\n".to_owned(),
    );
    review.kinds = vec!["wiki".to_owned(), "ledger".to_owned()];
    review.created = "2026-10-10".to_owned();
    review.updated_at = 202611201900;
    review.updated_label = "11/20".to_owned();
    push(review);

    let mut reading = Content::fixture(
        "p-reading",
        "page",
        "読書ノート",
        "# 読書ノート\n\n読んだ本の要点を残す。\n\n## 最近の本\n\n- [[p-goals|2026年の目標]]\n- [[p-rust|Rustのメモ]]\n".to_owned(),
    );
    reading.kinds = vec!["wiki".to_owned(), "note".to_owned()];
    reading.tags = vec!["読書".to_owned()];
    reading.created = "2026-10-12".to_owned();
    reading.updated_at = 202611182000;
    reading.updated_label = "昨日".to_owned();
    push(reading);

    let mut rust = Content::fixture(
        "p-rust",
        "page",
        "Rustのメモ",
        "# Rustのメモ\n\n所有権と借用のメモ。\n\n```rust\nfn main() {\n    let s = String::from(\"hello\");\n    println!(\"{s}\");\n}\n```\n\n- [[p-reading|読書ノート]]\n".to_owned(),
    );
    rust.kinds = vec!["wiki".to_owned(), "code".to_owned()];
    rust.tags = vec!["Rust".to_owned()];
    rust.created = "2026-10-15".to_owned();
    rust.updated_at = 202611171500;
    rust.updated_label = "11/17".to_owned();
    push(rust);

    for (id, title, updated_at, updated_label) in [
        ("p-credit", "クレジットカード", 202611191500, "11/19"),
        ("p-emoney", "電子マネー", 202611171300, "11/17"),
        ("p-returns", "返品・交換", 202611101000, "11/10"),
        ("p-electricity", "電気代", 202611161200, "11/16"),
        ("p-gas", "ガス代", 202611161100, "11/16"),
        ("p-rent", "家賃", 202610281000, "10/28"),
        ("p-phone", "通信費", 202611051000, "11/05"),
        ("p-sub", "サブスク", 202611141000, "11/14"),
        ("p-savings", "貯金の目標", 202610301000, "10/30"),
    ] {
        let parent = if id == "p-credit" || id == "p-emoney" || id == "p-returns" {
            "p-receipts"
        } else if id == "p-savings" {
            "p-goals"
        } else {
            "p-fixed"
        };
        let mut page = Content::fixture(
            id,
            "page",
            title,
            format!(
                "# {title}\n\n{title}のメモ。\n\n- [[{parent}|関連ページ]]\n- [[p-home|家計の記録]]\n"
            ),
        );
        page.kinds = vec!["wiki".to_owned(), "ledger".to_owned()];
        page.created = "2026-10-20".to_owned();
        page.updated_at = updated_at;
        page.updated_label = updated_label.to_owned();
        push(page);
    }

    let mut journal = Content::fixture(
        JOURNAL_ID,
        "snippet",
        "買い物ジャーナル",
        "2026-11-16 * 買い物\n    支出:食費   1,200 JPY\n    資産:現金  -1,200 JPY\n\n2026-11-17 * 電車\n    支出:交通     420 JPY\n    資産:現金    -420 JPY\n".to_owned(),
    );
    journal.format = Some("hledger".to_owned());
    journal.language = Some("ledger".to_owned());
    journal.kinds = vec!["wiki".to_owned(), "ledger".to_owned(), "free".to_owned()];
    journal.updated_at = 202611221431;
    journal.updated_label = "14:31".to_owned();
    push(journal);

    for (id, title, body, kinds, updated_at, updated_label) in [
        (
            BUDGET_ID,
            "予算クエリ",
            "2026-11-01 * 予算\n    支出:食費   30,000 JPY\n",
            vec!["wiki", "ledger"],
            202611181000,
            "11/18",
        ),
        (
            MONTHLY_ID,
            "月次サマリ",
            "2026-10-31 * 集計\n    支出:合計  82,400 JPY\n",
            vec!["wiki", "ledger"],
            202610311000,
            "10/31",
        ),
        (
            CREDIT_ID,
            "カード明細",
            "2026-11-19 * カード\n    支出:通信費  4,200 JPY\n",
            vec!["wiki", "ledger"],
            202611191500,
            "11/19",
        ),
        (
            POWER_ID,
            "電気代",
            "2026-11-16 * 電気\n    支出:電気代  6,800 JPY\n",
            vec!["wiki", "ledger"],
            202611161200,
            "11/16",
        ),
    ] {
        let mut snippet = Content::fixture(id, "snippet", title, body.to_owned());
        snippet.format = Some("hledger".to_owned());
        snippet.language = Some("ledger".to_owned());
        snippet.kinds = kinds.into_iter().map(str::to_owned).collect();
        snippet.updated_at = updated_at;
        snippet.updated_label = updated_label.to_owned();
        push(snippet);
    }

    contents
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn temp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "wiki-store-test-{}-{}-{name}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn sample_state() -> State {
        let mut page = Content::fixture(
            "p-test",
            "page",
            "Test page",
            "# Test\n\nbefore\n\n!```ledger\n2026-11-16 * x\n```\n\nafter\n".to_owned(),
        );
        page.updated_at = 1;
        State::from_contents([(page.id.clone(), page)].into_iter().collect())
    }

    #[test]
    fn persistence_round_trip_and_mode() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("wiki.json");
        let mut state = State::load(Some(path.clone())).unwrap();
        let created = state
            .create_page("Persisted", "body\n".to_owned(), vec![], vec![])
            .unwrap();
        drop(state);

        let state = State::load(Some(path.clone())).unwrap();
        assert_eq!(state.open(&created.id).unwrap().title, "Persisted");
        assert_eq!(state.backend_name(), "json");
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn existing_store_is_tightened_to_0600() {
        let dir = temp_dir("tighten");
        let path = dir.join("wiki.json");
        drop(State::load(Some(path.clone())).unwrap());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o644
        );

        let _state = State::load(Some(path.clone())).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn failed_persistence_leaves_memory_and_disk_unchanged() {
        let dir = temp_dir("failed-persist");
        let path = dir.join("wiki.json");
        let mut state = State::load(Some(path.clone())).unwrap();
        let before = state.open("p-home").unwrap();
        let count = state.list().len();

        // Replace the store file with a directory. The atomic temp file can no
        // longer be renamed over it, so the write must fail without touching
        // the in-memory map (or leaving a temp file behind).
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();

        let error = state
            .create_page("Must not land", "body\n".to_owned(), vec![], vec![])
            .unwrap_err();
        assert_eq!(error.code, 500);
        assert!(error.message.contains("cannot persist"));
        assert_eq!(state.list().len(), count);
        assert_eq!(state.open("p-home").unwrap().body, before.body);
        assert_eq!(state.open("p-home").unwrap().rev, before.rev);
        let leftovers = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp."))
            .count();
        assert_eq!(leftovers, 0);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn updates_title_and_tags() {
        let mut state = sample_state();
        let page = state.open("p-test").unwrap();
        let updated = state
            .update_meta(
                "p-test",
                None,
                Some("Renamed".to_owned()),
                Some(vec!["a".to_owned(), "a".to_owned(), "b".to_owned()]),
                None,
            )
            .unwrap();
        assert_eq!(updated.title, "Renamed");
        assert_eq!(updated.tags, ["a", "b"]);
        assert_eq!(updated.rev, page.rev + 1);
        assert_eq!(state.open("p-test").unwrap().title, "Renamed");

        let error = state
            .update_meta("p-test", Some(99), Some("Nope".to_owned()), None, None)
            .unwrap_err();
        assert_eq!(error.code, 409);
    }

    #[test]
    fn auto_convert_saves_page_and_snippets() {
        let mut state = sample_state();
        let page = state.open("p-test").unwrap();
        let report = state
            .save_with_conversion(
                "p-test",
                page.rev,
                "# Test\n\nbefore\n\n!```rust|code\nfn main() {}\n```\n\nafter\n".to_owned(),
            )
            .unwrap();
        assert_eq!(report.converted.len(), 1);
        assert!(report.failed.is_empty());
        assert!(report.page.body.contains("!```rust|code||"));
        let snippet = state.open(&report.converted[0]).unwrap();
        assert_eq!(snippet.content_type, "snippet");
        assert_eq!(snippet.body, "fn main() {}\n");
        assert!(snippet.kinds.contains(&"code".to_owned()));

        let again = state
            .save_with_conversion("p-test", report.page.rev, report.page.body.clone())
            .unwrap();
        assert!(again.converted.is_empty());
        assert!(again.failed.is_empty());
    }

    #[test]
    fn auto_convert_reuses_snippet_on_retry() {
        let mut state = sample_state();
        let page = state.open("p-test").unwrap();
        let loose = "# Test\n\n!```rust|code\nfn main() {}\n```\n".to_owned();
        let first = state
            .save_with_conversion("p-test", page.rev, loose.clone())
            .unwrap();
        let second = state
            .save_with_conversion("p-test", first.page.rev, loose)
            .unwrap();
        assert_eq!(second.converted, first.converted);
        let snippets = state
            .list()
            .into_iter()
            .filter(|summary| summary.content_type == "snippet")
            .count();
        assert_eq!(snippets, 1, "retry must not create an orphan snippet");
        assert!(second.page.body.contains("!```rust|code||"));
    }

    #[test]
    fn auto_convert_keeps_failed_fence_inline() {
        let mut state = sample_state();
        let page = state.open("p-test").unwrap();
        let body = "# Test\n\n!```ledger|bad kind\nx\n```\n".to_owned();
        let report = state
            .save_with_conversion("p-test", page.rev, body.clone())
            .unwrap();
        assert!(report.converted.is_empty());
        assert_eq!(report.failed.len(), 1);
        assert_eq!(state.open("p-test").unwrap().body, body);
    }

    #[test]
    fn create_page_and_snippet() {
        let mut state = sample_state();
        let page = state
            .create_page(
                " New page ",
                "# New\n".to_owned(),
                vec!["a".to_owned(), "a".to_owned()],
                vec!["alias".to_owned()],
            )
            .unwrap();
        assert_eq!(page.title, "New page");
        assert_eq!(page.content_type, "page");
        assert_eq!(page.kinds, ["wiki"]);
        assert_eq!(page.tags, ["a"]);
        assert_eq!(page.id.len(), 64);
        assert!(page.id.bytes().all(|byte| byte.is_ascii_hexdigit()));

        let snippet = state
            .create_snippet(
                "Ledger",
                "2026-11-16 * x\n".to_owned(),
                Some("hledger".to_owned()),
                None,
                vec![],
            )
            .unwrap();
        assert_eq!(snippet.content_type, "snippet");
        assert_eq!(snippet.format.as_deref(), Some("hledger"));
        assert_eq!(snippet.language.as_deref(), Some("ledger"));
        assert_eq!(snippet.kinds, ["wiki", "ledger"]);
        assert_eq!(state.open(&page.id).unwrap().title, "New page");
        assert_eq!(state.open(&snippet.id).unwrap().title, "Ledger");
    }

    #[test]
    fn fence_conversion_replaces_host_body_with_canonical_form() {
        let mut state = sample_state();
        let before = state.open("p-test").unwrap().body.clone();
        let report = state
            .convert_fence("p-test", FenceSelection::Index(0), None)
            .unwrap();
        assert_eq!(report.before_body, before);
        assert_eq!(report.snippet.body, "2026-11-16 * x\n");
        assert_eq!(report.snippet.format.as_deref(), Some("ledger"));
        assert_eq!(report.snippet.language.as_deref(), Some("ledger"));
        assert_eq!(report.snippet.kinds, ["wiki", "ledger"]);
        assert!(
            report
                .after_body
                .contains(&format!("!```ledger|ledger||{}", report.snippet.id))
        );
        assert!(!report.after_body.contains("2026-11-16 * x"));
        assert!(report.after_body.contains("before"));
        assert!(report.after_body.contains("after"));
        let page = state.open("p-test").unwrap();
        assert_eq!(page.rev, 2);
        assert_eq!(page.body, report.after_body);
        assert_eq!(
            state.open(&report.snippet.id).unwrap().body,
            report.snippet.body
        );
    }

    #[test]
    fn converting_the_same_fence_or_unknown_span_does_not_mutate() {
        let mut state = sample_state();
        let report = state
            .convert_fence("p-test", FenceSelection::Index(0), None)
            .unwrap();
        let snapshot = state.open("p-test").unwrap();
        assert_eq!(
            state
                .convert_fence("p-test", FenceSelection::Index(0), None)
                .unwrap_err()
                .code,
            404
        );
        assert_eq!(state.open("p-test").unwrap().rev, snapshot.rev);
        assert_eq!(state.open("p-test").unwrap().body, snapshot.body);

        let blocks = scan_embedded_blocks(&state.open("p-test").unwrap().body).unwrap();
        let block = blocks.iter().find(|block| block.spec.id.is_some()).unwrap();
        let error = state
            .convert_fence(
                "p-test",
                FenceSelection::Span {
                    start: block.start,
                    end: block.end,
                },
                None,
            )
            .unwrap_err();
        assert_eq!(error.code, 400);
        assert_eq!(state.open("p-test").unwrap().rev, snapshot.rev);
        assert_eq!(
            state.open(&report.snippet.id).unwrap().body,
            report.snippet.body
        );
    }

    #[test]
    fn conversion_keeps_every_explicit_kind() {
        let mut page = Content::fixture(
            "p-kinds",
            "page",
            "Kinds",
            "!```hledger|ledger,free\n2026-11-16 * x\n```\n".to_owned(),
        );
        page.updated_at = 1;
        let mut state = State::from_contents([(page.id.clone(), page)].into_iter().collect());
        let report = state
            .convert_fence("p-kinds", FenceSelection::Index(0), None)
            .unwrap();
        assert_eq!(report.snippet.kinds, ["wiki", "ledger", "free"]);
        assert!(
            report
                .after_body
                .contains(&format!("hledger|ledger,free||{}", report.snippet.id))
        );
    }

    #[test]
    fn conversion_changes_only_the_selected_loose_fence() {
        let body = "# Host\n\nintro\n\n!```rust|code\nfn one() {}\n```\n\nmiddle\n\n!```hledger|ledger,free\n2026-11-16 * x\n```\n\noutro\n";
        let mut page = Content::fixture("p-host", "page", "Host", body.to_owned());
        page.updated_at = 1;
        let mut state = State::from_contents([(page.id.clone(), page)].into_iter().collect());

        let first = state
            .convert_fence(
                "p-host",
                FenceSelection::Index(0),
                Some("Rust one".to_owned()),
            )
            .unwrap();
        assert_eq!(first.snippet.title, "Rust one");
        assert_eq!(first.snippet.body, "fn one() {}\n");
        assert_eq!(first.snippet.format.as_deref(), Some("rust"));
        assert_eq!(first.snippet.language.as_deref(), Some("rust"));
        assert_eq!(first.snippet.kinds, ["wiki", "code"]);
        let first_canonical = format!("!```rust|code||{}\n```\n", first.snippet.id);
        let expected = body.replacen("!```rust|code\nfn one() {}\n```\n", &first_canonical, 1);
        assert_eq!(first.after_body, expected);
        // The untouched loose fence and all surrounding host text survive.
        assert!(first.after_body.contains("intro\n\n"));
        assert!(first.after_body.contains("middle\n\n"));
        assert!(first.after_body.contains("outro\n"));
        assert!(
            first
                .after_body
                .contains("!```hledger|ledger,free\n2026-11-16 * x\n```\n")
        );

        let second = state
            .convert_fence("p-host", FenceSelection::Index(0), None)
            .unwrap();
        assert_eq!(second.snippet.body, "2026-11-16 * x\n");
        assert_eq!(second.snippet.format.as_deref(), Some("hledger"));
        assert_eq!(second.snippet.language.as_deref(), Some("ledger"));
        assert_eq!(second.snippet.kinds, ["wiki", "ledger", "free"]);
        let second_canonical = format!("!```hledger|ledger,free||{}\n```\n", second.snippet.id);
        assert!(second.after_body.contains(&first_canonical));
        assert!(second.after_body.contains(&second_canonical));
        assert!(!second.after_body.contains("!```rust|code\nfn one"));
        assert!(second.after_body.contains("middle\n\n"));
        assert_eq!(state.open("p-host").unwrap().rev, 3);
        assert_eq!(state.open("p-host").unwrap().body, second.after_body);
        assert_eq!(state.open(&first.snippet.id).unwrap().body, "fn one() {}\n");
    }

    #[test]
    fn workspace_returns_all_embeds_in_order() {
        let mut page = Content::fixture(
            "p-multi",
            "page",
            "Multi",
            "!```ledger||aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n```\n\nbetween\n\n!```ledger||bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\n```\n\nafter\n".to_owned(),
        );
        page.updated_at = 1;
        let snippet_a = Content::fixture(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "snippet",
            "A",
            "a\n".to_owned(),
        );
        let snippet_b = Content::fixture(
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "snippet",
            "B",
            "b\n".to_owned(),
        );
        let state = State::from_contents(
            [page, snippet_a, snippet_b]
                .into_iter()
                .map(|content| (content.id.clone(), content))
                .collect(),
        );
        let workspace = state.workspace("p-multi").unwrap();
        assert_eq!(workspace.embeds.len(), 2);
        assert_eq!(workspace.embeds[0].snippet.title, "A");
        assert_eq!(workspace.embeds[1].snippet.title, "B");
        assert!(
            workspace.embeds[1]
                .before
                .iter()
                .any(|line| line == "between")
        );
        assert!(workspace.after.iter().any(|line| line == "after"));
    }

    #[test]
    fn workspace_keeps_the_gui_contract_shape() {
        let state = State::load(None).unwrap();
        let value = serde_json::to_value(state.workspace("p-home").unwrap()).unwrap();
        let keys = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        assert_eq!(
            keys,
            [
                "after", "before", "embeds", "info", "links", "outline", "page", "recent",
                "related", "snippet"
            ]
        );
        let link_keys = value["links"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        assert_eq!(link_keys, ["backlinks", "outgoing", "two_hop"]);
        assert_eq!(value["page"]["type"], "page");
        assert_eq!(value["snippet"]["type"], "snippet");
        assert!(value["outline"].as_array().unwrap().len() >= 2);
        assert!(value["info"]["word_count"].as_u64().unwrap() > 0);
        assert!(value["recent"].as_array().unwrap().len() <= 8);
        assert_eq!(state.workspace("missing").unwrap_err().code, 404);
    }

    #[test]
    fn search_finds_japanese_titles_and_snippets() {
        let state = State::load(None).unwrap();
        let hits = state.search("電気");
        assert!(hits.iter().any(|item| item.id == "p-electricity"));
        assert!(hits.iter().any(|item| item.id == super::POWER_ID));
    }

    #[test]
    fn search_matches_title_body_and_filters() {
        let mut state = sample_state();
        let created = state
            .create_snippet(
                "Zebra ledger",
                "unique-body-token\n".to_owned(),
                Some("ledger".to_owned()),
                None,
                vec![],
            )
            .unwrap();
        assert_eq!(state.search("ZEBRA")[0].id, created.id);
        assert_eq!(state.search("unique-body-token")[0].id, created.id);
        assert!(
            state
                .search("ledger")
                .iter()
                .any(|item| item.id == created.id)
        );
        assert!(state.search("does-not-exist").is_empty());
    }

    #[test]
    fn delete_removes_content() {
        let mut state = sample_state();
        let created = state
            .create_page("Gone", "bye\n".to_owned(), vec![], vec![])
            .unwrap();
        let deleted = state.delete(&created.id).unwrap();
        assert_eq!(deleted.id, created.id);
        assert_eq!(state.open(&created.id).unwrap_err().code, 404);
        assert_eq!(state.delete(&created.id).unwrap_err().code, 404);
        assert!(!state.list().iter().any(|item| item.id == created.id));
    }

    #[test]
    fn save_reports_structured_conflict_and_keeps_content() {
        let mut state = sample_state();
        let error = state.save("p-test", 99, "nope\n".to_owned()).unwrap_err();
        assert_eq!(error.code, 409);
        let data = error.data.unwrap();
        assert_eq!(data["type"], "revision_conflict");
        assert_eq!(data["provided_rev"], 99);
        assert_eq!(data["current_rev"], 1);
        assert_eq!(
            state.open("p-test").unwrap().body,
            "# Test\n\nbefore\n\n!```ledger\n2026-11-16 * x\n```\n\nafter\n"
        );

        let saved = state.save("p-test", 1, "# changed\n".to_owned()).unwrap();
        assert_eq!(saved.rev, 2);
        assert_eq!(state.open("p-test").unwrap().body, "# changed\n");
    }

    #[test]
    fn conflicts_is_a_placeholder_with_current_rev() {
        let state = sample_state();
        let value = state.conflicts("p-test").unwrap();
        assert_eq!(value["supported"], true);
        assert_eq!(value["rev"], 1);
        assert_eq!(value["type"], "content_snapshot");
        assert_eq!(value["fields"].as_array().unwrap().len(), 4);
        assert_eq!(state.conflicts("missing").unwrap_err().code, 404);
    }

    #[test]
    fn transclude_wrapper_rejects_depth_above_three() {
        let state = sample_state();
        assert_eq!(state.transclude("p-test", 4).unwrap_err().code, 400);
        assert!(state.transclude("p-test", 3).is_ok());
    }

    #[test]
    fn watch_generation_advances_on_mutation() {
        let mut state = State::load(None).unwrap();
        let generation = state.current_generation();
        let watch = state.watch_handle();
        let waiter = std::thread::spawn(move || {
            let (lock, changed) = &*watch;
            let mut current = lock.lock().unwrap();
            while *current == generation {
                let (next, _) = changed
                    .wait_timeout(current, Duration::from_secs(2))
                    .unwrap();
                current = next;
            }
            *current
        });
        std::thread::sleep(Duration::from_millis(20));
        state
            .mutate(|contents| {
                contents
                    .get_mut("p-home")
                    .unwrap()
                    .tags
                    .push("watch".to_owned());
                Ok(())
            })
            .unwrap();
        assert!(waiter.join().unwrap() > generation);
    }

    fn raw_export_import_round_trip() {
        let source = State::load(None).unwrap();
        let dir = temp_dir("roundtrip-import");
        source.export(true, Some(&dir)).unwrap();
        let mut target = State::from_contents(BTreeMap::new());
        let report = target.import_dir(&dir).unwrap();
        assert!(report.imported >= 20, "{report:?}");
        assert_eq!(target.contents.len(), source.contents.len());
        assert_eq!(
            target.open("p-home").unwrap().title,
            source.open("p-home").unwrap().title
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    #[test]
    fn save_conflict_exposes_field_candidates() {
        let mut state = State::load(None).unwrap();
        let current = state.open("p-home").unwrap();
        let error = state
            .save("p-home", current.rev.saturating_sub(1), "mine\n".to_owned())
            .unwrap_err();
        assert_eq!(error.code, 409);
        let conflict = state.conflicts("p-home").unwrap();
        assert_eq!(conflict["type"], "revision_conflict");
        let field = &conflict["fields"][0];
        assert_eq!(field["field"]["type"], "body");
        assert_eq!(field["provided"], "mine\n");
        assert_eq!(field["current"], current.body);
        state
            .save("p-home", current.rev, "merged\n".to_owned())
            .unwrap();
        assert_eq!(
            state.conflicts("p-home").unwrap()["type"],
            "content_snapshot"
        );
    }

    #[test]
    fn metadata_conflict_reports_title_and_tags() {
        let mut state = State::load(None).unwrap();
        let current = state.open("p-home").unwrap();
        let error = state
            .update_meta(
                "p-home",
                Some(current.rev.saturating_sub(1)),
                Some("mine".to_owned()),
                Some(vec!["mine".to_owned()]),
                None,
            )
            .unwrap_err();
        assert_eq!(error.code, 409);
        let conflict = state.conflicts("p-home").unwrap();
        let fields = conflict["fields"].as_array().unwrap();
        assert!(fields.iter().any(|field| field["field"]["type"] == "title"));
        assert!(fields.iter().any(|field| field["field"]["type"] == "tags"));
        assert_eq!(fields[0]["provided"], "mine");
    }

    fn status_counts_all_content_types() {
        let state = sample_state();
        let status = state.status();
        assert_eq!(status["backend"], "memory");
        assert_eq!(status["contents"]["total"], 1);
        assert_eq!(status["contents"]["pages"], 1);
        assert_eq!(status["contents"]["snippets"], 0);
        assert!(status["version"].as_str().unwrap().contains('.'));
    }
}
