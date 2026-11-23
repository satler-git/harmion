#![allow(dead_code)]

//! Harmion Wiki GUI — the frozen v5 shell.
//!
//! The GUI is a reading surface: a four-column workspace (library, article, outline,
//! inspector) plus a non-modal two-hop shelf and a global palette. Editing stays in an
//! external Neovide attached to the active Content through `wiki-server`.
//!
//! Data comes from the local JSON-RPC server over `WIKI_SOCKET` (`list`, `open`,
//! `workspace`, `search`, `save`). When the server is unavailable the GUI uses a
//! built-in fixture and says so in the toolbar and the status bar.

mod highlight;
mod neovide;

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::Duration;

use anti_webui::prelude::*;
use harmion_wiki_core::parse_interpreter_calls;
use serde::Deserialize;
use serde_json::{Value, json};

const DEFAULT_SOCKET: &str = "/tmp/harmion-wiki-m0.sock";
const JOURNAL_ID: &str = "3f2a9c1e00000000000000000000000000000000000000000000000000000001";
const BUDGET_ID: &str = "3f2a9c1e00000000000000000000000000000000000000000000000000000002";
const MONTHLY_ID: &str = "3f2a9c1e00000000000000000000000000000000000000000000000000000003";
const HOME_ID: &str = "p-home";
const READING_ID: &str = "p-reading";
const RUST_ID: &str = "p-rust";
const WINDOW_WIDTH: u32 = 1600;
const WINDOW_HEIGHT: u32 = 1000;
const SHELF_HEIGHT: f32 = 430.0;
const SHELF_HANDLE: f32 = 36.0;

mod bundled {
    include!(concat!(env!("OUT_DIR"), "/fonts.rs"));
}

// ---------------------------------------------------------------------------
// Fonts and theme
// ---------------------------------------------------------------------------

fn fonts() -> &'static (String, String) {
    static FONTS: OnceLock<(String, String)> = OnceLock::new();
    FONTS.get_or_init(|| {
        let mut sans = "Noto Sans CJK JP".to_owned();
        let mut mono = "JetBrains Mono".to_owned();
        if !bundled::SANS.is_empty() {
            match register_font(bundled::SANS) {
                Ok(names) => {
                    if let Some(name) = names
                        .iter()
                        .find(|name| name.contains("JP"))
                        .or_else(|| names.first())
                    {
                        sans = name.clone();
                    }
                }
                Err(error) => eprintln!("wiki-gui: sans font failed: {error}"),
            }
        }
        if !bundled::MONO.is_empty() {
            match register_font(bundled::MONO) {
                Ok(names) => {
                    if let Some(name) = names.first() {
                        mono = name.clone();
                    }
                }
                Err(error) => eprintln!("wiki-gui: mono font failed: {error}"),
            }
        }
        (sans, mono)
    })
}

fn sans() -> &'static str {
    &fonts().0
}

fn mono() -> &'static str {
    &fonts().1
}

fn app_theme() -> Theme {
    Theme {
        page: rgb(0x1e1e1e),
        surface: rgb(0x252527),
        surface_muted: rgb(0x2c2c2e),
        overlay: rgb(0x2c2c2e),
        hover: rgb(0x2c2c2e),
        pressed: rgb(0x3a3a3c),
        selected: rgb(0x0a84ff),
        text: rgb(0xf5f5f7),
        text_muted: rgb(0xa1a1a6),
        text_disabled: rgb(0x6e6e73),
        on_accent: rgb(0xffffff),
        border: rgb(0x3a3a3c),
        border_strong: rgb(0x48484a),
        accent: rgb(0x0a84ff),
        accent_hover: rgb(0x409cff),
        accent_pressed: rgb(0x0070e0),
        accent_text: rgb(0x409cff),
        accent_muted: rgb(0x1c3a5e),
        success: rgb(0x30d158),
        success_muted: rgb(0x17351f),
        success_text: rgb(0x30d158),
        ..Theme::dark()
    }
}

// ---------------------------------------------------------------------------
// Wire model (mirrors apps/wiki-server/src/model.rs)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize)]
struct Summary {
    id: String,
    #[serde(rename = "type")]
    content_type: String,
    title: String,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    kinds: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    rev: u64,
    #[serde(default)]
    updated_label: String,
    #[serde(default)]
    excerpt: String,
    /// Not on the wire; the fixture uses it for the shelf's reference count.
    #[serde(default)]
    refs: usize,
}

impl Summary {
    fn is_page(&self) -> bool {
        self.content_type == "page"
    }

    fn is_snippet(&self) -> bool {
        self.content_type == "snippet"
    }

    fn kind_label(&self) -> String {
        summary_kind(self)
    }
}

#[derive(Clone, Debug, Deserialize)]
struct Content {
    id: String,
    #[serde(rename = "type")]
    content_type: String,
    title: String,
    body: String,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    kinds: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    rev: u64,
    #[serde(default)]
    created: String,
    #[serde(default)]
    updated_at: i64,
    #[serde(default)]
    updated_label: String,
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
            tags: content.tags.clone(),
            aliases: content.aliases.clone(),
            rev: content.rev,
            updated_label: content.updated_label.clone(),
            excerpt: excerpt(&content.body),
            refs: 0,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
struct OutlineItem {
    #[serde(default)]
    level: u8,
    title: String,
    #[serde(default)]
    line: usize,
}

#[derive(Clone, Debug, Deserialize)]
struct TwoHop {
    via: Summary,
    target: Summary,
    #[serde(default)]
    reason: String,
}

#[derive(Clone, Debug, Deserialize)]
struct EmbedView {
    #[serde(default)]
    before: Vec<String>,
    snippet: Content,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct Links {
    #[serde(default)]
    outgoing: Vec<Summary>,
    #[serde(default)]
    backlinks: Vec<Summary>,
    #[serde(default)]
    two_hop: Vec<TwoHop>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct PageInfo {
    #[serde(default)]
    created: String,
    #[serde(default)]
    updated: String,
    #[serde(default)]
    word_count: usize,
    #[serde(default)]
    tags: String,
}

#[derive(Clone, Debug, Deserialize)]
struct Workspace {
    page: Content,
    #[serde(default)]
    before: Vec<String>,
    #[serde(default)]
    after: Vec<String>,
    #[serde(default)]
    snippet: Option<Content>,
    #[serde(default)]
    embeds: Vec<EmbedView>,
    #[serde(default)]
    outline: Vec<OutlineItem>,
    #[serde(default)]
    links: Links,
    #[serde(default)]
    related: Vec<Summary>,
    #[serde(default)]
    recent: Vec<Summary>,
    #[serde(default)]
    info: PageInfo,
}

#[derive(Clone, Debug)]
struct Tab {
    id: Option<String>,
    title: String,
    kind: String,
    workspace: Option<Workspace>,
    error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Server,
    Fallback,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Inspector {
    Links,
    Info,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PaletteMode {
    Search,
    QuickOpen,
    Rename,
    Tags,
}

#[derive(Clone, Debug, Default)]
struct EditorState {
    attached: bool,
    follow: bool,
    detail: String,
    warning: Option<String>,
    error: Option<String>,
}

#[derive(Clone)]
struct AppState {
    source: Signal<Source>,
    boot_error: Option<String>,
    socket: String,
    summaries: Signal<Vec<Summary>>,
    providers: Signal<Vec<String>>,
    tag_filter: Signal<Option<String>>,
    tabs: Signal<Vec<Tab>>,
    active: Signal<usize>,
    inspector: Signal<Inspector>,
    inspector_visible: Signal<bool>,
    shelf_pinned: Signal<bool>,
    shelf_hover: Signal<bool>,
    palette: Signal<Option<PaletteMode>>,
    query: Signal<String>,
    results: Signal<Vec<Summary>>,
    palette_error: Signal<Option<String>>,
    next_selected: Signal<usize>,
    editor: Signal<EditorState>,
    history: Signal<Vec<String>>,
    history_index: Signal<usize>,
    outline_active: Signal<usize>,
    delete_armed: Signal<bool>,
    article_ref: NodeRef,
    root: NodeRef,
    manager: Rc<RefCell<neovide::Manager>>,
}

impl AppState {
    fn active_tab(&self) -> Option<Tab> {
        let tabs = self.tabs.get_untracked();
        tabs.get(
            self.active
                .get_untracked()
                .min(tabs.len().saturating_sub(1)),
        )
        .cloned()
    }

    fn active_index(&self) -> usize {
        let tabs = self.tabs.get_untracked();
        self.active
            .get_untracked()
            .min(tabs.len().saturating_sub(1))
    }

    fn active_workspace(&self) -> Option<Workspace> {
        self.active_tab().and_then(|tab| tab.workspace)
    }

    fn ensure_workspace(&self, index: usize) {
        let Some(tab) = self.tabs.get_untracked().get(index).cloned() else {
            return;
        };
        if tab.workspace.is_some() || tab.error.is_some() {
            return;
        }
        let result = match (&tab.id, self.source.get_untracked()) {
            (None, _) => Err("このページの Content ID はまだ解決できていません。".to_owned()),
            (Some(id), Source::Fallback) => fixture_workspace(id, &tab.title)
                .ok_or_else(|| "フォールバックにこのページのデータがありません。".to_owned()),
            (Some(id), Source::Server) => rpc("workspace", json!({ "id": id }))
                .and_then(|value| {
                    serde_json::from_value::<Workspace>(value)
                        .map_err(|error| format!("workspace の解釈に失敗: {error}"))
                })
                .map_err(|error| format!("workspace を取得できません: {error}")),
        };
        self.tabs.update(|tabs| {
            if let Some(tab) = tabs.get_mut(index) {
                match result {
                    Ok(workspace) => {
                        tab.title = workspace.page.title.clone();
                        tab.kind = workspace.page.content_type.clone();
                        tab.workspace = Some(workspace);
                        tab.error = None;
                    }
                    Err(error) => tab.error = Some(error),
                }
            }
        });
    }

    fn switch_tab(&self, index: usize) {
        let length = self.tabs.get_untracked().len();
        if index >= length {
            return;
        }
        self.active.set(index);
        self.ensure_workspace(index);
        if let Some(tab) = self.active_tab() {
            self.push_history(tab.id);
            self.follow_editor();
        }
    }

    fn open_summary(&self, summary: &Summary) {
        self.open_id(&summary.id, &summary.title, &summary.content_type);
    }

    fn new_page(&self) {
        if !matches!(self.source.get_untracked(), Source::Server) {
            self.editor.update(|editor| {
                editor.error = Some("新規ページ: wiki-server に接続していません".to_owned());
            });
            return;
        }
        let value = match rpc(
            "create_page",
            json!({
                "title": "無題のページ",
                "body": "# 無題のページ\n\n",
                "tags": [],
                "aliases": [],
            }),
        ) {
            Ok(value) => value,
            Err(error) => {
                self.editor
                    .update(|editor| editor.error = Some(format!("新規ページ: {error}")));
                return;
            }
        };
        let content = match serde_json::from_value::<Content>(value) {
            Ok(content) => content,
            Err(error) => {
                self.editor.update(|editor| {
                    editor.error = Some(format!("新規ページの解釈に失敗: {error}"));
                });
                return;
            }
        };
        let summary = Summary {
            id: content.id.clone(),
            content_type: content.content_type.clone(),
            title: content.title.clone(),
            format: content.format.clone(),
            language: content.language.clone(),
            kinds: content.kinds.clone(),
            tags: content.tags.clone(),
            aliases: content.aliases.clone(),
            rev: content.rev,
            updated_label: "たった今".to_owned(),
            excerpt: "# 無題のページ".to_owned(),
            refs: 0,
        };
        self.summaries.update(|items| {
            if !items.iter().any(|item| item.id == summary.id) {
                items.insert(0, summary);
            }
        });
        self.editor.update(|editor| editor.error = None);
        self.open_id(&content.id, &content.title, &content.content_type);
    }

    fn rename_active(&self, title: &str) {
        let Some(workspace) = self.active_workspace() else { return };
        let id = workspace.page.id.clone();
        let rev = workspace.page.rev;
        match rpc("update_meta", json!({ "id": id, "rev": rev, "title": title.trim() })) {
            Ok(value) => match serde_json::from_value::<Content>(value) {
                Ok(content) => {
                    self.tabs.update(|tabs| {
                        if let Some(tab) = tabs.iter_mut().find(|tab| tab.id.as_deref() == Some(id.as_str())) {
                            tab.title = content.title.clone();
                            if let Some(workspace) = tab.workspace.as_mut() {
                                workspace.page = content.clone();
                                workspace.info.updated = content.updated_label.clone();
                            }
                        }
                    });
                    self.summaries.update(|items| {
                        if let Some(summary) = items.iter_mut().find(|summary| summary.id == id) {
                            summary.title = content.title.clone();
                            summary.rev = content.rev;
                        }
                    });
                    self.close_palette();
                }
                Err(error) => self.palette_error.set(Some(format!("解釈に失敗: {error}"))),
            },
            Err(error) => self.palette_error.set(Some(format!("変更できません: {error}"))),
        }
    }

    fn set_tags(&self, tags: &str) {
        let Some(workspace) = self.active_workspace() else { return };
        let id = workspace.page.id.clone();
        let rev = workspace.page.rev;
        let values = tags
            .split([',', '、'])
            .map(str::trim)
            .filter(|tag| !tag.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        match rpc("update_meta", json!({ "id": id, "rev": rev, "tags": values })) {
            Ok(value) => match serde_json::from_value::<Content>(value) {
                Ok(content) => {
                    self.tabs.update(|tabs| {
                        if let Some(tab) = tabs.iter_mut().find(|tab| tab.id.as_deref() == Some(id.as_str())) {
                            if let Some(workspace) = tab.workspace.as_mut() {
                                workspace.page = content.clone();
                                workspace.info.tags = content.tags.join(", ");
                            }
                        }
                    });
                    self.summaries.update(|items| {
                        if let Some(summary) = items.iter_mut().find(|summary| summary.id == id) {
                            summary.tags = content.tags.clone();
                            summary.rev = content.rev;
                        }
                    });
                    self.close_palette();
                }
                Err(error) => self.palette_error.set(Some(format!("解釈に失敗: {error}"))),
            },
            Err(error) => self.palette_error.set(Some(format!("変更できません: {error}"))),
        }
    }

    fn open_id(&self, id: &str, title: &str, kind: &str) {
        if let Some(index) = self
            .tabs
            .get_untracked()
            .iter()
            .position(|tab| tab.id.as_deref() == Some(id))
        {
            self.active.set(index);
        } else {
            let tab = Tab {
                id: Some(id.to_owned()),
                title: title.to_owned(),
                kind: kind.to_owned(),
                workspace: None,
                error: None,
            };
            let index = self.tabs.get_untracked().len();
            self.tabs.update(|tabs| tabs.push(tab));
            self.active.set(index);
        }
        let index = self.active_index();
        self.ensure_workspace(index);
        self.push_history(Some(id.to_owned()));
        self.palette.set(None);
        self.palette_error.set(None);
        self.request_root_focus();
        self.follow_editor();
    }

    fn close_tab(&self, index: usize) {
        let mut tabs = self.tabs.get_untracked();
        if tabs.len() <= 1 || index >= tabs.len() {
            return;
        }
        tabs.remove(index);
        let new_active = self.active.get_untracked().min(tabs.len() - 1);
        self.tabs.set(tabs);
        self.active.set(new_active);
        self.ensure_workspace(new_active);
        self.follow_editor();
    }

    fn push_history(&self, id: Option<String>) {
        let Some(id) = id else { return };
        let current = self.history.get_untracked();
        let index = self.history_index.get_untracked();
        if current.get(index).is_some_and(|entry| *entry == id) {
            return;
        }
        let end = if current.is_empty() {
            0
        } else {
            (index + 1).min(current.len())
        };
        let mut next = current[..end].to_vec();
        next.push(id);
        self.history_index.set(next.len() - 1);
        self.history.set(next);
    }

    fn back(&self) {
        let index = self.history_index.get_untracked();
        if index == 0 {
            return;
        }
        self.history_index.set(index - 1);
        self.open_history_entry();
    }

    fn forward(&self) {
        let index = self.history_index.get_untracked();
        if index + 1 >= self.history.get_untracked().len() {
            return;
        }
        self.history_index.set(index + 1);
        self.open_history_entry();
    }

    fn open_history_entry(&self) {
        let index = self.history_index.get_untracked();
        let Some(id) = self.history.get_untracked().get(index).cloned() else {
            return;
        };
        if let Some(tab_index) = self
            .tabs
            .get_untracked()
            .iter()
            .position(|tab| tab.id.as_deref() == Some(id.as_str()))
        {
            self.active.set(tab_index);
            self.ensure_workspace(tab_index);
            self.follow_editor();
            return;
        }
        let summary = self
            .summaries
            .get_untracked()
            .into_iter()
            .find(|summary| summary.id == id);
        if let Some(summary) = summary {
            self.open_id(&summary.id, &summary.title, &summary.content_type);
        }
    }

    fn open_palette(&self, mode: PaletteMode) {
        self.palette.set(Some(mode));
        let prefill = match mode {
            PaletteMode::Rename => self
                .active_tab()
                .map(|tab| tab.title)
                .unwrap_or_default(),
            PaletteMode::Tags => self
                .active_workspace()
                .map(|workspace| workspace.page.tags.join(", "))
                .unwrap_or_default(),
            PaletteMode::Search | PaletteMode::QuickOpen => String::new(),
        };
        self.query.set(prefill);
        self.palette_error.set(None);
        self.refresh_results();
    }

    fn close_palette(&self) {
        self.palette.set(None);
        self.palette_error.set(None);
        self.request_root_focus();
    }

    fn on_query(&self, text: &str) {
        self.query.set(text.to_owned());
        self.refresh_results();
    }

    fn refresh_results(&self) {
        let query = self.query.get_untracked();
        let mode = self.palette.get_untracked();
        match mode {
            Some(PaletteMode::QuickOpen) => {
                let needle = query.to_lowercase();
                let mut results = self
                    .summaries
                    .get_untracked()
                    .into_iter()
                    .filter(|summary| summary.is_page())
                    .filter(|summary| {
                        needle.is_empty() || summary.title.to_lowercase().contains(&needle)
                    })
                    .collect::<Vec<_>>();
                results.truncate(20);
                self.results.set(results);
                self.palette_error.set(None);
            }
            Some(PaletteMode::Search) => {
                if query.trim().is_empty() {
                    let mut results = self
                        .summaries
                        .get_untracked()
                        .into_iter()
                        .filter(|summary| summary.is_page())
                        .collect::<Vec<_>>();
                    results.truncate(20);
                    self.results.set(results);
                    self.palette_error.set(None);
                    return;
                }
                if self.source.get_untracked() == Source::Server {
                    match rpc("search", json!({ "query": query })) {
                        Ok(value) => match value.get("items").cloned() {
                            Some(items) => match serde_json::from_value::<Vec<Summary>>(items) {
                                Ok(mut results) => {
                                    results.truncate(20);
                                    self.results.set(results);
                                    self.palette_error.set(None);
                                }
                                Err(error) => {
                                    self.results
                                        .set(local_search(&self.summaries.get_untracked(), &query));
                                    self.palette_error
                                        .set(Some(format!("検索結果の解釈に失敗: {error}")));
                                }
                            },
                            None => {
                                self.results
                                    .set(local_search(&self.summaries.get_untracked(), &query));
                                self.palette_error
                                    .set(Some("検索結果に items がありません。".to_owned()));
                            }
                        },
                        Err(error) => {
                            self.results
                                .set(local_search(&self.summaries.get_untracked(), &query));
                            self.palette_error
                                .set(Some(format!("検索サーバーに接続できません: {error}")));
                        }
                    }
                } else {
                    self.results
                        .set(local_search(&self.summaries.get_untracked(), &query));
                    self.palette_error.set(None);
                }
            }
            Some(PaletteMode::Rename | PaletteMode::Tags) => {
                self.results.set(Vec::new());
                self.palette_error.set(None);
            }
            None => {}
        }
    }

    fn open_editor(&self) {
        let Some(tab) = self.active_tab() else { return };
        let Some(id) = tab.id else {
            self.set_editor_error("Content ID がないため Neovide で開けません。".to_owned());
            return;
        };
        self.open_editor_for(&id, &tab.title);
    }

    fn open_editor_for(&self, id: &str, title: &str) {
        let outcome = self.manager.borrow_mut().open(id, title);
        match outcome {
            Ok(outcome) => {
                self.editor.set(EditorState {
                    attached: true,
                    follow: true,
                    detail: outcome.message,
                    warning: outcome.warning,
                    error: None,
                });
            }
            Err(error) => {
                eprintln!("wiki-gui: neovide: {error}");
                self.set_editor_error(error);
            }
        }
        self.request_root_focus();
    }

    fn set_editor_error(&self, error: String) {
        let mut editor = self.editor.get_untracked();
        editor.attached = self.manager.borrow().has_server();
        editor.error = Some(error);
        editor.detail = self.manager.borrow().status_label();
        self.editor.set(editor);
    }

    fn follow_editor(&self) {
        let editor = self.editor.get_untracked();
        if !editor.follow {
            return;
        }
        let Some(tab) = self.active_tab() else { return };
        let Some(id) = tab.id else { return };
        let outcome = self.manager.borrow_mut().open(&id, &tab.title);
        match outcome {
            Ok(outcome) => {
                let mut editor = self.editor.get_untracked();
                editor.attached = true;
                editor.detail = outcome.message;
                editor.warning = outcome.warning;
                editor.error = None;
                self.editor.set(editor);
            }
            Err(error) => {
                let mut editor = self.editor.get_untracked();
                editor.attached = self.manager.borrow().has_server();
                editor.detail = self.manager.borrow().status_label();
                editor.error = Some(error);
                self.editor.set(editor);
            }
        }
    }

    fn delete_content(&self, id: &str) {
        if !matches!(self.source.get_untracked(), Source::Server) {
            self.set_editor_error("削除: wiki-server に接続していません".to_owned());
            return;
        }
        if let Err(error) = rpc("delete", json!({ "id": id })) {
            self.set_editor_error(format!("削除できません: {error}"));
            return;
        }
        self.summaries
            .update(|items| items.retain(|item| item.id != id));
        self.tabs
            .update(|tabs| tabs.retain(|tab| tab.id.as_deref() != Some(id)));
        self.delete_armed.set(false);
        if self.tabs.get_untracked().is_empty() {
            self.editor.update(|editor| {
                editor.error = Some("削除しました。タブがありません。".to_owned())
            });
        } else {
            let index = self
                .active
                .get_untracked()
                .min(self.tabs.get_untracked().len() - 1);
            self.active.set(index);
            self.ensure_workspace(index);
        }
    }

    fn refresh(&self) {
        if matches!(self.source.get_untracked(), Source::Server) {
            match load_summaries() {
                Ok(items) => self.summaries.set(items),
                Err(error) => {
                    self.editor
                        .update(|editor| editor.error = Some(format!("再読み込み: {error}")));
                }
            }
        }
        let index = self.active_index();
        self.tabs.update(|tabs| {
            if let Some(tab) = tabs.get_mut(index) {
                tab.workspace = None;
                tab.error = None;
            }
        });
        self.ensure_workspace(index);
        self.outline_active.set(0);
    }

    fn scroll_to_line(&self, line: usize) {
        let y = (line.saturating_sub(1) as f32 * 24.0) as i32;
        let article = self.article_ref;
        request_frame(move |_| article.scroll_to_after_layout(0, y));
    }

    fn request_root_focus(&self) {
        let root = self.root;
        request_frame(move |_| root.focus());
    }
}

fn summary_kind(summary: &Summary) -> String {
    if let Some(kind) = summary.kinds.iter().find(|kind| kind.as_str() != "wiki") {
        return kind.clone();
    }
    if let Some(format) = &summary.format {
        return format.clone();
    }
    if let Some(language) = &summary.language {
        return language.clone();
    }
    summary.content_type.clone()
}

fn local_search(summaries: &[Summary], query: &str) -> Vec<Summary> {
    let needle = query.to_lowercase();
    let mut results = summaries
        .iter()
        .filter(|summary| {
            summary.title.to_lowercase().contains(&needle)
                || summary.excerpt.to_lowercase().contains(&needle)
                || summary
                    .kinds
                    .iter()
                    .any(|kind| kind.to_lowercase().contains(&needle))
        })
        .cloned()
        .collect::<Vec<_>>();
    results.truncate(20);
    results
}

// ---------------------------------------------------------------------------
// JSON-RPC
// ---------------------------------------------------------------------------

fn socket_path() -> String {
    std::env::var("WIKI_SOCKET").unwrap_or_else(|_| DEFAULT_SOCKET.to_owned())
}

fn rpc(method: &str, params: Value) -> Result<Value, String> {
    let path = socket_path();
    let mut stream =
        UnixStream::connect(&path).map_err(|error| format!("connect {path}: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| error.to_string())?;
    let request = json!({ "id": 1, "method": method, "params": params });
    let mut bytes = serde_json::to_vec(&request).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    stream
        .write_all(&bytes)
        .map_err(|error| error.to_string())?;
    stream.flush().map_err(|error| error.to_string())?;

    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|error| error.to_string())?;
    if line.trim().is_empty() {
        return Err("server closed the connection without a response".to_owned());
    }
    let response: Value = serde_json::from_str(&line).map_err(|error| error.to_string())?;
    if let Some(error) = response.get("error") {
        return Err(error.to_string());
    }
    Ok(response.get("result").cloned().unwrap_or(Value::Null))
}

#[allow(dead_code)]
fn save_content(id: &str, rev: u64, body: &str) -> Result<Content, String> {
    let value = rpc("save", json!({ "id": id, "rev": rev, "body": body }))?;
    serde_json::from_value(value).map_err(|error| format!("save の解釈に失敗: {error}"))
}

fn load_providers() -> Vec<String> {
    match rpc("provider_list", json!({})) {
        Ok(value) => value
            .get("providers")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get("name").and_then(Value::as_str).map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

fn load_summaries() -> Result<Vec<Summary>, String> {
    let value = rpc("list", json!({}))?;
    let items = value
        .get("items")
        .cloned()
        .ok_or_else(|| "list result has no items".to_owned())?;
    serde_json::from_value(items).map_err(|error| format!("list の解釈に失敗: {error}"))
}

fn boot(snapshot: bool) -> AppState {
    let socket = socket_path();
    let (summaries, source, boot_error) = match load_summaries() {
        Ok(summaries) => {
            eprintln!("wiki-gui: loaded list from {socket}");
            (summaries, Source::Server, None)
        }
        Err(error) => {
            eprintln!("wiki-gui: server unavailable ({error}); using built-in fixture");
            (fixture_summaries(), Source::Fallback, Some(error))
        }
    };

    let mut tabs = Vec::new();
    for title in ["家計の記録", "読書ノート", "Rustのメモ"] {
        let summary = summaries.iter().find(|summary| summary.title == title);
        tabs.push(Tab {
            id: summary.map(|summary| summary.id.clone()),
            title: title.to_owned(),
            kind: summary
                .map(|summary| summary.content_type.clone())
                .unwrap_or_else(|| "page".to_owned()),
            workspace: None,
            error: None,
        });
    }

    let manager = Rc::new(RefCell::new(neovide::Manager::detect()));
    let attached = manager.borrow().has_server();
    let detail = manager.borrow().status_label();
    let root = node_ref();
    let state = AppState {
        source: signal(source),
        boot_error,
        socket,
        summaries: signal(summaries),
        providers: signal(load_providers()),
        tag_filter: signal(None),
        tabs: signal(tabs),
        active: signal(0),
        inspector: signal(Inspector::Links),
        inspector_visible: signal(true),
        shelf_pinned: signal(snapshot),
        shelf_hover: signal(false),
        palette: signal(None),
        query: signal(String::new()),
        results: signal(Vec::new()),
        palette_error: signal(None),
        next_selected: signal(0),
        editor: signal(EditorState {
            attached,
            follow: attached,
            detail,
            warning: None,
            error: None,
        }),
        history: signal(Vec::new()),
        history_index: signal(0),
        outline_active: signal(0),
        delete_armed: signal(false),
        article_ref: node_ref(),
        root,
        manager,
    };
    for index in 0..3 {
        state.ensure_workspace(index);
    }
    if let Some(id) = state.active_tab().and_then(|tab| tab.id) {
        state.push_history(Some(id));
    }
    state
}

// ---------------------------------------------------------------------------
// Small view helpers
// ---------------------------------------------------------------------------

fn ui_text(label: impl Into<String>, size: f32, color: Color) -> Text {
    text(label.into())
        .font_family(sans())
        .text_size(size)
        .text_color(color)
}

fn mono_text(label: impl Into<String>, size: f32, color: Color) -> Text {
    text(label.into())
        .font_family(mono())
        .text_size(size)
        .text_color(color)
}

fn dot(color: Color) -> Div {
    div().w(7.0_f32).h(7.0_f32).rounded(3.5_f32).bg(color)
}

fn kbd(label: impl Into<String>) -> Div {
    div()
        .h(16.0_f32)
        .px(4.0_f32)
        .rounded(3.0_f32)
        .bg(theme().surface_muted)
        .flex()
        .items_center()
        .justify_center()
        .child(ui_text(label, 9.0, theme().text_muted))
}

fn section_label(label: impl Into<String>) -> Text {
    ui_text(label, 11.0, theme().text_disabled).font_weight(FontWeight::SEMIBOLD)
}

fn chip(label: impl Into<String>, color: Color) -> Div {
    div()
        .px(6.0_f32)
        .py(1.0_f32)
        .rounded(4.0_f32)
        .bg(theme().surface_muted)
        .child(mono_text(label, 8.5, color))
}

fn meta_sep() -> Text {
    ui_text("·", 11.0, theme().text_disabled)
}

fn tag_text(tags: &str) -> String {
    tags.split(',')
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

fn short_date(date: &str) -> String {
    if date.len() == 10
        && date.as_bytes().get(4) == Some(&b'-')
        && date.as_bytes().get(7) == Some(&b'-')
    {
        format!("{}/{}", &date[5..7], &date[8..10])
    } else {
        date.to_owned()
    }
}

fn short_id(id: &str) -> String {
    let prefix = id.chars().take(8).collect::<String>();
    format!("{prefix}…")
}

fn truncate_chars(text: &str, max: usize) -> String {
    let mut chars = text.chars();
    let out = chars.by_ref().take(max).collect::<String>();
    if chars.next().is_some() {
        format!("{out}…")
    } else {
        out
    }
}

fn excerpt_of(body: &str) -> String {
    let line = body
        .lines()
        .map(str::trim)
        .find(|line| {
            !line.is_empty()
                && !line.starts_with('#')
                && !line.starts_with("!```")
                && !line.starts_with("```")
                && !line.starts_with("関連:")
        })
        .unwrap_or_default();
    truncate_chars(line, 80)
}

fn class_color(class: highlight::Class) -> Color {
    match class {
        highlight::Class::Plain => rgb(0xc0caf5),
        highlight::Class::Date => rgb(0x7aa2f7),
        highlight::Class::Account => rgb(0x9ece6a),
        highlight::Class::Amount => rgb(0xe0af68),
        highlight::Class::Desc => rgb(0xc0caf5),
        highlight::Class::Comment => rgb(0x565f89),
        highlight::Class::Keyword => rgb(0xbb9af7),
        highlight::Class::Type => rgb(0x2ac3de),
        highlight::Class::Func => rgb(0x7aa2f7),
        highlight::Class::String => rgb(0x9ece6a),
        highlight::Class::Number => rgb(0xff9e64),
        highlight::Class::Bool => rgb(0xff9e64),
        highlight::Class::Key => rgb(0x7aa2f7),
        highlight::Class::Punct => rgb(0xa9b1d6),
    }
}

fn code_line(line: &str, language: &str) -> RichText {
    let tokens = highlight::line(line, language);
    let runs = tokens
        .into_iter()
        .map(|token| span(token.text).text_color(class_color(token.class)))
        .collect::<Vec<_>>();
    rich_text(runs)
        .font_family(mono())
        .text_size(12.0_f32)
        .line_height(LineHeight::Relative(1.75_f32))
}

fn code_block(lines: &[&str], language: &str) -> Div {
    let mut block = div().flex().flex_col();
    for (index, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            block = block.child(div().h(7.0_f32));
        } else if index == 0 {
            block = block.child(code_line(line, language));
        } else {
            block = block.child(code_line(line, language));
        }
    }
    block
}

// ---------------------------------------------------------------------------
// Shell
// ---------------------------------------------------------------------------

fn layout(state: &AppState) -> Div {
    let root = state.root;
    let key_state = state.clone();
    let root_ref = root;
    request_frame(move |_| root_ref.focus());
    div()
        .w_full()
        .h_full()
        .relative()
        .overflow_hidden()
        .flex()
        .flex_col()
        .bg(theme().page)
        .text_color(theme().text)
        .node_ref(root)
        .focusable()
        .on_key_down(move |event| handle_key(&key_state, event))
        .child(dyn_view({
            let state = state.clone();
            move || toolbar(&state)
        }))
        .child(
            div()
                .flex()
                .flex_row()
                .flex_1()
                .min_h(0.0_f32)
                .child(dyn_view({
                    let state = state.clone();
                    move || sidebar(&state)
                }))
                .child(dyn_view({
                    let state = state.clone();
                    move || editor_column(&state)
                }))
                .child(dyn_view({
                    let state = state.clone();
                    move || outline_column(&state)
                }))
                .child(dyn_view({
                    let state = state.clone();
                    move || inspector_column(&state)
                })),
        )
        .child(shelf(state))
        .child(dyn_view({
            let state = state.clone();
            move || status_bar(&state)
        }))
        .child(palette(state))
}

fn handle_key(state: &AppState, event: &KeyboardEvent) {
    let ctrl = event.modifiers.ctrl;
    let alt = event.modifiers.alt;
    if event.key == Key::Named(NamedKey::Escape) {
        if state.palette.get_untracked().is_some() {
            event.prevent_default();
            state.close_palette();
            return;
        }
        if state.shelf_pinned.get_untracked() && !state.shelf_hover.get_untracked() {
            event.prevent_default();
            state.shelf_pinned.set(false);
            return;
        }
    }
    if ctrl && alt {
        if event.key.is_char('l') {
            event.prevent_default();
            state.shelf_pinned.update(|pinned| *pinned = !*pinned);
            return;
        }
    }
    if !ctrl {
        return;
    }
    if event.key == Key::Named(NamedKey::Enter) {
        if state.palette.get_untracked().is_some() {
            event.prevent_default();
            if let Some(first) = state.results.get_untracked().into_iter().next() {
                state.open_summary(&first);
            }
        }
        return;
    }
    if event.key.is_char('k') {
        event.prevent_default();
        state.open_palette(PaletteMode::Search);
    } else if event.key.is_char('p') {
        event.prevent_default();
        state.open_palette(PaletteMode::QuickOpen);
    } else if event.key.is_char('e') {
        event.prevent_default();
        state.open_editor();
    } else if event.key.is_char('n') {
        event.prevent_default();
        state.new_page();
    } else if event.key.is_char('i') {
        event.prevent_default();
        state
            .inspector_visible
            .update(|visible| *visible = !*visible);
    } else if event.key.is_char('1') {
        event.prevent_default();
        state.inspector.set(Inspector::Links);
        state.inspector_visible.set(true);
    } else if event.key.is_char('2') {
        event.prevent_default();
        state.inspector.set(Inspector::Info);
        state.inspector_visible.set(true);
    } else if event.key.is_char('r') {
        event.prevent_default();
        state.refresh();
    } else if event.key.is_char('w') {
        event.prevent_default();
        let index = state.active_index();
        state.close_tab(index);
    } else if event.key.is_char('[') {
        event.prevent_default();
        state.back();
    } else if event.key.is_char(']') {
        event.prevent_default();
        state.forward();
    }
}

// ---------------------------------------------------------------------------
// Toolbar
// ---------------------------------------------------------------------------

fn toolbar(state: &AppState) -> Div {
    let tab = state.active_tab();
    let title = tab
        .as_ref()
        .map(|tab| tab.title.clone())
        .unwrap_or_else(|| "Wiki".to_owned());
    let editor = state.editor.get();
    let fallback = state.source.get() == Source::Fallback;

    let mut left = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(5.0_f32)
        .child(icon_button("▤", "サイドバー"))
        .child(vdiv())
        .child(icon_button("‹", "戻る Ctrl+[").on_click({
            let state = state.clone();
            move |_| state.back()
        }))
        .child(icon_button("›", "進む Ctrl+]").on_click({
            let state = state.clone();
            move |_| state.forward()
        }))
        .child(vdiv())
        .child(ui_text(&title, 13.0, theme().text).font_weight(FontWeight::SEMIBOLD))
        .child(ui_text("Wiki", 12.0, theme().text_disabled));
    if fallback {
        left = left.child(
            div()
                .px(6.0_f32)
                .py(2.0_f32)
                .rounded(4.0_f32)
                .bg(rgb(0x3a2a12))
                .child(ui_text("fixture", 9.0, rgb(0xe0af68))),
        );
    }

    let editor_label = tab
        .as_ref()
        .map(|tab| tab.title.clone())
        .unwrap_or_else(|| "—".to_owned());
    let editor_color = if editor.error.is_some() {
        rgb(0xff453a)
    } else if editor.attached {
        theme().success
    } else {
        theme().text_disabled
    };

    div()
        .h(52.0_f32)
        .px(14.0_f32)
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .bg(theme().page)
        .border_b(1.0_f32)
        .border_color(theme().border)
        .child(left)
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(6.0_f32)
                .child(
                    div()
                        .w(210.0_f32)
                        .h(28.0_f32)
                        .px(8.0_f32)
                        .rounded(6.0_f32)
                        .bg(theme().surface_muted)
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(6.0_f32)
                        .hover(|style| style.bg(theme().hover))
                        .on_click({
                            let state = state.clone();
                            move |_| state.open_palette(PaletteMode::Search)
                        })
                        .child(ui_text("◎", 13.0, theme().text_muted))
                        .child(ui_text("検索", 12.0, theme().text_muted))
                        .child(div().flex_1())
                        .child(kbd("Ctrl+K")),
                )
                .child(
                    div()
                        .h(28.0_f32)
                        .px(9.0_f32)
                        .rounded(6.0_f32)
                        .bg(theme().surface_muted)
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(6.0_f32)
                        .hover(|style| style.bg(theme().hover))
                        .on_click({
                            let state = state.clone();
                            move |_| state.open_editor()
                        })
                        .child(dot(editor_color))
                        .child(
                            ui_text("Neovide", 11.0, theme().text_muted)
                                .font_weight(FontWeight::MEDIUM),
                        )
                        .child(ui_text("·", 10.0, theme().text_disabled))
                        .child(ui_text(&editor_label, 11.0, theme().text))
                        .child(kbd("Ctrl+E")),
                )
                .child(
                    div()
                        .h(28.0_f32)
                        .px(7.0_f32)
                        .rounded(6.0_f32)
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(4.0_f32)
                        .hover(|style| style.bg(theme().hover))
                        .on_click({
                            let state = state.clone();
                            move |_| {
                                state
                                    .inspector_visible
                                    .update(|visible| *visible = !*visible)
                            }
                        })
                        .child(ui_text("ⓘ", 14.0, theme().text_muted))
                        .child(kbd("Ctrl+I")),
                ),
        )
}

fn vdiv() -> Div {
    div().w(1.0_f32).h(18.0_f32).mx(4.0_f32).bg(theme().border)
}

fn icon_button(glyph: &str, tooltip: &str) -> Div {
    div()
        .h(26.0_f32)
        .min_w(26.0_f32)
        .px(6.0_f32)
        .rounded(5.0_f32)
        .flex()
        .items_center()
        .justify_center()
        .hover(|style| style.bg(theme().surface_muted))
        .child(ui_text(glyph, 14.0, theme().text_muted))
        .access({
            let tooltip = tooltip.to_owned();
            move |access| access.label(tooltip.clone())
        })
}

// ---------------------------------------------------------------------------
// Sidebar
// ---------------------------------------------------------------------------

fn sidebar(state: &AppState) -> Div {
    let summaries = state.summaries.get();
    let active_tab = state.active_tab();
    let active_id = active_tab
        .as_ref()
        .and_then(|tab| tab.id.as_deref())
        .unwrap_or_default()
        .to_owned();
    let workspace = state.active_workspace();
    let tag_filter = state.tag_filter.get();
    let matches_tag = |summary: &Summary| {
        tag_filter.as_ref().is_none_or(|tag| {
            summary.tags.iter().any(|candidate| candidate == tag)
                || summary.kinds.iter().any(|candidate| candidate == tag)
        })
    };
    let pages = summaries
        .iter()
        .filter(|summary| summary.is_page() && matches_tag(summary))
        .collect::<Vec<_>>();
    let snippets = summaries
        .iter()
        .filter(|summary| summary.is_snippet() && matches_tag(summary))
        .collect::<Vec<_>>();
    let tags = summaries
        .iter()
        .flat_map(|summary| summary.tags.iter().cloned())
        .collect::<BTreeSet<_>>();
    let mut recent = workspace
        .as_ref()
        .map(|workspace| workspace.recent.clone())
        .filter(|recent| !recent.is_empty())
        .unwrap_or_else(|| summaries.iter().take(4).cloned().collect());
    recent.sort_by_key(|summary| !summary.is_page());

    let mut column = div()
        .w(236.0_f32)
        .h_full()
        .min_h(0.0_f32)
        .px(10.0_f32)
        .py(10.0_f32)
        .bg(theme().surface)
        .border_r(1.0_f32)
        .border_color(theme().border)
        .flex()
        .flex_col()
        .overflow_y_scroll()
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .px(10.0_f32)
                .pt(6.0_f32)
                .pb(8.0_f32)
                .child(ui_text("Wiki", 13.0, theme().text).font_weight(FontWeight::SEMIBOLD))
                .child(
                    div()
                        .hover(|style| style.bg(theme().surface_muted))
                        .rounded(4.0_f32)
                        .on_click({
                            let state = state.clone();
                            move |_| state.new_page()
                        })
                        .child(ui_text("＋", 17.0, theme().accent)),
                ),
        )
        .child(side_section("ライブラリ"))
        .child(nav_row(
            "▤",
            "すべてのページ",
            Some(pages.len()),
            false,
            {
                let state = state.clone();
                move || state.open_palette(PaletteMode::QuickOpen)
            },
        ))
        .child(nav_row("↺", "最近", Some(recent.len()), false, {
            let state = state.clone();
            move || state.open_palette(PaletteMode::Search)
        }))
        .child(side_section("タグ"));

    if tags.is_empty() {
        column = column.child(
            ui_text(
                "タグ付きページはまだありません。",
                10.5,
                theme().text_disabled,
            )
            .px(10.0_f32)
            .py(3.0_f32),
        );
    } else {
        let mut tag_row = div()
            .px(8.0_f32)
            .pb(3.0_f32)
            .flex()
            .flex_row()
            .flex_wrap()
            .gap(4.0_f32);
        for tag in tags.iter().take(12) {
            let active = tag_filter.as_deref() == Some(tag.as_str());
            tag_row = tag_row.child(
                div()
                    .px(7.0_f32)
                    .py(3.0_f32)
                    .rounded(4.0_f32)
                    .bg(if active {
                        theme().accent_muted
                    } else {
                        theme().surface_muted
                    })
                    .border(1.0_f32)
                    .border_color(if active {
                        theme().accent
                    } else {
                        theme().border
                    })
                    .hover(|style| style.bg(theme().hover))
                    .on_click({
                        let state = state.clone();
                        let tag = tag.clone();
                        move |_| {
                            let next = if state.tag_filter.get_untracked().as_deref()
                                == Some(tag.as_str())
                            {
                                None
                            } else {
                                Some(tag.clone())
                            };
                            state.tag_filter.set(next);
                        }
                    })
                    .child(ui_text(format!("#{tag}"), 10.0, theme().text_muted)),
            );
        }
        column = column.child(tag_row);
    }

    column = column.child(side_section("ページ"));

    for summary in &pages {
        let is_active = summary.id == active_id;
        column = column.child(page_row(state, summary, is_active));
    }

    column = column.child(side_section("スニペット"));
    for summary in &snippets {
        column = column.child(snippet_row(state, summary));
    }

    column = column.child(side_section("最近"));
    for summary in recent.iter().take(4) {
        column = column.child(
            div()
                .px(10.0_f32)
                .py(5.0_f32)
                .rounded(5.0_f32)
                .flex()
                .flex_row()
                .items_center()
                .gap(8.0_f32)
                .hover(|style| style.bg(theme().surface_muted))
                .on_click({
                    let state = state.clone();
                    let summary = summary.clone();
                    move |_| state.open_summary(&summary)
                })
                .child(
                    ui_text(truncate_chars(&summary.title, 18), 12.0, theme().text_muted).flex_1(),
                )
                .child(mono_text(summary.kind_label(), 9.0, theme().text_disabled))
                .child(ui_text(&summary.updated_label, 10.0, theme().text_disabled)),
        );
    }

    column
        .child(div().flex_1())
        .child(div().h(12.0_f32))
        .child(
            div()
                .h(28.0_f32)
                .w_full()
                .rounded(6.0_f32)
                .bg(theme().surface_muted)
                .flex()
                .items_center()
                .justify_center()
                .gap(6.0_f32)
                .hover(|style| style.bg(theme().surface_muted))
                .on_click({
                    let state = state.clone();
                    move |_| state.new_page()
                })
                .child(ui_text("新規ページ", 12.0, theme().text))
                .child(kbd("Ctrl+N")),
        )
        .child(div().h(7.0_f32))
        .child(
            div()
                .flex()
                .justify_between()
                .child(ui_text("クイックオープン", 10.0, theme().text_disabled))
                .child(kbd("Ctrl+P")),
        )
        .child(
            div()
                .flex()
                .justify_between()
                .child(ui_text("検索", 10.0, theme().text_disabled))
                .child(kbd("Ctrl+K")),
        )
}

fn side_section(label: &str) -> Div {
    div()
        .mt(16.0_f32)
        .mb(5.0_f32)
        .px(10.0_f32)
        .child(section_label(label))
}

fn nav_row(
    glyph: &str,
    label: &str,
    count: Option<usize>,
    active: bool,
    on_click: impl Fn() + 'static,
) -> Div {
    let mut row = div()
        .h(29.0_f32)
        .px(9.0_f32)
        .rounded(6.0_f32)
        .flex()
        .flex_row()
        .items_center()
        .gap(8.0_f32)
        .hover(|style| style.bg(theme().surface_muted))
        .on_click(move |_| on_click())
        .child(ui_text(
            glyph,
            12.0,
            if active {
                theme().on_accent
            } else {
                theme().text_disabled
            },
        ))
        .child(ui_text(
            label,
            13.0,
            if active {
                theme().on_accent
            } else {
                theme().text
            },
        ));
    if active {
        row = row.bg(theme().accent);
    }
    if let Some(count) = count {
        row = row.child(div().flex_1()).child(ui_text(
            count.to_string(),
            11.0,
            if active {
                rgb(0xffffff)
            } else {
                theme().text_disabled
            },
        ));
    }
    row
}

fn page_row(state: &AppState, summary: &Summary, active: bool) -> Div {
    let glyph = if active { "◈" } else { "◇" };
    let state = state.clone();
    let summary = summary.clone();
    let click_summary = summary.clone();
    let mut row = div()
        .h(29.0_f32)
        .px(9.0_f32)
        .rounded(6.0_f32)
        .flex()
        .flex_row()
        .items_center()
        .gap(8.0_f32)
        .hover(|style| style.bg(theme().surface_muted))
        .on_click(move |_| state.open_summary(&click_summary))
        .child(ui_text(
            glyph,
            12.0,
            if active {
                rgb(0xffffff)
            } else {
                theme().text_disabled
            },
        ))
        .child(ui_text(
            truncate_chars(&summary.title, 18),
            13.0,
            if active { rgb(0xffffff) } else { theme().text },
        ));
    if active {
        row = row.bg(theme().accent);
    }
    row
}

fn snippet_row(state: &AppState, summary: &Summary) -> Div {
    let state = state.clone();
    let summary = summary.clone();
    let click_summary = summary.clone();
    div()
        .h(29.0_f32)
        .px(9.0_f32)
        .rounded(6.0_f32)
        .flex()
        .flex_row()
        .items_center()
        .gap(8.0_f32)
        .hover(|style| style.bg(theme().surface_muted))
        .on_click(move |_| state.open_summary(&click_summary))
        .child(ui_text("▤", 11.0, theme().text_disabled))
        .child(ui_text(
            truncate_chars(&summary.title, 20),
            13.0,
            theme().text,
        ))
}

// ---------------------------------------------------------------------------
// Tabs and article column
// ---------------------------------------------------------------------------

fn editor_column(state: &AppState) -> Div {
    div()
        .flex_1()
        .min_w(0.0_f32)
        .min_h(0.0_f32)
        .flex()
        .flex_col()
        .bg(theme().page)
        .child(tabs(state))
        .child(
            div()
                .flex_1()
                .min_h(0.0_f32)
                .overflow_y_scroll()
                .child(article_scroll(state)),
        )
}

fn tabs(state: &AppState) -> Div {
    let tabs = state.tabs.get();
    let active = state.active_index();
    let mut row = div()
        .h(36.0_f32)
        .px(10.0_f32)
        .flex()
        .flex_row()
        .items_end()
        .bg(theme().surface)
        .border_b(1.0_f32)
        .border_color(theme().border);
    for (index, tab) in tabs.iter().enumerate() {
        let is_active = index == active;
        let mut tab_view = div()
            .h(29.0_f32)
            .px(10.0_f32)
            .rounded_t(6.0_f32)
            .flex()
            .flex_row()
            .items_center()
            .gap(8.0_f32)
            .hover(|style| style.bg(theme().surface_muted))
            .on_click({
                let state = state.clone();
                move |_| state.switch_tab(index)
            })
            .child(ui_text(
                &tab.title,
                12.0,
                if is_active {
                    theme().text
                } else {
                    theme().text_muted
                },
            ))
            .child(
                div()
                    .px(2.0_f32)
                    .rounded(3.0_f32)
                    .hover(|style| style.bg(theme().surface_muted))
                    .on_click({
                        let state = state.clone();
                        move |event| {
                            event.stop_propagation();
                            state.close_tab(index);
                        }
                    })
                    .child(ui_text("×", 12.0, theme().text_disabled)),
            );
        if is_active {
            tab_view = tab_view
                .bg(theme().page)
                .border(1.0_f32)
                .border_color(theme().border)
                .mb(-1.0_f32);
        }
        row = row.child(tab_view);
    }
    row.child(
        div()
            .w(26.0_f32)
            .h(26.0_f32)
            .mb(4.0_f32)
            .ml(4.0_f32)
            .rounded(5.0_f32)
            .flex()
            .items_center()
            .justify_center()
            .hover(|style| style.bg(theme().surface_muted))
            .on_click({
                let state = state.clone();
                move |_| state.open_palette(PaletteMode::QuickOpen)
            })
            .child(ui_text("＋", 15.0, theme().text_disabled)),
    )
    .child(div().flex_1())
    .child(div().pb(7.0_f32).pr(2.0_f32).child(ui_text(
        "Ctrl+W 閉じる",
        9.5,
        theme().text_disabled,
    )))
}

fn article_scroll(state: &AppState) -> Div {
    let tab = state.active_tab();
    let index = state.active_index();
    let body = match tab {
        Some(tab) => match tab.workspace {
            Some(workspace) => article(state, &workspace),
            None => placeholder(state, index, &tab),
        },
        None => div()
            .p(40.0_f32)
            .child(ui_text("タブがありません。", 13.0, theme().text_muted)),
    };
    div()
        .w_full()
        .node_ref(state.article_ref)
        .flex()
        .justify_center()
        .child(
            div()
                .w_full()
                .max_w(700.0_f32)
                .px(48.0_f32)
                .py(44.0_f32)
                .child(body),
        )
}

fn placeholder(state: &AppState, index: usize, tab: &Tab) -> Div {
    let error = tab
        .error
        .clone()
        .unwrap_or_else(|| "このページはまだ読み込まれていません。".to_owned());
    div()
        .flex()
        .flex_col()
        .child(ui_text("ノート", 11.0, theme().text_disabled))
        .child(
            ui_text(&tab.title, 29.0, theme().text)
                .mt(9.0_f32)
                .font_weight(FontWeight::BOLD),
        )
        .child(
            div()
                .mt(20.0_f32)
                .p(18.0_f32)
                .rounded(8.0_f32)
                .bg(theme().surface_muted)
                .border(1.0_f32)
                .border_color(theme().border)
                .child(ui_text("プレースホルダ", 11.0, theme().accent_text))
                .child(ui_text(error, 12.5, theme().text_muted).mt(6.0_f32))
                .child(
                    div()
                        .mt(12.0_f32)
                        .h(26.0_f32)
                        .px(9.0_f32)
                        .rounded(5.0_f32)
                        .bg(theme().surface)
                        .flex()
                        .items_center()
                        .hover(|style| style.bg(theme().hover))
                        .on_click({
                            let state = state.clone();
                            move |_| {
                                state.tabs.update(|tabs| {
                                    if let Some(tab) = tabs.get_mut(index) {
                                        tab.error = None;
                                    }
                                });
                                state.ensure_workspace(index);
                            }
                        })
                        .child(ui_text("再読み込み", 11.5, theme().text)),
                ),
        )
}

fn article(state: &AppState, workspace: &Workspace) -> Div {
    let page = &workspace.page;
    let info = &workspace.info;
    let tags = if !info.tags.trim().is_empty() {
        tag_text(&info.tags)
    } else {
        tag_text(&page.tags.join(", "))
    };

    let mut body = div()
        .flex()
        .flex_col()
        .child(ui_text("ノート", 11.0, theme().text_muted))
        .child(
            ui_text(&page.title, 29.0, theme().text)
                .mt(9.0_f32)
                .font_weight(FontWeight::BOLD)
                .letter_spacing(-0.6_f32),
        )
        .child(
            div()
                .mt(9.0_f32)
                .flex()
                .flex_row()
                .items_center()
                .flex_wrap()
                .gap(7.0_f32)
                .child(ui_text(
                    format!("更新 {}", info.updated),
                    12.0,
                    theme().text_muted,
                ))
                .child(meta_sep())
                .child(ui_text(
                    format!("作成 {}", short_date(&info.created)),
                    12.0,
                    theme().text_muted,
                ))
                .child(meta_sep())
                .child(ui_text(
                    format!("{}語", info.word_count),
                    12.0,
                    theme().text_muted,
                ))
                .child(meta_sep())
                .child(ui_text(
                    format!("{} バックリンク", workspace.links.backlinks.len()),
                    12.0,
                    theme().text_muted,
                ))
                .child(meta_sep())
                .child(ui_text(
                    if tags.is_empty() {
                        "タグなし".to_owned()
                    } else {
                        tags
                    },
                    12.0,
                    theme().text_muted,
                )),
        );

    body = body.child(render_markdown_body(state, workspace));

    body
}

#[derive(Debug)]
enum MdBlock {
    Heading(u8, String),
    Paragraph(String),
    List(bool, Vec<String>),
    Quote(Vec<String>),
    Code(String, Vec<String>),
    Table(Vec<Vec<String>>),
    Rule,
    Embed(String),
    Transclude(String),
    Interpreter(String, String),
}

fn render_markdown_body(state: &AppState, workspace: &Workspace) -> Div {
    let page = &workspace.page;
    let mut out = div().flex().flex_col();
    if page.content_type == "snippet" {
        let language = page
            .language
            .clone()
            .or_else(|| page.format.clone())
            .unwrap_or_else(|| "text".to_owned());
        let lines = page.body.lines().collect::<Vec<_>>();
        return out.child(code_panel(&language, &lines).mt(24.0_f32));
    }
    for block in parse_markdown(&page.body, &page.title) {
        match block {
            MdBlock::Heading(level, text) => {
                let size = match level {
                    2 => 24.0,
                    3 => 19.0,
                    4 => 16.0,
                    _ => 15.0,
                };
                out = out.child(
                    ui_text(text, size, theme().text)
                        .mt(if level <= 2 { 26.0_f32 } else { 20.0_f32 })
                        .font_weight(FontWeight::SEMIBOLD),
                );
            }
            MdBlock::Paragraph(text) => {
                out = out.child(
                    ui_text(strip_inline(&text), 15.5, theme().text)
                        .mt(14.0_f32)
                        .line_height(LineHeight::Relative(1.68_f32)),
                );
            }
            MdBlock::List(ordered, items) => {
                let mut list = div().mt(12.0_f32).flex().flex_col().gap(5.0_f32);
                for (index, item) in items.iter().enumerate() {
                    let marker = if ordered {
                        format!("{}.", index + 1)
                    } else {
                        "•".to_owned()
                    };
                    list = list.child(
                        div()
                            .flex()
                            .flex_row()
                            .items_start()
                            .gap(8.0_f32)
                            .child(ui_text(marker, 14.0, theme().text_disabled).min_w(18.0_f32))
                            .child(
                                ui_text(strip_inline(item), 14.5, theme().text)
                                    .line_height(LineHeight::Relative(1.6_f32)),
                            ),
                    );
                }
                out = out.child(list);
            }
            MdBlock::Quote(lines) => {
                out = out.child(
                    div()
                        .mt(18.0_f32)
                        .pl(14.0_f32)
                        .border_l(2.0_f32)
                        .border_color(theme().border)
                        .child(
                            ui_text(strip_inline(&lines.join("\n")), 14.0, theme().text_muted)
                                .line_height(LineHeight::Relative(1.65_f32)),
                        ),
                );
            }
            MdBlock::Code(language, lines) => {
                let refs = lines.iter().map(String::as_str).collect::<Vec<_>>();
                out = out.child(code_panel(&language, &refs));
            }
            MdBlock::Table(rows) => {
                if rows.is_empty() {
                    continue;
                }
                let mut table = div()
                    .mt(16.0_f32)
                    .rounded(8.0_f32)
                    .border(1.0_f32)
                    .border_color(theme().border)
                    .overflow_hidden()
                    .flex()
                    .flex_col();
                for (row_index, row) in rows.iter().enumerate() {
                    let mut table_row = div()
                        .flex()
                        .flex_row()
                        .border_b(1.0_f32)
                        .border_color(theme().border);
                    for cell in row {
                        table_row =
                            table_row.child(div().flex_1().px(9.0_f32).py(6.0_f32).child(ui_text(
                                strip_inline(cell),
                                11.5,
                                if row_index == 0 {
                                    theme().text
                                } else {
                                    theme().text_muted
                                },
                            )));
                    }
                    table = table.child(table_row);
                }
                out = out.child(table);
            }
            MdBlock::Rule => {
                out = out.child(div().h(1.0_f32).my(20.0_f32).bg(theme().border));
            }
            MdBlock::Embed(id) => {
                let snippet = workspace
                    .embeds
                    .iter()
                    .map(|embed| &embed.snippet)
                    .find(|snippet| snippet.id == id)
                    .or_else(|| {
                        workspace
                            .snippet
                            .as_ref()
                            .filter(|snippet| snippet.id == id)
                    })
                    .cloned();
                if let Some(snippet) = snippet {
                    out = out.child(embed(state, &snippet));
                } else {
                    out = out.child(
                        ui_text(
                            format!("埋め込みを表示できません: {id}"),
                            12.0,
                            theme().text_disabled,
                        )
                        .mt(16.0_f32),
                    );
                }
            }
            MdBlock::Transclude(target) => {
                let body = if matches!(state.source.get_untracked(), Source::Server) {
                    rpc("transclude", json!({ "id": target, "depth": 1 }))
                        .ok()
                        .and_then(|value| {
                            value.get("body").and_then(Value::as_str).map(str::to_owned)
                        })
                } else {
                    None
                };
                if let Some(body) = body {
                    out = out.child(
                        div()
                            .mt(18.0_f32)
                            .p(12.0_f32)
                            .rounded(8.0_f32)
                            .bg(theme().surface_muted)
                            .border(1.0_f32)
                            .border_color(theme().border)
                            .child(ui_text(strip_inline(&body), 13.0, theme().text_muted))
                            .line_height(LineHeight::Relative(1.6_f32)),
                    );
                } else {
                    out = out.child(
                        ui_text(
                            format!("参照を解決できません: {target}"),
                            13.0,
                            theme().text_disabled,
                        )
                        .mt(14.0_f32),
                    );
                }
            }
            MdBlock::Interpreter(provider, args) => {
                out = out.child(render_interpreter(state, workspace, &provider, &args));
            }
        }
    }
    out
}

fn render_interpreter(state: &AppState, workspace: &Workspace, provider: &str, args: &str) -> Div {
    let mut params = json!({
        "name": provider,
        "id": workspace.page.id,
    });
    if let Some(depth) = args
        .split([',', ' '])
        .find_map(|part| part.strip_prefix("depth:"))
    {
        if let Ok(depth) = depth.trim().parse::<u64>() {
            params["depth"] = json!(depth.min(3));
        }
    }
    let display = if matches!(state.source.get_untracked(), Source::Server) {
        rpc("provider_call", params).ok().and_then(|value| {
            value
                .get("display")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
    } else {
        None
    };
    let body = display.unwrap_or_else(|| format!("provider {provider} の結果は取得できません"));
    div()
        .mt(18.0_f32)
        .rounded(8.0_f32)
        .border(1.0_f32)
        .border_color(theme().border)
        .overflow_hidden()
        .child(
            div()
                .px(12.0_f32)
                .py(6.0_f32)
                .bg(theme().surface_muted)
                .child(mono_text(provider, 10.0, theme().text_disabled)),
        )
        .child(
            div()
                .p(12.0_f32)
                .child(ui_text(strip_inline(&body), 12.5, theme().text_muted)),
        )
}

fn code_panel(language: &str, lines: &[&str]) -> Div {
    div()
        .mt(22.0_f32)
        .rounded(10.0_f32)
        .bg(theme().surface_muted)
        .overflow_hidden()
        .child(
            div()
                .px(14.0_f32)
                .py(8.0_f32)
                .flex()
                .justify_between()
                .child(mono_text(language, 10.0, theme().text_disabled)),
        )
        .child(
            div()
                .px(14.0_f32)
                .pb(11.0_f32)
                .child(code_block(lines, language)),
        )
}

fn parse_markdown(body: &str, title: &str) -> Vec<MdBlock> {
    let lines = body.lines().collect::<Vec<_>>();
    let calls = parse_interpreter_calls(body);
    let mut blocks = Vec::new();
    let mut index = 0usize;
    while index < lines.len() {
        let raw = lines[index].trim_end_matches('\r');
        let line = raw.trim();
        if line.is_empty() {
            index += 1;
            continue;
        }
        if let Some(info) = line.strip_prefix("!```") {
            let id = info.split("||").nth(1).map(str::trim).unwrap_or_default();
            index += 1;
            while index < lines.len() && !lines[index].trim_start().starts_with("```") {
                index += 1;
            }
            index += 1;
            if !id.is_empty() {
                blocks.push(MdBlock::Embed(id.to_owned()));
            }
            continue;
        }
        if let Some(info) = line.strip_prefix("```") {
            let language = info.split('|').next().unwrap_or("text").trim().to_owned();
            index += 1;
            let mut code = Vec::new();
            while index < lines.len() && !lines[index].trim_start().starts_with("```") {
                code.push(lines[index].trim_end_matches('\r').to_owned());
                index += 1;
            }
            index += 1;
            blocks.push(MdBlock::Code(language, code));
            continue;
        }
        if let Some(call) = calls.iter().find(|call| call.line == index + 1) {
            blocks.push(MdBlock::Interpreter(
                call.provider.clone(),
                call.args.clone(),
            ));
            index += 1;
            continue;
        }
        if let Some((level, text)) = heading(line) {
            index += 1;
            if level == 1 && text == title {
                continue;
            }
            blocks.push(MdBlock::Heading(level, text.to_owned()));
            continue;
        }
        if line.starts_with('>') {
            let mut quote = Vec::new();
            while index < lines.len() {
                let candidate = lines[index].trim();
                let Some(text) = candidate.strip_prefix('>') else {
                    break;
                };
                quote.push(text.trim_start().to_owned());
                index += 1;
            }
            blocks.push(MdBlock::Quote(quote));
            continue;
        }
        if let Some((ordered, items)) = list_block(&lines, index) {
            let consumed = items.len();
            index += consumed;
            blocks.push(MdBlock::List(ordered, items));
            continue;
        }
        if line.contains('|') && index + 1 < lines.len() && is_table_separator(lines[index + 1]) {
            let mut rows = Vec::new();
            while index < lines.len() && lines[index].contains('|') {
                let cells = table_cells(lines[index]);
                if !is_table_separator(lines[index]) {
                    rows.push(cells);
                }
                index += 1;
            }
            blocks.push(MdBlock::Table(rows));
            continue;
        }
        if matches!(line, "---" | "***" | "___") {
            index += 1;
            blocks.push(MdBlock::Rule);
            continue;
        }
        if let Some(target) = line.strip_prefix("![[") {
            if let Some(target) = target.strip_suffix("]]") {
                index += 1;
                blocks.push(MdBlock::Transclude(target.to_owned()));
                continue;
            }
        }
        let mut paragraph = Vec::new();
        while index < lines.len() {
            let candidate = lines[index].trim();
            if candidate.is_empty()
                || candidate.starts_with('#')
                || candidate.starts_with("```")
                || candidate.starts_with("!```")
                || candidate.starts_with('>')
                || heading(candidate).is_some()
                || list_block(&lines, index).is_some()
            {
                break;
            }
            paragraph.push(candidate.to_owned());
            index += 1;
        }
        if paragraph.is_empty() {
            index += 1;
        } else {
            blocks.push(MdBlock::Paragraph(paragraph.join(" ")));
        }
    }
    blocks
}

fn heading(line: &str) -> Option<(u8, &str)> {
    let hashes = line.bytes().take_while(|byte| *byte == b'#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = line[hashes..].strip_prefix(' ')?;
    Some((hashes as u8, rest.trim()))
}

fn list_block(lines: &[&str], start: usize) -> Option<(bool, Vec<String>)> {
    let first = lines.get(start)?.trim();
    let ordered = first.split_once('.').is_some_and(|(number, rest)| {
        !number.is_empty()
            && number.bytes().all(|byte| byte.is_ascii_digit())
            && rest.starts_with(' ')
    });
    let bullet = first.starts_with("- ") || first.starts_with("* ") || ordered;
    if !bullet {
        return None;
    }
    let mut items = Vec::new();
    let mut index = start;
    while index < lines.len() {
        let line = lines[index].trim();
        if let Some(item) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            items.push(item.to_owned());
            index += 1;
            continue;
        }
        if let Some((number, rest)) = line.split_once('.')
            && !number.is_empty()
            && number.bytes().all(|byte| byte.is_ascii_digit())
            && let Some(item) = rest.strip_prefix(' ')
        {
            items.push(item.to_owned());
            index += 1;
            continue;
        }
        break;
    }
    (!items.is_empty()).then_some((ordered, items))
}

fn is_table_separator(line: &str) -> bool {
    let line = line.trim();
    line.contains('-')
        && line
            .chars()
            .all(|character| matches!(character, '-' | ':' | '|' | ' '))
}

fn table_cells(line: &str) -> Vec<String> {
    line.trim()
        .trim_matches('|')
        .split('|')
        .map(|cell| cell.trim().to_owned())
        .collect()
}

fn strip_inline(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("[[") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("]]") else {
            out.push_str(&rest[start..]);
            return out.replace("**", "").replace('`', "");
        };
        let inner = &after[..end];
        let label = inner
            .rsplit_once('|')
            .map(|(_, label)| label)
            .unwrap_or(inner);
        out.push_str(label);
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out.replace("**", "").replace('`', "")
}
fn quote_for(workspace: &Workspace) -> Option<(String, String)> {
    for line in &workspace.before {
        if let Some(quote) = line.trim_start().strip_prefix('>') {
            let quote = quote.trim().to_owned();
            if !quote.is_empty() {
                return Some((quote, "引用".to_owned()));
            }
        }
    }
    None
}

fn embed(state: &AppState, snippet: &Content) -> Div {
    let language = snippet
        .language
        .clone()
        .or_else(|| snippet.format.clone())
        .unwrap_or_else(|| "text".to_owned());
    let kinds = snippet
        .kinds
        .iter()
        .filter(|kind| kind.as_str() != "wiki")
        .cloned()
        .collect::<Vec<_>>()
        .join(" · ");
    let lines = snippet.body.lines().collect::<Vec<_>>();
    let meta = if kinds.is_empty() {
        format!(
            "{} · {}",
            snippet.format.clone().unwrap_or_default(),
            language
        )
    } else {
        format!("{} · {}", snippet.format.clone().unwrap_or_default(), kinds)
    };
    let id = snippet.id.clone();
    let title = snippet.title.clone();
    div()
        .mt(24.0_f32)
        .rounded(10.0_f32)
        .bg(theme().surface_muted)
        .overflow_hidden()
        .hover(|style| style.bg(theme().hover))
        .on_click({
            let state = state.clone();
            move |_| state.open_editor_for(&id, &title)
        })
        .child(
            div()
                .px(14.0_f32)
                .pt(11.0_f32)
                .pb(9.0_f32)
                .flex()
                .flex_row()
                .items_center()
                .gap(8.0_f32)
                .child(
                    ui_text(&snippet.title, 13.0, theme().text).font_weight(FontWeight::SEMIBOLD),
                )
                .child(mono_text(
                    meta.trim_matches(' ').trim_matches('·'),
                    10.0,
                    theme().text_disabled,
                )),
        )
        .child(
            div()
                .px(14.0_f32)
                .pb(11.0_f32)
                .child(code_block(&lines, &language)),
        )
        .child(
            div()
                .px(14.0_f32)
                .pb(11.0_f32)
                .flex()
                .justify_between()
                .child(mono_text(
                    short_id(&snippet.id),
                    10.0,
                    theme().text_disabled,
                ))
                .child(ui_text(
                    format!("最終更新 {}", snippet.updated_label),
                    10.5,
                    theme().text_disabled,
                )),
        )
}

fn balance_section() -> Div {
    div()
        .mt(30.0_f32)
        .child(
            div()
                .pb(7.0_f32)
                .border_b(1.0_f32)
                .border_color(theme().border)
                .flex()
                .justify_between()
                .child(ui_text("今月の残高", 13.0, theme().text).font_weight(FontWeight::SEMIBOLD))
                .child(mono_text("ledger/balance", 11.0, theme().text_disabled)),
        )
        .child(
            div()
                .pt(13.0_f32)
                .flex()
                .flex_row()
                .items_end()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(
                            ui_text("—", 39.0, theme().text)
                                .font_weight(FontWeight::SEMIBOLD)
                                .letter_spacing(-1.0_f32),
                        )
                        .child(ui_text("未取得", 12.0, theme().text_muted).mt(6.0_f32)),
                )
                .child(
                    div()
                        .pb(2.0_f32)
                        .flex()
                        .items_center()
                        .gap(6.0_f32)
                        .child(dot(theme().success))
                        .child(ui_text("未取得", 11.0, theme().text_muted)),
                ),
        )
}

// ---------------------------------------------------------------------------
// Outline
// ---------------------------------------------------------------------------

fn outline_column(state: &AppState) -> Div {
    let outline = state
        .active_workspace()
        .map(|workspace| workspace.outline)
        .unwrap_or_default();
    let mut items = outline
        .iter()
        .filter(|item| item.level >= 2)
        .collect::<Vec<_>>();
    if items.is_empty() {
        items = outline.iter().collect();
    }
    let mut nav = div().flex().flex_col().gap(1.0_f32);
    let active_outline = state.outline_active.get();
    for (index, item) in items.iter().enumerate() {
        let active = index == active_outline;
        nav = nav.child(
            div()
                .py(5.0_f32)
                .pl(9.0_f32)
                .pr(6.0_f32)
                .border_l(2.0_f32)
                .border_color(if active {
                    theme().accent
                } else {
                    Color::TRANSPARENT
                })
                .rounded(4.0_f32)
                .hover(|style| style.bg(theme().surface_muted))
                .on_click({
                    let state = state.clone();
                    let line = item.line;
                    let index = index;
                    move |_| {
                        state.outline_active.set(index);
                        state.scroll_to_line(line);
                    }
                })
                .child(ui_text(
                    truncate_chars(&item.title, 18),
                    if item.level >= 3 { 11.5 } else { 12.0 },
                    if active {
                        theme().text
                    } else {
                        theme().text_muted
                    },
                )),
        );
    }
    if items.is_empty() {
        nav = nav.child(
            ui_text("本文に目次はありません。", 11.0, theme().text_disabled)
                .px(9.0_f32)
                .py(5.0_f32),
        );
    }
    div()
        .w(176.0_f32)
        .h_full()
        .px(14.0_f32)
        .py(22.0_f32)
        .border_l(1.0_f32)
        .border_color(theme().border)
        .flex()
        .flex_col()
        .justify_center()
        .child(div().mb(9.0_f32).child(section_label("Outline")))
        .child(nav)
}

// ---------------------------------------------------------------------------
// Inspector
// ---------------------------------------------------------------------------

fn inspector_column(state: &AppState) -> Option<Div> {
    if !state.inspector_visible.get() {
        return None;
    }
    let mode = state.inspector.get();
    let mut column = div()
        .w(340.0_f32)
        .h_full()
        .min_h(0.0_f32)
        .px(20.0_f32)
        .py(18.0_f32)
        .bg(theme().surface)
        .border_l(1.0_f32)
        .border_color(theme().border)
        .flex()
        .flex_col()
        .overflow_y_scroll()
        .child(
            div()
                .p(2.0_f32)
                .rounded(7.0_f32)
                .bg(theme().surface_muted)
                .flex()
                .flex_row()
                .gap(2.0_f32)
                .child(segment_button(
                    state,
                    "リンク",
                    "Ctrl+1",
                    Inspector::Links,
                    mode,
                ))
                .child(segment_button(
                    state,
                    "情報",
                    "Ctrl+2",
                    Inspector::Info,
                    mode,
                )),
        )
        .child(div().h(18.0_f32));

    let content = match mode {
        Inspector::Links => inspector_links(state),
        Inspector::Info => inspector_info(state),
    };
    column = column.child(content);
    Some(column)
}

fn segment_button(
    state: &AppState,
    label: &str,
    shortcut: &str,
    value: Inspector,
    current: Inspector,
) -> Div {
    let active = value == current;
    div()
        .flex_1()
        .h(24.0_f32)
        .rounded(5.0_f32)
        .flex()
        .items_center()
        .justify_center()
        .gap(4.0_f32)
        .bg(if active {
            theme().pressed
        } else {
            Color::TRANSPARENT
        })
        .hover(|style| style.bg(theme().pressed))
        .on_click({
            let state = state.clone();
            move |_| state.inspector.set(value)
        })
        .child(ui_text(
            label,
            11.0,
            if active {
                theme().text
            } else {
                theme().text_muted
            },
        ))
        .child(ui_text(
            shortcut,
            8.5,
            if active {
                theme().text_muted
            } else {
                theme().text_disabled
            },
        ))
}

fn inspector_links(state: &AppState) -> Div {
    let workspace = state.active_workspace();
    let Some(workspace) = workspace else {
        return div().child(section_label("リンク")).child(
            ui_text(
                "ページを開くとリンクが表示されます。",
                11.5,
                theme().text_disabled,
            )
            .mt(8.0_f32),
        );
    };
    let two_hop = &workspace.links.two_hop;
    let total = two_hop.len();
    let groups = ordered_groups(&workspace);
    let mut preview = Vec::new();
    let mut seen_vias = BTreeSet::new();
    for (via, items) in &groups {
        if !seen_vias.insert(via.id.clone()) {
            continue;
        }
        if let Some(first) = items.first() {
            preview.push(first.clone());
        }
        if preview.len() == 3 {
            break;
        }
    }
    if preview.len() < 3 {
        for hop in two_hop {
            if preview
                .iter()
                .any(|existing| existing.target.id == hop.target.id)
            {
                continue;
            }
            preview.push(hop.clone());
            if preview.len() == 3 {
                break;
            }
        }
    }
    let shown = preview.len();
    let selected = state.next_selected.get().min(shown.saturating_sub(1));
    let mut content = div().flex().flex_col().child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(4.0_f32)
            .child(section_label("次に読む"))
            .child(ui_text(
                format!("{shown} / {total}"),
                10.0,
                theme().text_disabled,
            )),
    );

    if shown == 0 {
        content = content.child(
            ui_text(
                "2-hop のリンクはまだありません。",
                11.5,
                theme().text_disabled,
            )
            .mt(8.0_f32),
        );
    } else {
        let mut suggestions = div().mt(8.0_f32).flex().flex_col().gap(3.0_f32);
        for (index, hop) in preview.iter().enumerate() {
            suggestions = suggestions.child(suggestion(state, hop, index == selected, index));
        }
        content = content.child(suggestions);
    }

    content = content
        .child(
            div()
                .mt(4.0_f32)
                .flex()
                .items_center()
                .gap(6.0_f32)
                .hover(|style| style.bg(theme().surface_muted))
                .on_click({
                    let state = state.clone();
                    move |_| state.shelf_pinned.set(true)
                })
                .child(ui_text("2-hop をすべて見る", 11.0, theme().accent_text))
                .child(kbd("Ctrl+Alt+L")),
        )
        .child(
            div()
                .mt(7.0_f32)
                .flex()
                .justify_between()
                .child(ui_text("↑ ↓ 選択", 9.0, theme().text_disabled))
                .child(ui_text("↵ 開く", 9.0, theme().text_disabled))
                .child(ui_text("Ctrl+Enter 新規タブ", 9.0, theme().text_disabled)),
        )
        .child(divider())
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(4.0_f32)
                .child(section_label("関連スニペット"))
                .child(ui_text(
                    workspace.related.len().to_string(),
                    10.0,
                    theme().text_disabled,
                )),
        );

    let mut related = div().mt(4.0_f32).flex().flex_col();
    for summary in workspace.related.iter().take(6) {
        related = related.child(link_row(state, summary, &summary.updated_label));
    }
    if workspace.related.is_empty() {
        related = related.child(ui_text(
            "関連スニペットはありません。",
            11.0,
            theme().text_disabled,
        ));
    }
    content = content.child(related).child(divider()).child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(4.0_f32)
            .child(section_label("リンク"))
            .child(ui_text(
                (workspace.links.outgoing.len() + workspace.links.backlinks.len()).to_string(),
                10.0,
                theme().text_disabled,
            )),
    );

    let mut groups = div().mt(4.0_f32).flex().flex_col();
    groups = groups.child(div().mt(6.0_f32).mb(2.0_f32).child(ui_text(
        format!("発リンク {}", workspace.links.outgoing.len()),
        10.0,
        theme().text_disabled,
    )));
    for summary in workspace.links.outgoing.iter().take(8) {
        groups = groups.child(link_row(state, summary, "›"));
    }
    if workspace.links.outgoing.is_empty() {
        groups = groups.child(ui_text(
            "発リンクはありません。",
            11.0,
            theme().text_disabled,
        ));
    }
    groups = groups.child(div().mt(8.0_f32).mb(2.0_f32).child(ui_text(
        format!("バックリンク {}", workspace.links.backlinks.len()),
        10.0,
        theme().text_disabled,
    )));
    for summary in workspace.links.backlinks.iter().take(8) {
        groups = groups.child(link_row(state, summary, "›"));
    }
    if workspace.links.backlinks.is_empty() {
        groups = groups.child(ui_text(
            "バックリンクはありません。",
            11.0,
            theme().text_disabled,
        ));
    }
    content.child(groups)
}

fn suggestion(state: &AppState, hop: &TwoHop, selected: bool, index: usize) -> Div {
    let mut row = div()
        .rounded(6.0_f32)
        .px(12.0_f32)
        .py(10.0_f32)
        .flex()
        .flex_col()
        .hover(|style| style.bg(theme().surface_muted))
        .on_click({
            let state = state.clone();
            let target = hop.target.clone();
            move |_| {
                state.next_selected.set(index);
                state.open_summary(&target);
            }
        })
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(5.0_f32)
                .child(ui_text(
                    format!("{} から", hop.via.title),
                    9.5,
                    theme().text_muted,
                ))
                .child(chip("2-hop", theme().text_disabled)),
        )
        .child(
            ui_text(&hop.target.title, 12.5, theme().text)
                .mt(3.0_f32)
                .font_weight(FontWeight::SEMIBOLD),
        )
        .child(
            ui_text(
                truncate_chars(&hop.target.excerpt, 64),
                10.5,
                theme().text_muted,
            )
            .mt(3.0_f32)
            .line_height(LineHeight::Relative(1.45_f32)),
        )
        .child(ui_text(next_reason(hop), 9.5, theme().text_disabled).mt(4.0_f32));
    if selected {
        row = row.bg(theme().surface_muted);
    }
    row
}

fn next_reason(hop: &TwoHop) -> String {
    if hop.target.refs > 0 {
        format!("{} リンク", hop.target.refs)
    } else if !hop.reason.is_empty() && hop.reason != hop.target.excerpt {
        truncate_chars(&hop.reason, 72)
    } else {
        format!("共通タグ: {}", hop.target.kind_label())
    }
}

/// Group two-hop links by the page they were reached through. The outgoing order
/// is the reading order of the host page, so it decides the group order too.
fn ordered_groups(workspace: &Workspace) -> Vec<(Summary, Vec<TwoHop>)> {
    let mut groups: Vec<(Summary, Vec<TwoHop>)> = Vec::new();
    for hop in &workspace.links.two_hop {
        if let Some((_, items)) = groups.iter_mut().find(|(via, _)| via.id == hop.via.id) {
            items.push(hop.clone());
        } else {
            groups.push((hop.via.clone(), vec![hop.clone()]));
        }
    }
    let mut ordered = Vec::new();
    for via in &workspace.links.outgoing {
        if let Some(position) = groups
            .iter()
            .position(|(candidate, _)| candidate.id == via.id)
        {
            ordered.push(groups.remove(position));
        }
    }
    ordered.extend(groups);
    ordered
}

fn link_row(state: &AppState, summary: &Summary, arrow: &str) -> Div {
    let state = state.clone();
    let summary = summary.clone();
    let click_summary = summary.clone();
    let arrow = arrow.to_owned();
    div()
        .py(4.0_f32)
        .flex()
        .flex_row()
        .items_center()
        .gap(8.0_f32)
        .border_b(1.0_f32)
        .border_color(rgb(0x2f2f31))
        .hover(|style| style.bg(theme().surface_muted))
        .on_click(move |_| state.open_summary(&click_summary))
        .child(ui_text(truncate_chars(&summary.title, 30), 11.5, theme().text_muted).flex_1())
        .child(ui_text(arrow, 11.0, theme().text_disabled))
}

fn divider() -> Div {
    div().my(16.0_f32).h(1.0_f32).bg(theme().border)
}

fn inspector_info(state: &AppState) -> Div {
    let workspace = state.active_workspace();
    let editor = state.editor.get();
    let source = state.source.get();
    let Some(workspace) = workspace else {
        return div().child(section_label("ページ情報")).child(
            ui_text(
                "ページを開くと情報が表示されます。",
                11.5,
                theme().text_disabled,
            )
            .mt(8.0_f32),
        );
    };
    let info = &workspace.info;
    let tags = if info.tags.trim().is_empty() {
        workspace.page.tags.join(", ")
    } else {
        info.tags.clone()
    };
    let editor_status = if let Some(error) = &editor.error {
        format!("エラー: {error}")
    } else if editor.attached {
        format!("{} を編集中", workspace.page.title)
    } else {
        "未接続（Ctrl+E で起動）".to_owned()
    };

    let mut content = div()
        .flex()
        .flex_col()
        .child(section_label("ページ情報"))
        .child(info_row("作成", &short_date(&info.created)))
        .child(info_row("更新", &info.updated))
        .child(info_row("語数", &info.word_count.to_string()))
        .child(info_row(
            "タグ",
            if tags.is_empty() { "なし" } else { &tags },
        ))
        .child(info_row(
            "種類",
            if workspace.page.content_type.is_empty() {
                "page"
            } else {
                &workspace.page.content_type
            },
        ))
        .child(divider())
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(4.0_f32)
                .child(section_label("接続"))
                .child(ui_text(format!("{}", state.providers.get().len()), 10.0, theme().text_disabled)),
        )
        .child(
            div()
                .mt(6.0_f32)
                .flex()
                .items_center()
                .gap(7.0_f32)
                .child(dot(if state.providers.get().is_empty() { theme().text_disabled } else { theme().success }))
                .child(ui_text(
                    if state.providers.get().is_empty() { "provider 状態は未取得" } else { "ローカル provider を検出" },
                    12.0,
                    theme().text_muted,
                )),
        )
        .child(
            ui_text(
                if state.providers.get().is_empty() {
                    "provider を取得できません".to_owned()
                } else {
                    state.providers.get().join(" · ")
                },
                10.5,
                theme().text_disabled,
            )
            .mt(6.0_f32)
            .line_height(LineHeight::Relative(1.55_f32)),
        )
        .child(divider())
        .child(section_label("エディタ"))
        .child(info_row("Neovide", &editor_status))
        .child(
            ui_text(
                if let Some(warning) = &editor.warning {
                    format!("タブを切り替えると追従します。注意: {warning}")
                } else {
                    "タブを切り替えると、Neovide も同じ Content に追従します。".to_owned()
                },
                10.5,
                theme().text_disabled,
            )
            .mt(6.0_f32)
            .line_height(LineHeight::Relative(1.55_f32)),
        )
        .child(divider())
        .child(section_label("状態"))
        .child(
            div()
                .mt(6.0_f32)
                .flex()
                .items_center()
                .gap(7.0_f32)
                .child(dot(if source == Source::Fallback {
                    rgb(0xe0af68)
                } else {
                    theme().success
                }))
                .child(ui_text(
                    if source == Source::Fallback {
                        "フォールバック · 未接続"
                    } else {
                        "閲覧ビュー · 保存は Neovim"
                    },
                    12.0,
                    theme().text_muted,
                )),
        )
        .child(
            ui_text(
                if source == Source::Fallback {
                    format!(
                        "サーバーに接続できないため、内蔵 fixture を表示しています。\n{}",
                        state.boot_error.clone().unwrap_or_default()
                    )
                } else {
                    format!("競合情報は未取得\n{}", state.socket)
                },
                10.5,
                theme().text_disabled,
            )
            .mt(6.0_f32)
            .line_height(LineHeight::Relative(1.5_f32)),
        );
    if let Some(error) = &editor.error {
        content = content.child(
            ui_text(error, 10.5, rgb(0xff453a))
                .mt(8.0_f32)
                .line_height(LineHeight::Relative(1.45_f32)),
        );
    }
    if let Some(workspace) = state.active_workspace() {
        let id = workspace.page.id.clone();
        let armed = state.delete_armed.get();
        content = content.child(divider()).child(
            div()
                .flex()
                .flex_row()
                .gap(6.0_f32)
                .child(
                    div()
                        .flex_1()
                        .h(28.0_f32)
                        .rounded(5.0_f32)
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(theme().surface_muted)
                        .hover(|style| style.bg(theme().hover))
                        .on_click({
                            let state = state.clone();
                            move |_| state.open_palette(PaletteMode::Rename)
                        })
                        .child(ui_text("名前", 11.5, theme().text_muted)),
                )
                .child(
                    div()
                        .flex_1()
                        .h(28.0_f32)
                        .rounded(5.0_f32)
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(theme().surface_muted)
                        .hover(|style| style.bg(theme().hover))
                        .on_click({
                            let state = state.clone();
                            move |_| state.open_palette(PaletteMode::Tags)
                        })
                        .child(ui_text("タグ", 11.5, theme().text_muted)),
                ),
        );
        content = content.child(divider()).child(
            div()
                .h(28.0_f32)
                .px(9.0_f32)
                .rounded(5.0_f32)
                .flex()
                .items_center()
                .justify_center()
                .bg(if armed {
                    rgb(0x5a1d1d)
                } else {
                    theme().surface_muted
                })
                .border(1.0_f32)
                .border_color(if armed { rgb(0xff453a) } else { theme().border })
                .hover(|style| style.bg(if armed { rgb(0x7a2424) } else { theme().hover }))
                .on_click({
                    let state = state.clone();
                    move |_| {
                        if state.delete_armed.get_untracked() {
                            state.delete_content(&id);
                        } else {
                            state.delete_armed.set(true);
                        }
                    }
                })
                .child(ui_text(
                    if armed { "本当に削除" } else { "削除" },
                    11.5,
                    if armed {
                        rgb(0xffb4b4)
                    } else {
                        theme().text_muted
                    },
                )),
        );
    }
    content
}

fn info_row(label: &str, value: &str) -> Div {
    div()
        .py(5.0_f32)
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap(12.0_f32)
        .child(ui_text(label, 12.0, theme().text_muted))
        .child(ui_text(value, 12.0, theme().text))
}

// ---------------------------------------------------------------------------
// Two-hop shelf
// ---------------------------------------------------------------------------

fn shelf(state: &AppState) -> Div {
    let pinned = state.shelf_pinned;
    let hovered = state.shelf_hover;
    let count_state = state.clone();
    let body_state = state.clone();
    div()
        .absolute()
        .left(236.0_f32)
        .right(0.0_f32)
        .bottom(0.0_f32)
        .h(SHELF_HEIGHT)
        .z_index(20)
        .bg(theme().surface)
        .border_t(1.0_f32)
        .border_color(theme().border)
        .shadow(BoxShadow {
            offset_x: 0.0,
            offset_y: -16.0,
            blur: 40.0,
            spread: 0.0,
            color: rgba(0x00000057),
            inset: false,
        })
        .transition(Transition::ms(240).cubic_bezier(0.2, 0.8, 0.2, 1.0))
        .dyn_style(move |style| {
            let open = pinned.get() || hovered.get();
            style.translate_y(if open {
                0.0
            } else {
                SHELF_HEIGHT - SHELF_HANDLE
            })
        })
        .on_hover(move |over| hovered.set(over))
        .child(
            div()
                .h(SHELF_HANDLE)
                .px(20.0_f32)
                .flex()
                .flex_row()
                .items_center()
                .gap(9.0_f32)
                .on_click(move |_| pinned.update(|pinned| *pinned = !*pinned))
                .child(
                    ui_text("2-hop リンク", 11.0, theme().text).font_weight(FontWeight::SEMIBOLD),
                )
                .child(dyn_view(move || {
                    let total = count_state
                        .active_workspace()
                        .map(|workspace| workspace.links.two_hop.len())
                        .unwrap_or(0);
                    div()
                        .min_w(17.0_f32)
                        .h(16.0_f32)
                        .px(4.0_f32)
                        .rounded(4.0_f32)
                        .bg(theme().surface_muted)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(ui_text(total.to_string(), 9.0, theme().text_muted))
                }))
                .child(ui_text("すべて見る", 11.0, theme().text_muted))
                .child(div().flex_1())
                .child(ui_text("ホバーで表示 ·", 9.5, theme().text_disabled))
                .child(kbd("Ctrl+Alt+L"))
                .child(dyn_view(move || {
                    let open = pinned.get() || hovered.get();
                    ui_text(if open { "▾" } else { "▴" }, 11.0, theme().text_disabled)
                })),
        )
        .child(
            div()
                .h(SHELF_HEIGHT - SHELF_HANDLE)
                .px(20.0_f32)
                .pt(8.0_f32)
                .pb(14.0_f32)
                .overflow_y_scroll()
                .child(dyn_view(move || shelf_body(&body_state))),
        )
}

fn shelf_body(state: &AppState) -> Div {
    let workspace = state.active_workspace();
    let groups = workspace.as_ref().map(ordered_groups).unwrap_or_default();
    if groups.is_empty() {
        return div().child(ui_text(
            "このページから 2-hop で辿れるリンクはありません。",
            12.0,
            theme().text_disabled,
        ));
    }
    let mut columns = div().flex().flex_col().gap(16.0_f32);
    for chunk in groups.chunks(3) {
        let mut row = div().flex().flex_row().gap(24.0_f32);
        for (via, items) in chunk {
            let mut group = div().flex_1().min_w(0.0_f32).flex().flex_col().child(
                div()
                    .mb(3.0_f32)
                    .px(7.0_f32)
                    .flex()
                    .justify_between()
                    .child(ui_text(
                        format!("{} から", via.title),
                        10.0,
                        theme().text_muted,
                    ))
                    .child(ui_text(
                        items.len().to_string(),
                        10.0,
                        theme().text_disabled,
                    )),
            );
            for hop in items {
                group = group.child(hop_row(state, hop, items.len()));
            }
            row = row.child(group);
        }
        columns = columns.child(row);
    }
    columns
}

fn hop_row(state: &AppState, hop: &TwoHop, group_count: usize) -> Div {
    let state = state.clone();
    let target = hop.target.clone();
    let count = if target.refs > 0 {
        target.refs
    } else {
        group_count
    };
    let kind_tag = target.kind_label();
    div()
        .rounded(6.0_f32)
        .px(8.0_f32)
        .py(7.0_f32)
        .flex()
        .flex_col()
        .hover(|style| style.bg(theme().surface_muted))
        .on_click(move |_| state.open_summary(&target))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(6.0_f32)
                .child(
                    ui_text(truncate_chars(&hop.target.title, 22), 11.5, theme().text)
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .child(mono_text(kind_tag, 8.5, theme().text_disabled)),
        )
        .child(
            ui_text(
                truncate_chars(&hop.target.excerpt, 78),
                10.0,
                theme().text_muted,
            )
            .mt(2.0_f32)
            .line_height(LineHeight::Relative(1.4_f32)),
        )
        .child(
            ui_text(
                format!(
                    "更新 {} · {} リンク · {}",
                    hop.target.updated_label,
                    count,
                    hop.target.kind_label()
                ),
                9.0,
                theme().text_disabled,
            )
            .mt(3.0_f32),
        )
}

// ---------------------------------------------------------------------------
// Palette (non-modal: no backdrop)
// ---------------------------------------------------------------------------

fn palette(state: &AppState) -> Show {
    let show_state = state.clone();
    show(move || show_state.palette.get().is_some(), {
        let state = state.clone();
        move || palette_card(&state)
    })
}

fn palette_card(state: &AppState) -> Div {
    let input = node_ref();
    request_frame(move |_| input.focus());
    let mode = state.palette.get().unwrap_or(PaletteMode::Search);
    let header_state = state.clone();
    let results_state = state.clone();
    div()
        .absolute()
        .top(58.0_f32)
        .left(320.0_f32)
        .w(620.0_f32)
        .z_index(40)
        .rounded(10.0_f32)
        .bg(theme().surface)
        .border(1.0_f32)
        .border_color(theme().border)
        .shadow_lg()
        .p(8.0_f32)
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(7.0_f32)
                .px(8.0_f32)
                .child(ui_text(
                    match mode {
                        PaletteMode::Search => "◎",
                        PaletteMode::Rename | PaletteMode::Tags => "✎",
                        PaletteMode::QuickOpen => "›",
                    },
                    14.0,
                    theme().text_muted,
                ))
                .child(
                    text_input()
                        .value(state.query)
                        .placeholder(match mode {
                            PaletteMode::Search => "検索 — タイトル・本文・タグ",
                            PaletteMode::Rename => "新しいタイトル",
                            PaletteMode::Tags => "タグをカンマ区切りで入力",
                            PaletteMode::QuickOpen => "クイックオープン — ページ名",
                        })
                        .font_family(sans())
                        .text_size(12.5_f32)
                        .text_color(theme().text)
                        .w_full()
                        .h(28.0_f32)
                        .px(6.0_f32)
                        .node_ref(input)
                        .on_input({
                            let state = state.clone();
                            move |text| state.on_query(text)
                        })
                        .on_submit({
                            let state = state.clone();
                            move |_| {
                                let query = state.query.get_untracked();
                                match state.palette.get_untracked() {
                                    Some(PaletteMode::Rename) => state.rename_active(&query),
                                    Some(PaletteMode::Tags) => state.set_tags(&query),
                                    _ => {
                                        if let Some(first) =
                                            state.results.get_untracked().into_iter().next()
                                        {
                                            state.open_summary(&first);
                                        }
                                    }
                                }
                            }
                        })
                        .on_key_down({
                            let state = state.clone();
                            move |event| {
                                if event.key == Key::Named(NamedKey::Escape) {
                                    event.prevent_default();
                                    state.close_palette();
                                }
                            }
                        }),
                )
                .child(kbd("Esc")),
        )
        .child(
            div()
                .px(10.0_f32)
                .pt(6.0_f32)
                .pb(4.0_f32)
                .child(dyn_view(move || {
                    let mode = header_state.palette.get().unwrap_or(PaletteMode::Search);
                    let label = match mode {
                        PaletteMode::Search => "検索",
                        PaletteMode::Rename => "タイトルを変更",
                        PaletteMode::Tags => "タグを変更",
                        PaletteMode::QuickOpen => "クイックオープン",
                    };
                    ui_text(label, 9.5, theme().text_disabled)
                })),
        )
        .child(dyn_view(move || {
            let results = results_state.results.get();
            let error = results_state.palette_error.get();
            let mut list = div().flex().flex_col();
            if let Some(error) = error {
                list = list.child(ui_text(error, 10.5, rgb(0xe0af68)).px(10.0_f32).py(6.0_f32));
            }
            if results.is_empty() {
                list = list.child(
                    ui_text("一致する項目はありません。", 12.0, theme().text_disabled)
                        .px(10.0_f32)
                        .py(8.0_f32),
                );
            }
            for summary in results.iter().take(20) {
                list = list.child(
                    div()
                        .px(10.0_f32)
                        .py(7.0_f32)
                        .rounded(6.0_f32)
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(9.0_f32)
                        .hover(|style| style.bg(theme().surface_muted))
                        .on_click({
                            let state = results_state.clone();
                            let summary = summary.clone();
                            move |_| state.open_summary(&summary)
                        })
                        .child(
                            ui_text(truncate_chars(&summary.title, 32), 12.5, theme().text)
                                .flex_1(),
                        )
                        .child(mono_text(summary.kind_label(), 9.0, theme().text_disabled))
                        .child(ui_text(&summary.updated_label, 10.0, theme().text_disabled)),
                );
            }
            list
        }))
        .child(
            div()
                .px(10.0_f32)
                .pt(5.0_f32)
                .flex()
                .justify_between()
                .child(ui_text("Enter 開く", 9.0, theme().text_disabled))
                .child(ui_text("Esc 閉じる", 9.0, theme().text_disabled))
                .child(ui_text(
                    "Ctrl+K 検索 · Ctrl+P クイックオープン",
                    9.0,
                    theme().text_disabled,
                )),
        )
}

// ---------------------------------------------------------------------------
// Status bar
// ---------------------------------------------------------------------------

fn status_bar(state: &AppState) -> Div {
    let editor = state.editor.get();
    let source = state.source.get();
    let title = state
        .active_tab()
        .map(|tab| tab.title)
        .unwrap_or_else(|| "Wiki".to_owned());
    let error = editor.error.clone();
    div()
        .h(26.0_f32)
        .px(20.0_f32)
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .bg(theme().surface)
        .border_t(1.0_f32)
        .border_color(theme().border)
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(7.0_f32)
                .child(dot(if source == Source::Fallback {
                    rgb(0xe0af68)
                } else {
                    theme().success
                }))
                .child(ui_text(
                    if source == Source::Fallback {
                        "内蔵 fixture  ·  サーバー未接続"
                    } else {
                        "閲覧のみ  ·  保存は Neovim"
                    },
                    9.5,
                    theme().text_muted,
                ))
                .child(if let Some(error) = error {
                    ui_text(truncate_chars(&error, 70), 9.5, rgb(0xff453a))
                } else {
                    ui_text("", 9.5, theme().text_disabled)
                }),
        )
        .child(ui_text(
            format!("provider 未取得  ·  {title}  ·  {}", editor.detail),
            9.5,
            theme().text_disabled,
        ))
}

// ---------------------------------------------------------------------------
// Markdown helpers used by the reading surface
// ---------------------------------------------------------------------------

/// Non-embed fenced code blocks in a page body, with their info string and text.
fn extract_fences(body: &str) -> Vec<(String, String)> {
    let mut blocks = Vec::new();
    let mut current: Option<(String, Vec<String>)> = None;
    for line in body.lines() {
        let trimmed = line.trim_start();
        if let Some((language, lines)) = current.as_mut() {
            if trimmed.starts_with("```") {
                blocks.push((language.clone(), lines.join("\n")));
                current = None;
            } else {
                lines.push(line.to_owned());
            }
            continue;
        }
        if trimmed.starts_with("```") && !trimmed.starts_with("!```") {
            let info = trimmed.trim_start_matches('`').trim();
            let language = info.split('|').next().unwrap_or("text").trim().to_owned();
            current = Some((language, Vec::new()));
        }
    }
    blocks
}

fn excerpt(body: &str) -> String {
    excerpt_of(body)
}

// ---------------------------------------------------------------------------
// Built-in fixture (only used when the server is unavailable)
// ---------------------------------------------------------------------------

fn fixture_summaries() -> Vec<Summary> {
    let mut items = Vec::new();
    let mut page = |id: &str, title: &str, kind: &str, updated: &str, excerpt: &str| {
        items.push(Summary {
            id: id.to_owned(),
            content_type: "page".to_owned(),
            title: title.to_owned(),
            format: None,
            language: None,
            kinds: vec!["wiki".to_owned(), kind.to_owned()],
            tags: Vec::new(),
            aliases: Vec::new(),
            rev: 1,
            updated_label: updated.to_owned(),
            excerpt: excerpt.to_owned(),
            refs: 0,
        });
    };
    page(
        "p-home",
        "家計の記録",
        "ledger",
        "14:31",
        "毎月の支出を hledger のジャーナルで管理している。",
    );
    page(
        "p-goals",
        "2026年の目標",
        "ledger",
        "昨日",
        "家計の記録を毎週見直す。",
    );
    page(
        "p-receipts",
        "レシート処理",
        "ledger",
        "11/20",
        "レシートは週末にまとめて処理する。",
    );
    page(
        "p-fixed",
        "固定費の見直し",
        "ledger",
        "11/19",
        "毎月の固定費を一覧にする。",
    );
    page(
        "p-review",
        "月次レビュー",
        "ledger",
        "11/20",
        "1か月の支出を振り返り、翌月の予算を決める。",
    );
    page(
        "p-reading",
        "読書ノート",
        "note",
        "昨日",
        "読んだ本の要点を残す。",
    );
    page(
        "p-rust",
        "Rustのメモ",
        "code",
        "11/20",
        "所有権と借用のメモ。",
    );
    page(
        "p-credit",
        "クレジットカード",
        "ledger",
        "11/19",
        "引き落とし日と未払いを確認する。",
    );
    page(
        "p-emoney",
        "電子マネー",
        "ledger",
        "11/17",
        "チャージと残高の記録。",
    );
    page(
        "p-returns",
        "返品・交換",
        "ledger",
        "11/10",
        "レシートの保管と期限。",
    );
    page(
        "p-electricity",
        "電気代",
        "ledger",
        "11/16",
        "前月との差が大きい月を振り返る。",
    );
    page(
        "p-gas",
        "ガス代",
        "ledger",
        "11/16",
        "使用量の変化を確認する。",
    );
    page("p-rent", "家賃", "ledger", "10/28", "更新時期と契約条件。");
    page(
        "p-phone",
        "通信費",
        "ledger",
        "11/05",
        "プラン見直しの候補。",
    );
    page(
        "p-sub",
        "サブスク",
        "ledger",
        "11/14",
        "利用状況と解約候補。",
    );
    page(
        "p-savings",
        "貯金の目標",
        "ledger",
        "10/30",
        "先取りで残す額を決める。",
    );
    page(
        "p-household",
        "家計簿の付け方",
        "ledger",
        "11/18",
        "固定費を先に一覧にする。変動費は前月と比べる。",
    );
    page(
        "p-diary",
        "日記 11/22",
        "diary",
        "11/22",
        "今日の出来事を短く残す。",
    );

    let mut snippet = |id: &str,
                       title: &str,
                       format: &str,
                       language: &str,
                       kinds: &[&str],
                       updated: &str,
                       excerpt: &str| {
        items.push(Summary {
            id: id.to_owned(),
            content_type: "snippet".to_owned(),
            title: title.to_owned(),
            format: Some(format.to_owned()),
            language: Some(language.to_owned()),
            kinds: std::iter::once("wiki".to_owned())
                .chain(kinds.iter().map(|kind| (*kind).to_owned()))
                .collect(),
            tags: Vec::new(),
            aliases: Vec::new(),
            rev: 1,
            updated_label: updated.to_owned(),
            excerpt: excerpt.to_owned(),
            refs: 0,
        });
    };
    snippet(
        JOURNAL_ID,
        "買い物ジャーナル",
        "hledger",
        "ledger",
        &["ledger", "free"],
        "14:31",
        "2026-11-16 * 買い物",
    );
    snippet(
        BUDGET_ID,
        "予算クエリ",
        "hledger",
        "ledger",
        &["ledger"],
        "11/18",
        "2026-11-01 * 予算",
    );
    snippet(
        MONTHLY_ID,
        "月次サマリ",
        "hledger",
        "ledger",
        &["ledger"],
        "10/31",
        "2026-10-31 * 集計",
    );
    items
}

fn fixture_summary(id: &str) -> Option<Summary> {
    fixture_summaries()
        .into_iter()
        .find(|summary| summary.id == id)
}

fn fixture_workspace(id: &str, title: &str) -> Option<Workspace> {
    match title {
        "家計の記録" => Some(fixture_home()),
        "読書ノート" => Some(fixture_reading()),
        "Rustのメモ" => Some(fixture_rust()),
        _ => fixture_summary(id)
            .or_else(|| {
                fixture_summaries()
                    .into_iter()
                    .find(|summary| summary.title == title)
            })
            .map(|summary| generic_workspace(&summary)),
    }
}

fn fixture_page(id: &str, title: &str, kind: &str, tags: &[&str], body: &str) -> Content {
    Content {
        id: id.to_owned(),
        content_type: "page".to_owned(),
        title: title.to_owned(),
        body: body.to_owned(),
        format: None,
        language: None,
        kinds: vec!["wiki".to_owned(), kind.to_owned()],
        tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
        aliases: Vec::new(),
        rev: 1,
        created: "2026-10-02".to_owned(),
        updated_at: 202611221431,
        updated_label: "14:31".to_owned(),
    }
}

fn fixture_snippet(id: &str, title: &str, body: &str) -> Content {
    Content {
        id: id.to_owned(),
        content_type: "snippet".to_owned(),
        title: title.to_owned(),
        body: body.to_owned(),
        format: Some("hledger".to_owned()),
        language: Some("ledger".to_owned()),
        kinds: vec!["wiki".to_owned(), "ledger".to_owned(), "free".to_owned()],
        tags: Vec::new(),
        aliases: Vec::new(),
        rev: 1,
        created: "2026-10-02".to_owned(),
        updated_at: 202611221431,
        updated_label: "14:31".to_owned(),
    }
}

fn fixture_home() -> Workspace {
    let page = fixture_page(
        "p-home",
        "家計の記録",
        "ledger",
        &["家計", "ledger", "毎月"],
        "# 家計の記録\n\n毎月の支出を hledger のジャーナルで管理している。\n\n## 買い物ジャーナル\n\n!```hledger|ledger,free||JOURNAL_ID\n```\n\n## 今月の残高\n\n固定費の見直しは月1回、日曜日にまとめて行う。\n",
    );
    let snippet = fixture_snippet(
        JOURNAL_ID,
        "買い物ジャーナル",
        "2026-11-16 * 買い物\n    支出:食費   1,200 JPY\n    資産:現金  -1,200 JPY\n\n2026-11-17 * 電車\n    支出:交通     420 JPY\n    資産:現金    -420 JPY\n\n; 11月分\n",
    );
    let goals = fixture_summary("p-goals").unwrap();
    let receipts = fixture_summary("p-receipts").unwrap();
    let fixed = fixture_summary("p-fixed").unwrap();
    let target = |id: &str, title: &str, kind: &str, updated: &str, excerpt: &str, refs: usize| {
        let mut summary = fixture_summary(id).unwrap();
        summary.title = title.to_owned();
        summary.kinds = vec!["wiki".to_owned(), kind.to_owned()];
        summary.updated_label = updated.to_owned();
        summary.excerpt = excerpt.to_owned();
        summary.refs = refs;
        summary
    };
    let mut two_hop = Vec::new();
    let mut hop = |via: &Summary, target: Summary, reason: &str| {
        two_hop.push(TwoHop {
            via: via.clone(),
            target,
            reason: reason.to_owned(),
        });
    };
    hop(
        &goals,
        target(
            "p-household",
            "家計簿の付け方",
            "ledger",
            "11/18",
            "固定費を先に一覧にする。変動費は前月と比べる。",
            3,
        ),
        "共通タグ: 家計",
    );
    hop(
        &goals,
        target(
            "p-review",
            "毎月のレビュー",
            "ledger",
            "11/20",
            "1か月の支出を振り返り、翌月の予算を決める。",
            2,
        ),
        "共通タグ: 家計",
    );
    hop(
        &receipts,
        target(
            "p-credit",
            "クレジットカード",
            "ledger",
            "11/19",
            "引き落とし日と未払いを確認する。",
            3,
        ),
        "3 リンク",
    );
    hop(
        &fixed,
        target(
            "p-electricity",
            "電気代",
            "ledger",
            "11/16",
            "前月との差が大きい月を振り返る。",
            4,
        ),
        "共通タグ: 固定費",
    );
    let two_hop_all = {
        // The shelf shows all of them; the inspector and fixture summary above only use the
        // first three. Keep the design's twelve rows available for the open shelf.
        let extra = [
            (
                &goals,
                target(
                    "p-fixed-inner",
                    "固定費の内訳",
                    "ledger",
                    "11/12",
                    "家賃・電気・通信の内訳をまとめる。",
                    1,
                ),
            ),
            (
                &goals,
                target(
                    "p-savings",
                    "貯金の目標",
                    "ledger",
                    "10/30",
                    "先取りで残す額を決める。",
                    0,
                ),
            ),
            (
                &receipts,
                target(
                    "p-emoney",
                    "電子マネー",
                    "ledger",
                    "11/17",
                    "チャージと残高の記録。",
                    2,
                ),
            ),
            (
                &receipts,
                target(
                    "p-returns",
                    "返品・交換",
                    "ledger",
                    "11/10",
                    "レシートの保管と期限。",
                    1,
                ),
            ),
            (
                &fixed,
                target(
                    "p-gas",
                    "ガス代",
                    "ledger",
                    "11/16",
                    "使用量の変化を確認する。",
                    2,
                ),
            ),
            (
                &fixed,
                target(
                    "p-rent",
                    "家賃",
                    "ledger",
                    "10/28",
                    "更新時期と契約条件。",
                    1,
                ),
            ),
            (
                &fixed,
                target(
                    "p-phone",
                    "通信費",
                    "ledger",
                    "11/05",
                    "プラン見直しの候補。",
                    2,
                ),
            ),
            (
                &fixed,
                target(
                    "p-sub",
                    "サブスク",
                    "ledger",
                    "11/14",
                    "利用状況と解約候補。",
                    3,
                ),
            ),
        ];
        for (via, target) in extra {
            two_hop.push(TwoHop {
                via: via.clone(),
                target,
                reason: "2-hop".to_owned(),
            });
        }
        two_hop
    };
    let related = vec![
        fixture_summary(BUDGET_ID).unwrap(),
        fixture_summary(MONTHLY_ID).unwrap(),
    ];
    Workspace {
        page,
        before: vec!["毎月の支出を hledger のジャーナルで管理している。".to_owned()],
        after: vec!["固定費の見直しは月1回、日曜日にまとめて行う。".to_owned()],
        snippet: Some(snippet),
        embeds: Vec::new(),
        outline: vec![
            OutlineItem {
                level: 1,
                title: "家計の記録".to_owned(),
                line: 1,
            },
            OutlineItem {
                level: 2,
                title: "買い物ジャーナル".to_owned(),
                line: 5,
            },
            OutlineItem {
                level: 2,
                title: "今月の残高".to_owned(),
                line: 10,
            },
            OutlineItem {
                level: 2,
                title: "固定費の見直し".to_owned(),
                line: 14,
            },
        ],
        links: Links {
            outgoing: vec![goals.clone(), receipts.clone(), fixed.clone()],
            backlinks: vec![goals, receipts, fixed],
            two_hop: two_hop_all,
        },
        related,
        recent: fixture_recent(),
        info: PageInfo {
            created: "2026-10-02".to_owned(),
            updated: "14:31".to_owned(),
            word_count: 642,
            tags: "家計, ledger, 毎月".to_owned(),
        },
    }
}

fn fixture_reading() -> Workspace {
    let page = fixture_page(
        READING_ID,
        "読書ノート",
        "note",
        &["note", "読書"],
        "# 読書ノート\n\n読んだ本の要点を残す。\n\n## 最近の本\n\n- [[p-goals|2026年の目標]]\n- [[p-rust|Rustのメモ]]\n",
    );
    publish_reading(page)
}

fn publish_reading(mut page: Content) -> Workspace {
    page.updated_label = "昨日".to_owned();
    let goals = fixture_summary("p-goals").unwrap();
    let rust = fixture_summary(RUST_ID).unwrap();
    Workspace {
        page: page.clone(),
        before: vec!["読んだ本の要点を残す。".to_owned()],
        after: Vec::new(),
        snippet: None,
        embeds: Vec::new(),
        outline: vec![
            OutlineItem {
                level: 1,
                title: "読書ノート".to_owned(),
                line: 1,
            },
            OutlineItem {
                level: 2,
                title: "最近の本".to_owned(),
                line: 5,
            },
        ],
        links: Links {
            outgoing: vec![goals.clone(), rust],
            backlinks: vec![goals],
            two_hop: Vec::new(),
        },
        related: Vec::new(),
        recent: fixture_recent(),
        info: PageInfo {
            created: "2026-10-12".to_owned(),
            updated: "昨日".to_owned(),
            word_count: 128,
            tags: "note, 読書".to_owned(),
        },
    }
}

fn fixture_rust() -> Workspace {
    let mut page = fixture_page(
        RUST_ID,
        "Rustのメモ",
        "code",
        &["code", "Rust"],
        "# Rustのメモ\n\n所有権と借用のメモ。\n\n```rust\nfn main() {\n    let s = String::from(\"hello\");\n    println!(\"{s}\");\n}\n```\n\n- [[p-reading|読書ノート]]\n",
    );
    page.updated_label = "11/20".to_owned();
    let reading = fixture_summary(READING_ID).unwrap();
    Workspace {
        page: page.clone(),
        before: vec!["所有権と借用のメモ。".to_owned()],
        after: Vec::new(),
        snippet: None,
        embeds: Vec::new(),
        outline: vec![OutlineItem {
            level: 1,
            title: "Rustのメモ".to_owned(),
            line: 1,
        }],
        links: Links {
            outgoing: vec![reading.clone()],
            backlinks: vec![reading],
            two_hop: Vec::new(),
        },
        related: Vec::new(),
        recent: fixture_recent(),
        info: PageInfo {
            created: "2026-10-15".to_owned(),
            updated: "11/20".to_owned(),
            word_count: 96,
            tags: "code, Rust".to_owned(),
        },
    }
}

fn generic_workspace(summary: &Summary) -> Workspace {
    let page = Content {
        id: summary.id.clone(),
        content_type: summary.content_type.clone(),
        title: summary.title.clone(),
        body: format!("# {}\n\n{}\n", summary.title, summary.excerpt),
        format: summary.format.clone(),
        language: summary.language.clone(),
        kinds: summary.kinds.clone(),
        tags: Vec::new(),
        aliases: Vec::new(),
        rev: summary.rev,
        created: "2026-10-20".to_owned(),
        updated_at: 0,
        updated_label: summary.updated_label.clone(),
    };
    Workspace {
        page,
        before: vec![summary.excerpt.clone()],
        after: Vec::new(),
        snippet: None,
        embeds: Vec::new(),
        outline: vec![OutlineItem {
            level: 1,
            title: summary.title.clone(),
            line: 1,
        }],
        links: Links::default(),
        related: Vec::new(),
        recent: fixture_recent(),
        info: PageInfo {
            created: "2026-10-20".to_owned(),
            updated: summary.updated_label.clone(),
            word_count: summary.excerpt.split_whitespace().count(),
            tags: summary.kind_label(),
        },
    }
}

fn fixture_recent() -> Vec<Summary> {
    let ids = ["p-home", "p-reading", "p-rust", "p-diary"];
    ids.iter().filter_map(|id| fixture_summary(id)).collect()
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() {
    let mut args = std::env::args().skip(1);
    let snapshot_path = if args.next().as_deref() == Some("--snapshot") {
        Some(args.next().expect("--snapshot requires a path"))
    } else {
        None
    };
    let snapshot = snapshot_path.is_some();

    let _ = fonts();
    set_theme(app_theme());
    let state = boot(snapshot);

    if let Some(out) = snapshot_path {
        anti_webui::app::headless::render_png(out, WINDOW_WIDTH, WINDOW_HEIGHT, 1.0, move || {
            layout(&state)
        })
        .unwrap();
        return;
    }

    App::new()
        .title("Harmion Wiki")
        .size(WINDOW_WIDTH as i32, WINDOW_HEIGHT as i32)
        .run(move || layout(&state))
        .unwrap();
}

#[cfg(test)]
mod gui_markdown_tests {
    use super::*;

    #[test]
    fn parses_headings_lists_tables_and_embeds() {
        let body = "# Title\n\n## Section\n\n- one\n- two\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\n!```rust|code||abc\n```\n";
        let blocks = parse_markdown(body, "Title");
        assert!(
            blocks
                .iter()
                .any(|block| matches!(block, MdBlock::Heading(2, title) if title == "Section"))
        );
        assert!(
            blocks.iter().any(
                |block| matches!(block, MdBlock::List(false, items) if items == &["one", "two"])
            )
        );
        assert!(
            blocks
                .iter()
                .any(|block| matches!(block, MdBlock::Table(rows) if rows.len() == 2))
        );
        assert!(
            blocks
                .iter()
                .any(|block| matches!(block, MdBlock::Embed(id) if id == "abc"))
        );
    }

    #[test]
    fn strips_wiki_links_without_trailing_brackets() {
        assert_eq!(strip_inline("see [[p-goals|目標]] now"), "see 目標 now");
        assert_eq!(strip_inline("[[p-home]]"), "p-home");
        assert_eq!(strip_inline("**bold** and `code`"), "bold and code");
    }
}
