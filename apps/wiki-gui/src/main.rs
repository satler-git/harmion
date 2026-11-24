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
mod palette;
mod shelf;
mod toolbar;
mod ui;

use palette::palette;
use shelf::shelf;
use toolbar::toolbar;
use ui::{
    chip, divider, dot, icon, icon_button, icon_text_button, kbd, mono_text, nav_row,
    section_label, ui_text, vdiv,
};

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::ops::Range;
use std::os::unix::net::UnixStream;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use anti_webui::prelude::*;
use anti_webui_markdown::{MarkdownTheme, markdown_view};
use anti_webui_symbols_outlined as icons;
use harmion_wiki_core::parse_interpreter_calls;
use serde::{Deserialize, Serialize};
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
const SHELF_HEIGHT: f32 = 404.0;
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
        // The design language is IBM Plex Sans for chrome and Noto Serif for the reading
        // surface. Both are installed with the Nix devShell; the bundled font wins when set.
        let mut sans = "IBM Plex Sans JP".to_owned();
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

/// The display face: page titles and section headings. Kept separate from the UI face so the
/// reading surface has an editorial voice without making every control serif.
fn display() -> &'static str {
    "Noto Serif CJK JP"
}

fn app_theme() -> Theme {
    Theme {
        // A warm-neutral graphite rather than pure black: long reading sessions are easier on
        // the eye and the article surface separates from the chrome without hard lines.
        page: rgb(0x1b1c1f),
        surface: rgb(0x202126),
        surface_muted: rgb(0x292a30),
        overlay: rgb(0x24252a),
        hover: rgb(0x2e3037),
        pressed: rgb(0x383a42),
        selected: rgb(0x3b7df0),
        text: rgb(0xf2f2f5),
        text_muted: rgb(0xa4a6ae),
        text_disabled: rgb(0x72747d),
        on_accent: rgb(0xffffff),
        border: rgb(0x2f3138),
        border_strong: rgb(0x3e4048),
        accent: rgb(0x4c8dff),
        accent_hover: rgb(0x6ba1ff),
        accent_pressed: rgb(0x3574dd),
        accent_text: rgb(0x7db0ff),
        accent_muted: rgb(0x1d3350),
        success: rgb(0x3ecf72),
        success_muted: rgb(0x17351f),
        success_text: rgb(0x3ecf72),
        ..Theme::dark()
    }
}

// ---------------------------------------------------------------------------
// Wire model (mirrors apps/wiki-server/src/model.rs)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize, Serialize)]
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

#[derive(Clone, Debug, Deserialize, Serialize)]
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

#[derive(Clone, Debug, Deserialize, Serialize)]
struct OutlineItem {
    #[serde(default)]
    level: u8,
    title: String,
    #[serde(default)]
    line: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct TwoHop {
    via: Summary,
    target: Summary,
    #[serde(default)]
    reason: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct EmbedView {
    #[serde(default)]
    before: Vec<String>,
    snippet: Content,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct Links {
    #[serde(default)]
    outgoing: Vec<Summary>,
    #[serde(default)]
    backlinks: Vec<Summary>,
    #[serde(default)]
    two_hop: Vec<TwoHop>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
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

#[derive(Clone, Debug, Deserialize, Serialize)]
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

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct CacheFile {
    summaries: Vec<Summary>,
    workspaces: BTreeMap<String, Workspace>,
    #[serde(default)]
    session: Option<SessionFile>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SessionFile {
    tabs: Vec<SessionTab>,
    active: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SessionTab {
    id: String,
    title: String,
    kind: String,
    scroll: f32,
    outline: usize,
}

#[derive(Clone, Debug)]
struct Tab {
    id: Option<String>,
    title: String,
    kind: String,
    workspace: Option<Workspace>,
    error: Option<String>,
    scroll: f32,
    outline: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Server,
    Offline,
    Fallback,
    Empty,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SidebarMode {
    All,
    Recent,
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
struct PollGuard(Arc<AtomicBool>);

impl Drop for PollGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

enum PollUpdate {
    Summaries(Vec<Summary>),
    Reconnected(Vec<Summary>),
    Disconnected,
    Ping,
}

#[derive(Clone)]
struct AppState {
    snapshot: bool,
    source: Signal<Source>,
    boot_error: Option<String>,
    socket: String,
    summaries: Signal<Vec<Summary>>,
    providers: Signal<Vec<String>>,
    cache: Rc<RefCell<CacheFile>>,
    poll_guard: Rc<PollGuard>,
    updates: Rc<RefCell<mpsc::Receiver<PollUpdate>>>,
    tag_filter: Signal<Option<String>>,
    sidebar_mode: Signal<SidebarMode>,
    sidebar_open: Signal<bool>,
    tabs: Signal<Vec<Tab>>,
    active: Signal<usize>,
    inspector: Signal<Inspector>,
    inspector_visible: Signal<bool>,
    shelf_pinned: Signal<bool>,
    shelf_hover: Signal<bool>,
    shelf_dismissed: Signal<bool>,
    palette: Signal<Option<PaletteMode>>,
    palette_selected: Signal<usize>,
    palette_needs_focus: Signal<bool>,
    query: Signal<String>,
    results: Signal<Vec<Summary>>,
    palette_error: Signal<Option<String>>,
    next_selected: Signal<usize>,
    editor: Signal<EditorState>,
    editor_watch: Signal<bool>,
    history: Signal<Vec<String>>,
    history_index: Signal<usize>,
    outline_active: Signal<usize>,
    /// Width of the window in logical pixels, updated from the root's resize listener. Used to
    /// drop the outline rail before it would overlap the article.
    viewport: Signal<f32>,
    delete_armed: Signal<bool>,
    toast: Signal<Option<String>>,
    shortcuts: Signal<bool>,
    outline_open: Signal<bool>,
    outline_collapsed: Signal<bool>,
    outline_keyboard: Signal<bool>,
    tab_focus: Signal<bool>,
    article_ref: NodeRef,
    heading_refs: Rc<RefCell<Vec<NodeRef>>>,
    root: NodeRef,
    manager: Rc<RefCell<neovide::Manager>>,
}

impl AppState {
    fn active_tab(&self) -> Option<Tab> {
        let tabs = self.tabs.get();
        tabs.get(self.active.get().min(tabs.len().saturating_sub(1)))
            .cloned()
    }

    fn active_index(&self) -> usize {
        let tabs = self.tabs.get();
        self.active.get().min(tabs.len().saturating_sub(1))
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
            (Some(id), Source::Offline) => self
                .cache
                .borrow()
                .workspaces
                .get(id)
                .cloned()
                .ok_or_else(|| "オフラインキャッシュにこのページがありません。".to_owned()),
            (Some(id), Source::Fallback) => fixture_workspace(id, &tab.title)
                .ok_or_else(|| "フォールバックにこのページのデータがありません。".to_owned()),
            (Some(_), Source::Empty) => Err("ページがありません。".to_owned()),
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
                        if matches!(self.source.get_untracked(), Source::Server) {
                            self.cache
                                .borrow_mut()
                                .workspaces
                                .insert(workspace.page.id.clone(), workspace.clone());
                            let _ = write_cache(&self.cache.borrow());
                        }
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

    fn save_active_reading_state(&self) {
        let index = self.active_index();
        let scroll = self.article_ref.scroll_offset().y;
        let outline = self.outline_active.get_untracked();
        self.tabs.update(|tabs| {
            if let Some(tab) = tabs.get_mut(index) {
                tab.scroll = scroll;
                tab.outline = outline;
            }
        });
    }

    fn apply_reading_state(&self, scroll: f32, outline: usize) {
        self.outline_active.set(outline);
        let article = self.article_ref;
        let y = scroll.max(0.0).round() as i32;
        request_frame(move |_| article.scroll_to_after_layout(0, y));
    }

    fn save_session(&self) {
        if self.snapshot {
            return;
        }
        let tabs = self
            .tabs
            .get_untracked()
            .iter()
            .filter_map(|tab| {
                tab.id.as_ref().map(|id| SessionTab {
                    id: id.clone(),
                    title: tab.title.clone(),
                    kind: tab.kind.clone(),
                    scroll: tab.scroll,
                    outline: tab.outline,
                })
            })
            .collect::<Vec<_>>();
        if tabs.is_empty() {
            return;
        }
        let session = SessionFile {
            tabs,
            active: self.active_index(),
        };
        let mut cache = self.cache.borrow_mut();
        cache.session = Some(session);
        let _ = write_cache(&cache);
    }

    fn restore_active_reading_state(&self) {
        if let Some(tab) = self.active_tab() {
            self.apply_reading_state(tab.scroll, tab.outline);
        }
    }

    fn switch_tab(&self, index: usize) {
        let length = self.tabs.get_untracked().len();
        if index >= length || index == self.active_index() {
            return;
        }
        self.save_active_reading_state();
        self.tab_focus.set(false);
        let Some(target) = self.tabs.get_untracked().get(index).cloned() else {
            return;
        };
        self.active.set(index);
        self.outline_active.set(target.outline);
        self.outline_open.set(false);
        self.delete_armed.set(false);
        self.ensure_workspace(index);
        self.apply_reading_state(target.scroll, target.outline);
        self.save_session();
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
        let Some(workspace) = self.active_workspace() else {
            return;
        };
        let id = workspace.page.id.clone();
        let rev = workspace.page.rev;
        match rpc(
            "update_meta",
            json!({ "id": id, "rev": rev, "title": title.trim() }),
        ) {
            Ok(value) => match serde_json::from_value::<Content>(value) {
                Ok(content) => {
                    self.tabs.update(|tabs| {
                        if let Some(tab) = tabs
                            .iter_mut()
                            .find(|tab| tab.id.as_deref() == Some(id.as_str()))
                        {
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
            Err(error) => self
                .palette_error
                .set(Some(format!("変更できません: {error}"))),
        }
    }

    fn set_tags(&self, tags: &str) {
        let Some(workspace) = self.active_workspace() else {
            return;
        };
        let id = workspace.page.id.clone();
        let rev = workspace.page.rev;
        let values = tags
            .split([',', '、'])
            .map(str::trim)
            .filter(|tag| !tag.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        match rpc(
            "update_meta",
            json!({ "id": id, "rev": rev, "tags": values }),
        ) {
            Ok(value) => match serde_json::from_value::<Content>(value) {
                Ok(content) => {
                    self.tabs.update(|tabs| {
                        if let Some(tab) = tabs
                            .iter_mut()
                            .find(|tab| tab.id.as_deref() == Some(id.as_str()))
                            && let Some(workspace) = tab.workspace.as_mut()
                        {
                            workspace.page = content.clone();
                            workspace.info.tags = content.tags.join(", ");
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
            Err(error) => self
                .palette_error
                .set(Some(format!("変更できません: {error}"))),
        }
    }

    fn open_id(&self, id: &str, title: &str, kind: &str) {
        self.outline_open.set(false);
        if let Some(index) = self
            .tabs
            .get_untracked()
            .iter()
            .position(|tab| tab.id.as_deref() == Some(id))
        {
            self.switch_tab(index);
        } else {
            self.save_active_reading_state();
            let tab = Tab {
                id: Some(id.to_owned()),
                title: title.to_owned(),
                kind: kind.to_owned(),
                workspace: None,
                error: None,
                scroll: 0.0,
                outline: 0,
            };
            let index = self.tabs.get_untracked().len();
            self.tabs.update(|tabs| tabs.push(tab));
            self.active.set(index);
            self.ensure_workspace(index);
            self.apply_reading_state(0.0, 0);
        }
        self.delete_armed.set(false);
        self.push_history(Some(id.to_owned()));
        self.palette.set(None);
        self.palette_error.set(None);
        self.save_session();
    }

    fn close_tab(&self, index: usize) {
        let mut tabs = self.tabs.get_untracked();
        if tabs.len() <= 1 || index >= tabs.len() {
            return;
        }
        self.save_active_reading_state();
        tabs.remove(index);
        let new_active = self.active.get_untracked().min(tabs.len() - 1);
        let target = tabs.get(new_active).cloned();
        self.tabs.set(tabs);
        self.active.set(new_active);
        self.delete_armed.set(false);
        self.ensure_workspace(new_active);
        if let Some(target) = target {
            self.apply_reading_state(target.scroll, target.outline);
        }
        self.save_session();
    }

    fn close_other_tabs(&self, keep: usize) {
        let tabs = self.tabs.get_untracked();
        let Some(tab) = tabs.get(keep).cloned() else {
            return;
        };
        self.save_active_reading_state();
        self.tabs.set(vec![tab.clone()]);
        self.active.set(0);
        self.ensure_workspace(0);
        self.apply_reading_state(tab.scroll, tab.outline);
        self.save_session();
    }

    fn close_tabs_to_right(&self, index: usize) {
        let tabs = self.tabs.get_untracked();
        if index + 1 >= tabs.len() {
            return;
        }
        self.save_active_reading_state();
        self.tabs.update(|tabs| tabs.truncate(index + 1));
        let active = self.active.get_untracked().min(index);
        self.active.set(active);
        let target = self.tabs.get_untracked().get(active).cloned();
        self.ensure_workspace(active);
        if let Some(target) = target {
            self.apply_reading_state(target.scroll, target.outline);
        }
        self.save_session();
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
            self.switch_tab(tab_index);
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
        self.palette_selected.set(0);
        self.palette_needs_focus.set(true);
        let prefill = match mode {
            PaletteMode::Rename => self.active_tab().map(|tab| tab.title).unwrap_or_default(),
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
        self.palette_selected.set(0);
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
                self.start_editor_watch();
            }
            Err(error) => {
                eprintln!("wiki-gui: neovide: {error}");
                self.set_editor_error(error);
            }
        }
    }

    fn start_editor_watch(&self) {
        if self.editor_watch.get_untracked() {
            return;
        }
        self.editor_watch.set(true);
        let state = self.clone();
        set_timeout(Duration::from_secs(2), move || editor_watch_tick(&state));
    }

    fn sync_editor_page(&self) {
        if !matches!(self.source.get_untracked(), Source::Server) {
            return;
        }
        let Some(tab) = self.active_tab() else { return };
        let Some(id) = tab.id else { return };
        let Some(current) = tab.workspace else { return };
        let Ok(value) = rpc("workspace", json!({ "id": id })) else {
            return;
        };
        let Ok(next) = serde_json::from_value::<Workspace>(value) else {
            return;
        };
        if next.page.rev == current.page.rev {
            return;
        }
        let scroll = self.article_ref.scroll_offset().y;
        let outline = self
            .outline_active
            .get_untracked()
            .min(next.outline.len().saturating_sub(1));
        self.tabs.update(|tabs| {
            if let Some(tab) = tabs
                .iter_mut()
                .find(|tab| tab.id.as_deref() == Some(id.as_str()))
            {
                tab.title = next.page.title.clone();
                tab.kind = next.page.content_type.clone();
                tab.workspace = Some(next.clone());
                tab.error = None;
            }
        });
        self.cache
            .borrow_mut()
            .workspaces
            .insert(id.clone(), next.clone());
        let _ = write_cache(&self.cache.borrow());
        self.summaries.update(|items| {
            if let Some(summary) = items.iter_mut().find(|summary| summary.id == id) {
                summary.title = next.page.title.clone();
                summary.rev = next.page.rev;
            }
        });
        self.apply_reading_state(scroll, outline);
        notify(self, "エディタの変更を取り込みました");
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

    fn delete_content(&self, id: &str, rev: u64) {
        if !matches!(self.source.get_untracked(), Source::Server) {
            self.delete_armed.set(false);
            self.set_editor_error("削除: wiki-server に接続していません".to_owned());
            return;
        }
        if let Err(error) = rpc("delete", json!({ "id": id, "rev": rev })) {
            self.delete_armed.set(false);
            self.set_editor_error(format!("削除できません: {error}"));
            return;
        }
        self.summaries
            .update(|items| items.retain(|item| item.id != id));
        self.tabs
            .update(|tabs| tabs.retain(|tab| tab.id.as_deref() != Some(id)));
        self.delete_armed.set(false);
        notify(self, "ページを削除しました");
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
            let target = self.tabs.get_untracked().get(index).cloned();
            self.ensure_workspace(index);
            if let Some(target) = target {
                self.apply_reading_state(target.scroll, target.outline);
            }
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
        let scroll = self.article_ref.scroll_offset().y;
        let outline = self.outline_active.get_untracked();
        self.tabs.update(|tabs| {
            if let Some(tab) = tabs.get_mut(index) {
                tab.workspace = None;
                tab.error = None;
            }
        });
        self.ensure_workspace(index);
        self.apply_reading_state(scroll, outline);
    }

    fn scroll_to_outline(&self, index: usize, line: usize) {
        if let Some(anchor) = self.heading_refs.borrow().get(index).copied() {
            let article = self.article_ref;
            request_frame(move |_| {
                let (Some(top), Some(heading)) = (
                    article.logical_rect().map(|rect| rect.y),
                    anchor.logical_rect().map(|rect| rect.y),
                ) else {
                    return;
                };
                // Put the heading just below the top edge instead of letting
                // `scroll_into_view` align its bottom with the viewport bottom.
                let y = (article.scroll_offset().y + heading - top - 24.0).max(0.0);
                article.scroll_to(0.0, y);
            });
            if let Some(id) = self.active_tab().and_then(|tab| tab.id)
                && let Err(error) = self.manager.borrow().goto_line(&id, line)
            {
                self.editor.update(|editor| editor.warning = Some(error));
            }
            return;
        }
        self.scroll_to_line(line);
    }

    fn scroll_to_line(&self, line: usize) {
        let y = (line.saturating_sub(1) as f32 * 27.0) as i32;
        let article = self.article_ref;
        request_frame(move |_| article.scroll_to_after_layout(0, y));
    }

    fn request_root_focus(&self) {
        let root = self.root;
        request_frame(move |_| root.focus());
    }
}

/// Keeps the outline rail in step with what is actually at the top of the reading pane.
fn sync_active_heading(state: &AppState) {
    let Some(top) = state.article_ref.logical_rect().map(|rect| rect.y) else {
        return;
    };
    let refs = state.heading_refs.borrow();
    let mut active = 0usize;
    for (index, heading) in refs.iter().enumerate() {
        let Some(rect) = heading.logical_rect() else {
            continue;
        };
        if rect.y <= top + 140.0 {
            active = index;
        } else {
            break;
        }
    }
    if state.outline_active.get_untracked() != active {
        state.outline_active.set(active);
    }
}

fn highlighted_text(text: &str, query: &str, size: f32, base: Color, accent: Color) -> RichText {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return rich_text(vec![span(text).text_color(base).text_size(size)]);
    }
    let lower = text.to_lowercase();
    let mut runs = Vec::new();
    let mut cursor = 0;
    for (start, _) in lower.match_indices(&needle) {
        if start > cursor {
            runs.push(span(&text[cursor..start]).text_color(base).text_size(size));
        }
        let end = (start + needle.len()).min(text.len());
        runs.push(
            span(&text[start..end])
                .text_color(accent)
                .text_size(size)
                .font_weight(FontWeight::SEMIBOLD),
        );
        cursor = end;
    }
    if cursor < text.len() {
        runs.push(span(&text[cursor..]).text_color(base).text_size(size));
    }
    rich_text(runs)
}

fn source_label(source: Source) -> &'static str {
    match source {
        Source::Server => "サーバー",
        Source::Offline => "オフラインキャッシュ",
        Source::Fallback => "内蔵 fixture",
        Source::Empty => "未接続",
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

fn cache_path() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .filter(|path| !path.to_string_lossy().contains("/target/nix-"))
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".cache"))
        })
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    base.join("harmion-wiki").join("cache.json")
}

fn load_cache() -> Option<CacheFile> {
    let raw = fs::read_to_string(cache_path()).ok()?;
    serde_json::from_str(&raw).ok()
}

fn write_cache(cache: &CacheFile) -> std::io::Result<()> {
    let path = cache_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let raw = serde_json::to_vec_pretty(cache).map_err(std::io::Error::other)?;
    fs::write(&path, raw)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn load_summaries() -> Result<Vec<Summary>, String> {
    let value = rpc("list", json!({}))?;
    let items = value
        .get("items")
        .cloned()
        .ok_or_else(|| "list result has no items".to_owned())?;
    serde_json::from_value(items).map_err(|error| format!("list の解釈に失敗: {error}"))
}

fn start_polling(alive: Arc<AtomicBool>, updates: mpsc::Sender<PollUpdate>) {
    let mut since = 0u64;
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_millis(250));
            if !alive.load(Ordering::SeqCst) {
                break;
            }
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let watch = rpc("watch", json!({ "since": since, "timeout_ms": 20_000 }));
                match watch {
                    Ok(value) => {
                        if let Some(generation) = value.get("generation").and_then(Value::as_u64) {
                            since = generation;
                        }
                        if let Ok(items) = serde_json::from_value::<Vec<Summary>>(
                            value.get("items").cloned().unwrap_or(Value::Null),
                        ) && updates.send(PollUpdate::Summaries(items)).is_ok()
                        {
                            request_frame(|_| {});
                        }
                    }
                    Err(_) => {
                        if let Ok(items) = load_summaries() {
                            since = 0;
                            if updates.send(PollUpdate::Reconnected(items)).is_ok() {
                                request_frame(|_| {});
                            }
                        } else if updates.send(PollUpdate::Disconnected).is_ok() {
                            request_frame(|_| {});
                        }
                    }
                }
            }));
            if outcome.is_err() || updates.send(PollUpdate::Ping).is_err() {
                break;
            }
        }
    });
}

fn editor_watch_tick(state: &AppState) {
    if !state.editor_watch.get_untracked() {
        return;
    }
    let attached = state.manager.borrow().has_server() && state.editor.get_untracked().attached;
    if !attached {
        state.editor_watch.set(false);
        let mut editor = state.editor.get_untracked();
        editor.attached = false;
        editor.detail = state.manager.borrow().status_label();
        state.editor.set(editor);
        return;
    }
    state.sync_editor_page();
    let state = state.clone();
    set_timeout(Duration::from_secs(2), move || editor_watch_tick(&state));
}

fn drain_updates(state: &AppState) {
    while let Ok(update) = state.updates.borrow().try_recv() {
        match update {
            PollUpdate::Summaries(items) => state.summaries.set(items),
            PollUpdate::Reconnected(items) => {
                state.source.set(Source::Server);
                state.summaries.set(items);
            }
            PollUpdate::Disconnected => {
                if matches!(state.source.get_untracked(), Source::Server) {
                    state.source.set(Source::Offline);
                }
            }
            PollUpdate::Ping => {}
        }
    }
}

fn default_tabs(summaries: &[Summary]) -> Vec<Tab> {
    let mut chosen: Vec<&Summary> = Vec::new();
    for title in ["家計の記録", "読書ノート", "Rustのメモ"] {
        if let Some(summary) = summaries.iter().find(|summary| summary.title == title) {
            chosen.push(summary);
        }
    }
    for summary in summaries.iter().filter(|summary| summary.is_page()) {
        if chosen.len() >= 3 {
            break;
        }
        if !chosen.iter().any(|kept| kept.id == summary.id) {
            chosen.push(summary);
        }
    }
    chosen
        .into_iter()
        .map(|summary| Tab {
            id: Some(summary.id.clone()),
            title: summary.title.clone(),
            kind: summary.content_type.clone(),
            workspace: None,
            error: None,
            scroll: 0.0,
            outline: 0,
        })
        .collect()
}

fn boot(snapshot: bool, shelf: bool) -> AppState {
    let socket = socket_path();
    let mut cached = load_cache().unwrap_or_default();
    let (summaries, source, boot_error) = match load_summaries() {
        Ok(summaries) => {
            eprintln!("wiki-gui: loaded list from {socket}");
            cached.summaries = summaries.clone();
            let _ = write_cache(&cached);
            (summaries, Source::Server, None)
        }
        Err(error) => {
            if !cached.summaries.is_empty() {
                eprintln!("wiki-gui: server unavailable ({error}); using offline cache");
                (cached.summaries.clone(), Source::Offline, Some(error))
            } else if snapshot {
                eprintln!("wiki-gui: server unavailable ({error}); using built-in fixture");
                (fixture_summaries(), Source::Fallback, Some(error))
            } else {
                eprintln!("wiki-gui: server unavailable ({error}); starting empty");
                (Vec::new(), Source::Empty, Some(error))
            }
        }
    };

    let restored = (!snapshot)
        .then_some(cached.session.as_ref())
        .flatten()
        .map(|session| {
            let tabs = session
                .tabs
                .iter()
                .filter_map(|saved| {
                    summaries
                        .iter()
                        .find(|summary| summary.id == saved.id)
                        .map(|summary| Tab {
                            id: Some(summary.id.clone()),
                            title: summary.title.clone(),
                            kind: summary.content_type.clone(),
                            workspace: None,
                            error: None,
                            scroll: saved.scroll.max(0.0),
                            outline: saved.outline,
                        })
                })
                .collect::<Vec<_>>();
            let active = session.active.min(tabs.len().saturating_sub(1));
            (tabs, active)
        })
        .filter(|(tabs, _)| !tabs.is_empty());
    let (tabs, active_index) = restored.unwrap_or_else(|| (default_tabs(&summaries), 0));
    let initial_outline = tabs.get(active_index).map(|tab| tab.outline).unwrap_or(0);

    let manager = Rc::new(RefCell::new(neovide::Manager::detect()));
    let poll_alive = Arc::new(AtomicBool::new(true));
    let poll_guard = Rc::new(PollGuard(poll_alive.clone()));
    let (poll_tx, poll_rx) = mpsc::channel();
    let attached = manager.borrow().has_server() && manager.borrow().has_window();
    let detail = manager.borrow().status_label();
    let root = node_ref();
    let state = AppState {
        snapshot,
        source: signal(source),
        boot_error,
        socket,
        summaries: signal(summaries),
        providers: signal(load_providers()),
        cache: Rc::new(RefCell::new(cached)),
        poll_guard,
        updates: Rc::new(RefCell::new(poll_rx)),
        tag_filter: signal(None),
        sidebar_mode: signal(SidebarMode::All),
        sidebar_open: signal(true),
        tabs: signal(tabs),
        active: signal(active_index),
        inspector: signal(Inspector::Links),
        // The inspector is a contextual drawer, not a permanent column: the reading surface
        // gets the whole width until the user asks for links or page info.
        inspector_visible: signal(false),
        shelf_pinned: signal(shelf),
        shelf_hover: signal(false),
        shelf_dismissed: signal(false),
        palette: signal(None),
        palette_selected: signal(0),
        palette_needs_focus: signal(false),
        query: signal(String::new()),
        results: signal(Vec::new()),
        palette_error: signal(None),
        next_selected: signal(0),
        editor_watch: signal(false),
        editor: signal(EditorState {
            attached,
            follow: attached,
            detail,
            warning: None,
            error: None,
        }),
        history: signal(Vec::new()),
        history_index: signal(0),
        outline_active: signal(initial_outline),
        viewport: signal(WINDOW_WIDTH as f32),
        delete_armed: signal(false),
        toast: signal(None),
        shortcuts: signal(false),
        outline_open: signal(false),
        outline_collapsed: signal(false),
        outline_keyboard: signal(false),
        tab_focus: signal(false),
        article_ref: node_ref(),
        heading_refs: Rc::new(RefCell::new(Vec::new())),
        root,
        manager,
    };
    for index in 0..state.tabs.get_untracked().len() {
        state.ensure_workspace(index);
    }
    if let Some(tab) = state.active_tab() {
        if tab.scroll > 0.0 {
            state.apply_reading_state(tab.scroll, tab.outline);
        }
        state.push_history(tab.id);
    }
    if !snapshot {
        start_polling(poll_alive, poll_tx);
    }
    state
}

// ---------------------------------------------------------------------------
// Small view helpers
// ---------------------------------------------------------------------------

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

thread_local! {
    /// One tree-sitter highlighter for every fenced block. It compiles grammars lazily on a
    /// worker and bumps `ready`, which the block's `dyn_view` reads to colour itself again.
    static CODE_HIGHLIGHTER: CodeHighlighter = CodeHighlighter::new(CodeTheme::dark());
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

/// One line of a block already parsed by tree-sitter. `offset` is the line's byte offset in the
/// whole block, so styles crossing line boundaries are clipped correctly.
fn code_line_colored(
    line: &str,
    language: &str,
    offset: usize,
    colors: &[(Range<usize>, Rc<Style>)],
) -> RichText {
    if colors.is_empty() {
        return code_line(line, language);
    }
    let line_end = offset + line.len();
    let mut runs: Vec<Run> = Vec::new();
    let mut cursor = offset;
    for (range, style) in colors {
        let start = range.start.max(offset);
        let end = range.end.min(line_end);
        if start >= end {
            continue;
        }
        if start > cursor
            && let Some(text) = line.get(cursor - offset..start - offset)
        {
            runs.push(span(text).text_color(theme().text).into());
        }
        if let Some(text) = line.get(start - offset..end - offset) {
            runs.push(span(text).shared_style(style.clone()).into());
        }
        cursor = end;
    }
    if cursor < line_end
        && let Some(text) = line.get(cursor - offset..)
    {
        runs.push(span(text).text_color(theme().text).into());
    }
    if runs.is_empty() {
        runs.push(Run::Text(line.to_owned()));
    }
    rich_text(runs)
        .font_family(mono())
        .text_size(12.0_f32)
        .line_height(LineHeight::Relative(1.75_f32))
}

fn code_block(lines: &[&str], language: &str) -> View {
    let highlighter = CODE_HIGHLIGHTER.with(Clone::clone);
    let ready = highlighter.ready();
    let code = lines.join("\n");
    let language = language.to_owned();
    let lines = lines
        .iter()
        .map(|line| (*line).to_owned())
        .collect::<Vec<_>>();
    dyn_view(move || {
        // Grammar compilation happens off the UI thread; reading this signal repaints the
        // block once the highlighter can colour it.
        let _ = ready.get();
        let colors = highlighter.highlight(&language, &code);
        let mut offset = 0usize;
        let mut block = div().flex().flex_col();
        for line in &lines {
            if line.trim().is_empty() {
                block = block.child(div().h(7.0_f32));
            } else {
                block = block.child(code_line_colored(line, &language, offset, &colors));
            }
            offset += line.len() + 1;
        }
        block
    })
}

// ---------------------------------------------------------------------------
// Shell
// ---------------------------------------------------------------------------

fn layout(state: &AppState) -> Div {
    drain_updates(state);
    let root = state.root;
    let key_state = state.clone();
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
        .on_resized({
            let state = state.clone();
            move |width, _| state.viewport.set(width)
        })
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
                .child(
                    div()
                        .h_full()
                        .overflow_hidden()
                        .transition(Transition::ms(180))
                        .dyn_style({
                            let state = state.clone();
                            move |style| style.w(if state.sidebar_open.get() { 264.0 } else { 0.0 })
                        })
                        .child(dyn_view({
                            let state = state.clone();
                            move || sidebar(&state)
                        })),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(0.0_f32)
                        .h_full()
                        .flex()
                        .flex_row()
                        .justify_center()
                        .child(
                            div()
                                .flex_1()
                                .min_w(0.0_f32)
                                .max_w(976.0_f32)
                                .h_full()
                                .flex()
                                .flex_row()
                                .child(dyn_view({
                                    let state = state.clone();
                                    move || editor_column(&state)
                                }))
                                .child(dyn_view({
                                    let state = state.clone();
                                    move || outline_column(&state)
                                })),
                        ),
                )
                .child(dyn_view({
                    let state = state.clone();
                    move || inspector_column(&state)
                })),
        )
        .child(outline_panel(state))
        .child(shelf(state))
        .child(dyn_view({
            let state = state.clone();
            move || status_bar(&state)
        }))
        .child(palette(state))
        .child(shortcuts_overlay(state))
        .child(toast(state))
}

fn scroll_article(state: &AppState, delta: f32) {
    let article = state.article_ref;
    let current = article.scroll_offset().y;
    article.scroll_to(0.0, (current + delta).max(0.0));
}

fn handle_key(state: &AppState, event: &KeyboardEvent) {
    let ctrl = event.modifiers.ctrl;
    let alt = event.modifiers.alt;
    if event.key == Key::Named(NamedKey::Escape) {
        if state.shortcuts.get_untracked() {
            event.prevent_default();
            state.shortcuts.set(false);
            return;
        }
        if state.outline_open.get_untracked() {
            event.prevent_default();
            state.outline_open.set(false);
            return;
        }
        if state.outline_keyboard.get_untracked() {
            event.prevent_default();
            state.outline_keyboard.set(false);
            return;
        }
        if state.palette.get_untracked().is_some() {
            event.prevent_default();
            state.close_palette();
            return;
        }
        if state.delete_armed.get_untracked() {
            event.prevent_default();
            state.delete_armed.set(false);
            return;
        }
        if state.inspector_visible.get_untracked() {
            event.prevent_default();
            state.inspector_visible.set(false);
            return;
        }
        if state.shelf_pinned.get_untracked() {
            event.prevent_default();
            state.shelf_pinned.set(false);
            return;
        }
        if state.tab_focus.get_untracked() {
            event.prevent_default();
            state.tab_focus.set(false);
            return;
        }
    }
    if state.shortcuts.get_untracked() {
        event.prevent_default();
        return;
    }
    if state.palette.get_untracked().is_some() {
        let length = state.results.get_untracked().len();
        if length > 0 && event.key == Key::Named(NamedKey::ArrowDown) {
            event.prevent_default();
            let index = (state.palette_selected.get_untracked() + 1).min(length - 1);
            state.palette_selected.set(index);
            return;
        }
        if length > 0 && event.key == Key::Named(NamedKey::ArrowUp) {
            event.prevent_default();
            let index = state.palette_selected.get_untracked().saturating_sub(1);
            state.palette_selected.set(index);
            return;
        }
        if event.key == Key::Named(NamedKey::Enter) {
            event.prevent_default();
            let index = state
                .palette_selected
                .get_untracked()
                .min(length.saturating_sub(1));
            if let Some(summary) = state.results.get_untracked().get(index) {
                state.open_summary(summary);
            }
            return;
        }
        // While the palette is open it owns the keyboard. Otherwise Ctrl+W et al. would
        // close a tab behind the search field the user is typing into.
        return;
    }
    if state.shortcuts.get_untracked() {
        if event.key.is_char('?') {
            event.prevent_default();
            state.shortcuts.set(false);
        }
        return;
    }
    if !ctrl && event.key.is_char('?') {
        event.prevent_default();
        state.shortcuts.set(true);
        return;
    }
    if ctrl && alt && (event.key.is_char('l') || event.key.is_char('L')) {
        event.prevent_default();
        if state.shelf_pinned.get_untracked() {
            state.shelf_pinned.set(false);
            state.shelf_dismissed.set(true);
        } else {
            state.shelf_dismissed.set(false);
            state.shelf_pinned.set(true);
        }
        return;
    }
    if ctrl && event.key == Key::Named(NamedKey::Tab) {
        event.prevent_default();
        let length = state.tabs.get_untracked().len();
        if length > 1 {
            let current = state.active_index();
            let next = if event.modifiers.shift {
                (current + length - 1) % length
            } else {
                (current + 1) % length
            };
            state.switch_tab(next);
            state.tab_focus.set(true);
        }
        return;
    }
    if !ctrl {
        if state.outline_open.get_untracked() || state.outline_keyboard.get_untracked() {
            let items = outline_key_items(state);
            if !items.is_empty() {
                match event.key {
                    Key::Named(NamedKey::ArrowDown) => {
                        event.prevent_default();
                        let next = (state.outline_active.get_untracked() + 1).min(items.len() - 1);
                        state.outline_active.set(next);
                        return;
                    }
                    Key::Named(NamedKey::ArrowUp) => {
                        event.prevent_default();
                        let next = state.outline_active.get_untracked().saturating_sub(1);
                        state.outline_active.set(next);
                        return;
                    }
                    Key::Named(NamedKey::Enter) => {
                        event.prevent_default();
                        let index = state.outline_active.get_untracked().min(items.len() - 1);
                        if let Some(item) = items.get(index) {
                            state.outline_active.set(index);
                            state.scroll_to_outline(index, item.line);
                            state.outline_open.set(false);
                            state.outline_keyboard.set(false);
                        }
                        return;
                    }
                    _ => {}
                }
            } else {
                state.outline_keyboard.set(false);
            }
        }
        // Arrow keys only navigate the 2-hop shelf while it is actually visible; otherwise
        // they scroll the article, so the page is readable without a pointer.
        let shelf_open =
            state.shelf_pinned.get_untracked() && !state.shelf_dismissed.get_untracked();
        if !shelf_open {
            let page = state
                .article_ref
                .logical_rect()
                .map(|rect| rect.height)
                .unwrap_or(0.0_f32);
            let step = (page * 0.82).max(120.0_f32);
            let delta = match event.key {
                Key::Named(NamedKey::Space) | Key::Named(NamedKey::PageDown) => Some(step),
                Key::Named(NamedKey::PageUp) => Some(-step),
                Key::Named(NamedKey::ArrowDown) => Some(56.0_f32),
                Key::Named(NamedKey::ArrowUp) => Some(-56.0_f32),
                Key::Named(NamedKey::Home) => Some(-f32::MAX),
                _ => None,
            };
            if let Some(delta) = delta {
                event.prevent_default();
                scroll_article(state, delta);
            }
            return;
        }
        let hops = state
            .active_workspace()
            .map(|workspace| workspace.links.two_hop)
            .unwrap_or_default();
        if !hops.is_empty() && event.key == Key::Named(NamedKey::ArrowDown) {
            event.prevent_default();
            let index = (state.next_selected.get_untracked() + 1).min(hops.len() - 1);
            state.next_selected.set(index);
            return;
        }
        if !hops.is_empty() && event.key == Key::Named(NamedKey::ArrowUp) {
            event.prevent_default();
            let index = state.next_selected.get_untracked().saturating_sub(1);
            state.next_selected.set(index);
            return;
        }
        if event.key == Key::Named(NamedKey::Enter) {
            let index = state
                .next_selected
                .get_untracked()
                .min(hops.len().saturating_sub(1));
            if let Some(hop) = hops.get(index) {
                event.prevent_default();
                state.open_summary(&hop.target);
            }
            return;
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
        state.outline_open.set(false);
        state
            .inspector_visible
            .update(|visible| *visible = !*visible);
    } else if event.key.is_char('1') {
        event.prevent_default();
        state.outline_open.set(false);
        state.inspector.set(Inspector::Links);
        state.inspector_visible.set(true);
    } else if event.key.is_char('2') {
        event.prevent_default();
        state.outline_open.set(false);
        state.inspector.set(Inspector::Info);
        state.inspector_visible.set(true);
    } else if event.key.is_char('3') {
        event.prevent_default();
        state.inspector_visible.set(false);
        toggle_outline(state);
    } else if event.key.is_char('\\') {
        event.prevent_default();
        state.sidebar_open.update(|open| *open = !*open);
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
    let mode = state.sidebar_mode.get();
    let matches_tag = |summary: &Summary| {
        tag_filter.as_ref().is_none_or(|tag| {
            summary.tags.iter().any(|candidate| candidate == tag)
                || summary.kinds.iter().any(|candidate| candidate == tag)
        })
    };
    let mut pages = summaries
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
    if mode == SidebarMode::Recent {
        let ids = recent
            .iter()
            .map(|summary| summary.id.as_str())
            .collect::<BTreeSet<_>>();
        pages.retain(|summary| ids.contains(summary.id.as_str()));
    }

    let mut list = div().flex().flex_col().px(10.0_f32).pb(8.0_f32);
    list = list
        .child(side_section("ライブラリ"))
        .child(nav_row(
            icons::DESCRIPTION,
            "すべてのページ",
            Some(summaries.iter().filter(|summary| summary.is_page()).count()),
            mode == SidebarMode::All,
            {
                let state = state.clone();
                move || {
                    state.sidebar_mode.set(SidebarMode::All);
                    state.tag_filter.set(None);
                }
            },
        ))
        .child(nav_row(
            icons::HISTORY,
            "最近",
            Some(recent.len()),
            mode == SidebarMode::Recent,
            {
                let state = state.clone();
                move || {
                    state.sidebar_mode.set(SidebarMode::Recent);
                    state.tag_filter.set(None);
                }
            },
        ))
        .child(side_section("タグ"));

    if tags.is_empty() {
        list = list.child(
            ui_text(
                "タグ付きページはまだありません。",
                10.5,
                theme().text_disabled,
            )
            .py(3.0_f32),
        );
    } else {
        let mut tag_row = div().pb(3.0_f32).flex().flex_row().flex_wrap().gap(4.0_f32);
        for tag in tags.iter().take(6) {
            let active = tag_filter.as_deref() == Some(tag.as_str());
            tag_row = tag_row.child(
                div()
                    .px(7.0_f32)
                    .py(3.0_f32)
                    .rounded(5.0_f32)
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
                    .cursor(CursorIcon::Pointer)
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
                    .child(ui_text(
                        format!("#{tag}"),
                        10.0,
                        if active {
                            theme().accent_text
                        } else {
                            theme().text_muted
                        },
                    )),
            );
        }
        list = list.child(tag_row);
    }

    list = list.child(side_section(if mode == SidebarMode::Recent {
        "最近のページ"
    } else {
        "ページ"
    }));
    if pages.is_empty() {
        list = list.child(ui_text(
            "該当するページはありません。",
            10.5,
            theme().text_disabled,
        ));
    }
    for summary in &pages {
        let is_active = summary.id == active_id;
        list = list.child(page_row(state, summary, is_active));
    }

    if !snippets.is_empty() {
        list = list.child(side_section("スニペット"));
        for summary in &snippets {
            list = list.child(snippet_row(state, summary));
        }
    }

    div()
        .w(264.0_f32)
        .h_full()
        .min_h(0.0_f32)
        .bg(theme().surface)
        .border_r(1.0_f32)
        .border_color(theme().border)
        .flex()
        .flex_col()
        .child(
            div()
                .px(10.0_f32)
                .pt(10.0_f32)
                .pb(8.0_f32)
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(2.0_f32)
                        .child(
                            ui_text("Wiki", 15.0, theme().text).font_weight(FontWeight::SEMIBOLD),
                        )
                        .child(ui_text(
                            format!(
                                "{} ページ · {} スニペット",
                                summaries.iter().filter(|summary| summary.is_page()).count(),
                                summaries
                                    .iter()
                                    .filter(|summary| summary.is_snippet())
                                    .count()
                            ),
                            10.0,
                            theme().text_disabled,
                        )),
                )
                .child({
                    let open = state.clone();
                    let button = div()
                        .w(28.0_f32)
                        .h(28.0_f32)
                        .rounded(7.0_f32)
                        .bg(theme().surface_muted)
                        .flex()
                        .items_center()
                        .justify_center()
                        .hover(|style| style.bg(theme().hover))
                        .cursor(CursorIcon::Pointer)
                        .on_click(move |_| open.new_page())
                        .child(icon(icons::NOTE_ADD, 18.0).text_color(theme().accent));
                    tooltip(button, "新規ページ Ctrl+N")
                }),
        )
        .child(
            div()
                .flex_1()
                .min_h(0.0_f32)
                .overflow_y_scroll()
                .child(list),
        )
}

fn side_section(label: &str) -> Div {
    div()
        .mt(18.0_f32)
        .mb(5.0_f32)
        .px(2.0_f32)
        .child(section_label(label))
}

fn copy_wiki_link(state: &AppState, summary: &Summary) {
    set_clipboard(ClipboardKind::Clipboard, format!("wiki://{}", summary.id));
    notify(
        state,
        format!("リンクをコピー · {}", truncate_chars(&summary.title, 24)),
    );
}

fn summary_menu_entries(state: &AppState, summary: &Summary) -> Vec<MenuEntry> {
    let open = state.clone();
    let open_summary = summary.clone();
    let copy = state.clone();
    let copy_summary = summary.clone();
    let rename = state.clone();
    let rename_summary = summary.clone();
    let tags = state.clone();
    let tags_summary = summary.clone();
    let remove = state.clone();
    let remove_summary = summary.clone();
    vec![
        menu_item("開く")
            .on_click(move || open.open_summary(&open_summary))
            .into(),
        menu_item("リンクをコピー")
            .on_click(move || copy_wiki_link(&copy, &copy_summary))
            .into(),
        menu_separator(),
        menu_item("名前を変更…")
            .on_click(move || {
                rename.open_summary(&rename_summary);
                rename.open_palette(PaletteMode::Rename);
            })
            .into(),
        menu_item("タグを編集…")
            .on_click(move || {
                tags.open_summary(&tags_summary);
                tags.open_palette(PaletteMode::Tags);
            })
            .into(),
        menu_separator(),
        menu_item("削除を確認…")
            .on_click(move || {
                remove.open_summary(&remove_summary);
                remove.inspector.set(Inspector::Info);
                remove.inspector_visible.set(true);
                remove.delete_armed.set(true);
            })
            .into(),
    ]
}

fn tab_menu_entries(state: &AppState, index: usize, tab: &Tab) -> Vec<MenuEntry> {
    let close = state.clone();
    let others = state.clone();
    let right = state.clone();
    let copy = state.clone();
    let mut entries = vec![
        menu_item("閉じる")
            .shortcut("Ctrl+W")
            .on_click(move || close.close_tab(index))
            .into(),
        menu_item("他のタブを閉じる")
            .on_click(move || others.close_other_tabs(index))
            .into(),
        menu_item("右側のタブを閉じる")
            .on_click(move || right.close_tabs_to_right(index))
            .into(),
        menu_separator(),
    ];
    if let Some(id) = tab.id.clone() {
        let title = tab.title.clone();
        entries.push(
            menu_item("リンクをコピー")
                .on_click(move || {
                    set_clipboard(ClipboardKind::Clipboard, format!("wiki://{id}"));
                    notify(&copy, format!("リンクをコピー · {title}"));
                })
                .into(),
        );
    }
    entries
}

fn link_menu_entries(state: &AppState, summary: &Summary) -> Vec<MenuEntry> {
    let open = state.clone();
    let open_summary = summary.clone();
    let copy = state.clone();
    let copy_summary = summary.clone();
    vec![
        menu_item("開く")
            .on_click(move || open.open_summary(&open_summary))
            .into(),
        menu_item("リンクをコピー")
            .on_click(move || copy_wiki_link(&copy, &copy_summary))
            .into(),
    ]
}

fn article_menu_entries(state: &AppState, workspace: &Workspace) -> Vec<MenuEntry> {
    let copy = state.clone();
    let copy_page = workspace.page.clone();
    let links = state.clone();
    let edit = state.clone();
    let reload = state.clone();
    vec![
        menu_item("ページのリンクをコピー")
            .on_click(move || {
                set_clipboard(ClipboardKind::Clipboard, format!("wiki://{}", copy_page.id));
                notify(&copy, "ページのリンクをコピー");
            })
            .into(),
        menu_item("ページ情報を表示")
            .shortcut("Ctrl+I")
            .on_click(move || {
                links.inspector.set(Inspector::Links);
                links.inspector_visible.set(true);
            })
            .into(),
        menu_separator(),
        menu_item("Neovide で編集")
            .shortcut("Ctrl+E")
            .on_click(move || edit.open_editor())
            .into(),
        menu_item("再読み込み")
            .shortcut("Ctrl+R")
            .on_click(move || reload.refresh())
            .into(),
    ]
}

fn page_row(state: &AppState, summary: &Summary, active: bool) -> Div {
    let click_state = state.clone();
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
        .cursor(CursorIcon::Pointer)
        .on_click(move |_| click_state.open_summary(&click_summary))
        .child(icon(icons::DESCRIPTION, 16.0).text_color(if active {
            theme().accent
        } else {
            theme().text_disabled
        }))
        .child(ui_text(
            truncate_chars(&summary.title, 20),
            13.0,
            theme().text,
        ));
    if active {
        row = row
            .bg(theme().accent_muted)
            .border_l(2.0_f32)
            .border_color(theme().accent)
            .pl(7.0_f32);
    }
    let menu = summary_menu_entries(state, &summary);
    context_menu(tooltip(row, summary.title.clone()), menu)
}

fn snippet_row(state: &AppState, summary: &Summary) -> Div {
    let click_state = state.clone();
    let summary = summary.clone();
    let click_summary = summary.clone();
    let title = summary.title.clone();
    let row = div()
        .h(29.0_f32)
        .px(9.0_f32)
        .rounded(6.0_f32)
        .flex()
        .flex_row()
        .items_center()
        .gap(8.0_f32)
        .hover(|style| style.bg(theme().surface_muted))
        .cursor(CursorIcon::Pointer)
        .on_click(move |_| click_state.open_summary(&click_summary))
        .child(icon(icons::CODE, 15.0).text_color(theme().text_disabled))
        .child(ui_text(
            truncate_chars(&summary.title, 20),
            13.0,
            theme().text,
        ));
    let menu = summary_menu_entries(state, &summary);
    context_menu(tooltip(row, title), menu)
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
                .node_ref(state.article_ref)
                .on_scrolled({
                    let state = state.clone();
                    move |_| {
                        let state = state.clone();
                        request_frame(move |_| sync_active_heading(&state));
                    }
                })
                .child(article_scroll(state)),
        )
}

fn tabs(state: &AppState) -> Div {
    let tabs = state.tabs.get();
    let active = state.active_index();
    let keyboard_focus = state.tab_focus.get();
    let mut row = div()
        .h(38.0_f32)
        .px(12.0_f32)
        .flex()
        .flex_row()
        .items_center()
        .gap(2.0_f32)
        .bg(theme().page)
        .border_b(1.0_f32)
        .border_color(theme().border);
    if tabs.is_empty() {
        row = row
            .child(ui_text("ページがありません", 11.0, theme().text_disabled))
            .child(div().flex_1());
    }
    for (index, tab) in tabs.iter().enumerate() {
        let is_active = index == active;
        let mut tab_view = div()
            .h(28.0_f32)
            .px(10.0_f32)
            .rounded(7.0_f32)
            .border(1.0_f32)
            .border_color(if is_active && keyboard_focus {
                theme().accent
            } else {
                Color::TRANSPARENT
            })
            .flex()
            .flex_row()
            .items_center()
            .gap(7.0_f32)
            .hover(|style| style.bg(theme().surface_muted))
            .cursor(CursorIcon::Pointer)
            .on_click({
                let state = state.clone();
                move |_| state.switch_tab(index)
            })
            .child(
                icon(
                    if tab.kind == "snippet" {
                        icons::CODE
                    } else {
                        icons::DESCRIPTION
                    },
                    14.0,
                )
                .text_color(if is_active {
                    theme().accent_text
                } else {
                    theme().text_disabled
                }),
            )
            .child(ui_text(
                truncate_chars(&tab.title, 22),
                12.0,
                if is_active {
                    theme().text
                } else {
                    theme().text_muted
                },
            ))
            .child(
                div()
                    .w(17.0_f32)
                    .h(17.0_f32)
                    .rounded(4.0_f32)
                    .flex()
                    .items_center()
                    .justify_center()
                    .hover(|style| style.bg(theme().pressed))
                    .cursor(CursorIcon::Pointer)
                    .on_click({
                        let state = state.clone();
                        move |event| {
                            event.stop_propagation();
                            state.close_tab(index);
                        }
                    })
                    .child(icon(icons::CLOSE, 14.0).text_color(theme().text_disabled)),
            );
        if is_active {
            tab_view = tab_view.bg(theme().surface_muted);
        }
        row = row.child(context_menu(tab_view, tab_menu_entries(state, index, tab)));
    }
    row.child({
        let open = state.clone();
        let button = div()
            .w(28.0_f32)
            .h(28.0_f32)
            .ml(2.0_f32)
            .rounded(7.0_f32)
            .flex()
            .items_center()
            .justify_center()
            .hover(|style| style.bg(theme().surface_muted))
            .cursor(CursorIcon::Pointer)
            .on_click(move |_| open.open_palette(PaletteMode::QuickOpen))
            .child(icon(icons::MANAGE_SEARCH, 17.0).text_color(theme().text_disabled));
        tooltip(button, "クイックオープン Ctrl+P")
    })
}

fn article_scroll(state: &AppState) -> Div {
    let tab = state.active_tab();
    let index = state.active_index();
    let show_empty = state.summaries.get().is_empty()
        && tab.as_ref().and_then(|tab| tab.id.as_deref()).is_none();
    let (body, menu) = if show_empty {
        (empty_state(state), None)
    } else {
        match tab {
            Some(tab) => match tab.workspace.clone() {
                Some(workspace) => {
                    let menu = article_menu_entries(state, &workspace);
                    (article(state, &workspace), Some(menu))
                }
                None => (placeholder(state, index, &tab), None),
            },
            None => (
                div()
                    .p(40.0_f32)
                    .child(ui_text("タブがありません。", 13.0, theme().text_muted)),
                None,
            ),
        }
    };
    let page = div()
        .w_full()
        .max_w(800.0_f32)
        .px(56.0_f32)
        .pt(52.0_f32)
        .pb(96.0_f32)
        .child(body);
    let page = match menu {
        Some(menu) => context_menu(page, menu),
        None => page,
    };
    div().w_full().flex().justify_center().child(page)
}

fn empty_state(state: &AppState) -> Div {
    let online = matches!(state.source.get(), Source::Server);
    let create = state.clone();
    let retry = state.clone();
    let mut action = div().mt(18.0_f32).flex().flex_row().gap(8.0_f32);
    if online {
        action = action.child(
            icon_text_button(
                icons::NOTE_ADD,
                "最初のページを作成",
                11.5,
                theme().accent_text,
            )
            .on_click(move |_| create.new_page()),
        );
    } else {
        action = action
            .child(
                icon_text_button(icons::REFRESH, "再試行", 11.5, theme().accent_text)
                    .on_click(move |_| retry.refresh()),
            )
            .child(ui_text(
                "wiki-server を起動してから再試行してください。",
                10.5,
                theme().text_disabled,
            ));
    }
    div()
        .pt(72.0_f32)
        .max_w(520.0_f32)
        .child(ui_text("ノート", 11.0, theme().text_disabled))
        .child(
            ui_text("Wiki へようこそ", 30.0, theme().text)
                .mt(8.0_f32)
                .font_weight(FontWeight::BOLD),
        )
        .child(
            ui_text(
                if online {
                    "ページはまだありません。最初の1枚を作りましょう。"
                } else {
                    "ローカルサーバーに接続できません。表示できるページもまだありません。"
                },
                13.5,
                theme().text_muted,
            )
            .mt(12.0_f32)
            .line_height(LineHeight::Relative(1.7_f32)),
        )
        .child(action)
        .child(
            ui_text(
                if online {
                    "Ctrl+N で新規ページ、Ctrl+K で検索できます。"
                } else {
                    "Ctrl+K で検索できます。接続後に Ctrl+N で新規ページ。"
                },
                10.5,
                theme().text_disabled,
            )
            .mt(14.0_f32),
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
    let backlinks_state = state.clone();

    let mut body = div()
        .flex()
        .flex_col()
        .child(
            ui_text(&page.title, 33.0, theme().text)
                .font_family(display())
                .mt(0.0_f32)
                .font_weight(FontWeight::BOLD)
                .letter_spacing(-0.3_f32)
                .line_height(LineHeight::Relative(1.32)),
        )
        .child(
            div()
                .mt(12.0_f32)
                .pb(18.0_f32)
                .flex()
                .flex_row()
                .items_center()
                .flex_wrap()
                .gap(8.0_f32)
                .border_b(1.0_f32)
                .border_color(rgba(0xffffff0a))
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
                .child(
                    div()
                        .px(4.0_f32)
                        .rounded(4.0_f32)
                        .hover(|style| style.bg(theme().surface_muted))
                        .cursor(CursorIcon::Pointer)
                        .on_click(move |_| {
                            backlinks_state.inspector.set(Inspector::Links);
                            backlinks_state.inspector_visible.set(true);
                            backlinks_state.outline_open.set(false);
                        })
                        .child(ui_text(
                            format!("{} バックリンク", workspace.links.backlinks.len()),
                            12.0,
                            theme().text_muted,
                        )),
                ),
        );

    state.heading_refs.borrow_mut().clear();
    body = body.child(render_markdown_body(
        state,
        workspace,
        Some(&state.heading_refs),
    ));

    // Backlinks belong at the end of the reading flow, not only behind Ctrl+I. This is the
    // feature that makes a wiki feel connected, so it stays visible by default.
    let backlinks = &workspace.links.backlinks;
    if !backlinks.is_empty() {
        let mut rows = div().mt(8.0_f32).flex().flex_col();
        for summary in backlinks.iter().take(4) {
            let state = state.clone();
            let summary = summary.clone();
            let target = summary.clone();
            let excerpt = if summary.excerpt.is_empty() {
                summary.kind_label()
            } else {
                summary.excerpt.clone()
            };
            rows = rows.child(
                div()
                    .py(9.0_f32)
                    .border_b(1.0_f32)
                    .border_color(rgba(0xffffff0a))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(10.0_f32)
                    .hover(|style| style.bg(theme().surface_muted))
                    .rounded(6.0_f32)
                    .px(6.0_f32)
                    .cursor(CursorIcon::Pointer)
                    .on_click(move |_| state.open_summary(&target))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(3.0_f32)
                            .flex_1()
                            .min_w(0.0_f32)
                            .child(ui_text(
                                truncate_chars(&summary.title, 40),
                                12.5,
                                theme().text,
                            ))
                            .child(ui_text(
                                truncate_chars(&excerpt, 64),
                                10.5,
                                theme().text_disabled,
                            )),
                    )
                    .child(icon(icons::CHEVRON_RIGHT, 16.0).text_color(theme().text_disabled)),
            );
        }
        let mut section = div()
            .mt(46.0_f32)
            .pt(18.0_f32)
            .border_t(1.0_f32)
            .border_color(theme().border)
            .child(
                div()
                    .flex()
                    .justify_between()
                    .child(section_label("このページを参照"))
                    .child(ui_text(
                        format!("{} 件", backlinks.len()),
                        10.0,
                        theme().text_disabled,
                    )),
            )
            .child(rows);
        if backlinks.len() > 4 {
            let state = state.clone();
            section = section.child(
                icon_text_button(
                    icons::LINK,
                    "すべてのリンクを表示",
                    11.0,
                    theme().accent_text,
                )
                .mt(10.0_f32)
                .on_click(move |_| {
                    state.inspector.set(Inspector::Links);
                    state.inspector_visible.set(true);
                }),
            );
        }
        body = body.child(section);
    }

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

enum BodySegment {
    Heading(u8, String),
    Markdown(String),
    Code(String, Vec<String>),
    Embed(String),
    Transclude(String),
    Interpreter(String, String),
}

fn flush_markdown(markdown: &mut String, segments: &mut Vec<BodySegment>) {
    if !markdown.trim().is_empty() {
        segments.push(BodySegment::Markdown(std::mem::take(markdown)));
    } else {
        markdown.clear();
    }
}

/// Splits a page at the things `markdown_view` cannot own: wiki embeds, transclusions,
/// interpreter panels and fenced code that keeps the app's copy button. Headings start a new
/// segment too, so each heading gets a `NodeRef` for the outline and scroll spy.
fn split_segments(body: &str, title: &str) -> Vec<BodySegment> {
    let lines = body.lines().collect::<Vec<_>>();
    let calls = parse_interpreter_calls(body);
    let mut segments = Vec::new();
    let mut markdown = String::new();
    let mut index = 0usize;
    while index < lines.len() {
        let raw = lines[index].trim_end_matches('\r');
        let line = raw.trim();
        if line.is_empty() {
            markdown.push('\n');
            index += 1;
            continue;
        }
        if let Some(info) = line.strip_prefix("!```") {
            flush_markdown(&mut markdown, &mut segments);
            let preset = info.split("||").next().unwrap_or("text").trim().to_owned();
            let id = info.split("||").nth(1).map(str::trim).unwrap_or_default();
            index += 1;
            let mut code = Vec::new();
            while index < lines.len() && !lines[index].trim_start().starts_with("```") {
                code.push(lines[index].trim_end_matches('\r').to_owned());
                index += 1;
            }
            index += 1;
            if id.is_empty() {
                // A loose fence has no canonical id yet. Showing it as code is better than
                // dropping the content; the Neovim side can convert it later.
                segments.push(BodySegment::Code(preset, code));
            } else {
                segments.push(BodySegment::Embed(id.to_owned()));
            }
            continue;
        }
        if let Some(info) = line.strip_prefix("```") {
            flush_markdown(&mut markdown, &mut segments);
            let language = info.split('|').next().unwrap_or("text").trim().to_owned();
            index += 1;
            let mut code = Vec::new();
            while index < lines.len() && !lines[index].trim_start().starts_with("```") {
                code.push(lines[index].trim_end_matches('\r').to_owned());
                index += 1;
            }
            index += 1;
            segments.push(BodySegment::Code(language, code));
            continue;
        }
        if let Some(call) = calls.iter().find(|call| call.line == index + 1) {
            flush_markdown(&mut markdown, &mut segments);
            segments.push(BodySegment::Interpreter(
                call.provider.clone(),
                call.args.clone(),
            ));
            index += 1;
            continue;
        }
        if let Some((level, text)) = heading(line) {
            flush_markdown(&mut markdown, &mut segments);
            index += 1;
            if level == 1 && text == title {
                continue;
            }
            segments.push(BodySegment::Heading(level, text.to_owned()));
            continue;
        }
        if let Some(target) = line.strip_prefix("![[")
            && let Some(target) = target.strip_suffix("]]")
        {
            flush_markdown(&mut markdown, &mut segments);
            segments.push(BodySegment::Transclude(target.trim().to_owned()));
            index += 1;
            continue;
        }
        markdown.push_str(raw);
        markdown.push('\n');
        index += 1;
    }
    flush_markdown(&mut markdown, &mut segments);
    segments
}

/// `markdown_view` understands `[label](target)` but not `[[target|label]]`; turning one into the
/// other keeps wiki links clickable while the standard Markdown renderer does the paragraph work.
fn wiki_links_to_markdown(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("[[") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("]]") else {
            out.push_str(&rest[start..]);
            return out;
        };
        let inner = &after[..end];
        let (target, label) = inner.rsplit_once('|').unwrap_or((inner, inner));
        out.push('[');
        out.push_str(label);
        out.push_str("](wiki:");
        out.push_str(target.trim());
        out.push(')');
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

#[allow(clippy::field_reassign_with_default)]
fn wiki_markdown_theme() -> MarkdownTheme {
    let mut markdown = MarkdownTheme::default();
    markdown.root = Style::default()
        .font_family(sans())
        .text_size(15.5_f32)
        .text_color(theme().text)
        .line_height(LineHeight::Relative(1.78_f32));
    markdown.paragraph = Style::default().mb(15.0_f32);
    markdown.strong = Style::default()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme().text);
    markdown.emphasis = Style::default().italic();
    markdown.strikethrough = Style::default().strikethrough();
    markdown.code = Style::default()
        .font_family(mono())
        .text_size(13.0_f32)
        .bg(theme().surface_muted)
        .text_color(theme().accent_text)
        .rounded(4.0_f32)
        .px(4.0_f32);
    markdown.link = Style::default().text_color(theme().accent_text).underline();
    markdown.link_hover = Style::default()
        .text_color(theme().accent_hover)
        .underline();
    markdown.quote = Style::default()
        .border_l(3.0_f32)
        .border_color(theme().accent_muted)
        .px(16.0_f32)
        .py(10.0_f32)
        .bg(rgba(0xffffff06))
        .text_color(theme().text_muted)
        .mb(20.0_f32);
    markdown.list = Style::default().mb(14.0_f32);
    markdown.item_gap = 6.0_f32;
    markdown.indent = 22.0_f32;
    markdown.table = Style::default().mb(18.0_f32);
    markdown.table_head = Style::default()
        .bg(theme().surface_muted)
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme().text);
    markdown.table_cell = Style::default()
        .px(12.0_f32)
        .py(7.0_f32)
        .border(1.0_f32)
        .border_color(theme().border)
        .text_size(13.0_f32);
    markdown.table_stripe = Color::TRANSPARENT;
    markdown.rule = Style::default().h(1.0_f32).my(26.0_f32).bg(theme().border);
    markdown.task_done = Style::default()
        .strikethrough()
        .text_color(theme().text_disabled);
    markdown
}

fn handle_markdown_link(state: &AppState, target: &str) {
    if let Some(id) = target.strip_prefix("wiki:") {
        open_wiki_target(state, id);
    } else if target.starts_with("http://") || target.starts_with("https://") {
        set_clipboard(ClipboardKind::Clipboard, target.to_owned());
        notify(
            state,
            format!("URLをコピー · {}", truncate_chars(target, 40)),
        );
    }
}

fn render_markdown_body(
    state: &AppState,
    workspace: &Workspace,
    collect: Option<&RefCell<Vec<NodeRef>>>,
) -> Div {
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

    let mut rendered = false;
    for segment in split_segments(&page.body, &page.title) {
        rendered = true;
        match segment {
            BodySegment::Heading(level, text) => {
                let (size, top) = match level {
                    1 => (28.0_f32, 40.0_f32),
                    2 => (24.0_f32, 38.0_f32),
                    3 => (19.5_f32, 28.0_f32),
                    4 => (16.0_f32, 22.0_f32),
                    _ => (15.0_f32, 18.0_f32),
                };
                let heading = rich_text(inline_runs(&text, Some(state)))
                    .font_family(if level <= 3 { display() } else { sans() })
                    .text_size(size)
                    .text_color(theme().text)
                    .font_weight(FontWeight::SEMIBOLD)
                    .letter_spacing(-0.2_f32)
                    .line_height(LineHeight::Relative(1.45_f32))
                    .mt(top)
                    .mb(10.0_f32);
                if level >= 2
                    && let Some(collect) = collect
                {
                    let anchor = node_ref();
                    collect.borrow_mut().push(anchor);
                    out = out.child(div().node_ref(anchor).child(heading));
                } else {
                    out = out.child(heading);
                }
            }
            BodySegment::Markdown(text) => {
                let link_state = state.clone();
                out = out.child(
                    markdown_view(wiki_links_to_markdown(&text))
                        .theme(wiki_markdown_theme())
                        .on_link(move |target| handle_markdown_link(&link_state, target)),
                );
            }
            BodySegment::Code(language, lines) => {
                let refs = lines.iter().map(String::as_str).collect::<Vec<_>>();
                out = out.child(code_panel(&language, &refs));
            }
            BodySegment::Embed(id) => {
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
            BodySegment::Transclude(target) => {
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
                    let mut nested = workspace.clone();
                    nested.page.body = body;
                    nested.page.title.clear();
                    out = out.child(
                        div()
                            .mt(18.0_f32)
                            .p(12.0_f32)
                            .rounded(8.0_f32)
                            .bg(theme().surface_muted)
                            .border(1.0_f32)
                            .border_color(theme().border)
                            .child(render_markdown_body(state, &nested, None)),
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
            BodySegment::Interpreter(provider, args) => {
                out = out.child(render_interpreter(state, workspace, &provider, &args));
            }
        }
    }
    if !rendered {
        let edit = state.clone();
        out = out.child(
            div()
                .mt(28.0_f32)
                .p(20.0_f32)
                .rounded(10.0_f32)
                .bg(theme().surface_muted)
                .border(1.0_f32)
                .border_color(theme().border)
                .flex()
                .flex_col()
                .gap(8.0_f32)
                .child(ui_text("このページはまだ空です。", 13.0, theme().text))
                .child(ui_text(
                    "Ctrl+E で Neovide を開いて書き始められます。",
                    11.0,
                    theme().text_disabled,
                ))
                .child(
                    icon_text_button(
                        icons::EDIT,
                        "Neovide で書き始める",
                        11.5,
                        theme().accent_text,
                    )
                    .on_click(move |_| edit.open_editor()),
                ),
        );
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
        && let Ok(depth) = depth.trim().parse::<u64>()
    {
        params["depth"] = json!(depth.min(3));
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
    let plain = lines.join("\n");
    div()
        .mt(22.0_f32)
        .rounded(10.0_f32)
        .bg(theme().surface_muted)
        .border(1.0_f32)
        .border_color(theme().border)
        .overflow_hidden()
        .child(
            div()
                .px(14.0_f32)
                .py(7.0_f32)
                .flex()
                .flex_row()
                .items_center()
                .gap(10.0_f32)
                .border_b(1.0_f32)
                .border_color(rgba(0xffffff0d))
                .child(mono_text(language, 10.0, theme().text_disabled))
                .child(div().flex_1())
                .child(
                    div()
                        .h(22.0_f32)
                        .px(7.0_f32)
                        .rounded(4.0_f32)
                        .flex()
                        .items_center()
                        .hover(|style| style.bg(theme().hover))
                        .cursor(CursorIcon::Pointer)
                        .on_click(move |_| {
                            set_clipboard(ClipboardKind::Clipboard, plain.clone());
                        })
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(4.0_f32)
                                .child(icon(icons::CONTENT_COPY, 13.0))
                                .child(ui_text("コピー", 10.0, theme().text_disabled)),
                        ),
                ),
        )
        .child(
            div()
                .px(14.0_f32)
                .py(11.0_f32)
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
        if let Some(target) = line.strip_prefix("![[")
            && let Some(target) = target.strip_suffix("]]")
        {
            index += 1;
            blocks.push(MdBlock::Transclude(target.to_owned()));
            continue;
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
/// Splits one line of Markdown into styled runs: `**bold**`, `*italic*`, `~~strike~~`,
/// `` `code` ``, `[label](url)` and `[[target|label]]`. Wiki links stay on the reading surface;
/// they open by title through the link index, not by navigating away from the page.
fn inline_runs(text: &str, state: Option<&AppState>) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    let mut plain = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("[[")
            && let Some(end) = after.find("]]")
        {
            let inner = &after[..end];
            let (target, label) = inner.rsplit_once('|').unwrap_or((inner, inner));
            flush_plain(&mut runs, &mut plain);
            let mut run = span(label).text_color(theme().accent_text).underline();
            if let Some(state) = state {
                let state = state.clone();
                let target = target.trim().to_owned();
                run = run.on_click(move || open_wiki_target(&state, &target));
            }
            runs.push(run.into());
            rest = &after[end + 2..];
            continue;
        }
        if let Some(after) = rest.strip_prefix("**")
            && let Some(end) = after.find("**")
            && end > 0
        {
            flush_plain(&mut runs, &mut plain);
            runs.push(
                span(&after[..end])
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme().text)
                    .into(),
            );
            rest = &after[end + 2..];
            continue;
        }
        if let Some(after) = rest.strip_prefix("~~")
            && let Some(end) = after.find("~~")
            && end > 0
        {
            flush_plain(&mut runs, &mut plain);
            runs.push(span(&after[..end]).strikethrough().into());
            rest = &after[end + 2..];
            continue;
        }
        if let Some(after) = rest.strip_prefix('`')
            && let Some(end) = after.find('`')
            && end > 0
        {
            flush_plain(&mut runs, &mut plain);
            runs.push(
                span(&after[..end])
                    .font_family(mono())
                    .text_size(13.0_f32)
                    .bg(theme().surface_muted)
                    .text_color(theme().accent_text)
                    .px(4.0_f32)
                    .rounded(4.0_f32)
                    .into(),
            );
            rest = &after[end + 1..];
            continue;
        }
        if let Some(after) = rest.strip_prefix('[')
            && let Some(close) = after.find(']')
            && after[close + 1..].starts_with('(')
            && let Some(paren) = after[close + 1..].find(')')
        {
            let label = &after[..close];
            flush_plain(&mut runs, &mut plain);
            runs.push(
                span(label)
                    .text_color(theme().accent_text)
                    .underline()
                    .into(),
            );
            rest = &after[close + 1 + paren + 1..];
            continue;
        }
        if let Some(after) = rest.strip_prefix('*')
            && !after.starts_with('*')
            && let Some(end) = after.find('*')
            && end > 0
        {
            flush_plain(&mut runs, &mut plain);
            runs.push(span(&after[..end]).italic().into());
            rest = &after[end + 1..];
            continue;
        }
        let mut chars = rest.char_indices();
        let (_, first) = chars.next().expect("non-empty");
        plain.push(first);
        rest = &rest[first.len_utf8()..];
    }
    flush_plain(&mut runs, &mut plain);
    if runs.is_empty() {
        runs.push(Run::Text(String::new()));
    }
    runs
}

/// Resolves a `[[target|label]]` by title or Content ID against the summary list, then opens it
/// as a tab. The link parser stays free of navigation policy.
fn open_wiki_target(state: &AppState, target: &str) {
    let (page, fragment) = match target.split_once('#') {
        Some((page, fragment)) => (page.trim(), Some(fragment.trim())),
        None => (target.trim(), None),
    };
    let page = page.strip_prefix("id:").unwrap_or(page).trim();
    let lower = page.to_lowercase();
    let summaries = state.summaries.get_untracked();
    let found = summaries.iter().find(|summary| {
        summary.id == page
            || summary.title == page
            || summary.title.to_lowercase() == lower
            || summary
                .aliases
                .iter()
                .any(|alias| alias == page || alias.to_lowercase() == lower)
    });
    let Some(summary) = found.cloned() else {
        return;
    };
    state.open_summary(&summary);
    if let Some(fragment) = fragment.filter(|fragment| !fragment.is_empty()) {
        let state = state.clone();
        let fragment = fragment.to_owned();
        request_frame(move |_| scroll_to_fragment(&state, &fragment));
    }
}

fn scroll_to_fragment(state: &AppState, fragment: &str) {
    let Some(workspace) = state.active_workspace() else {
        return;
    };
    let items = workspace
        .outline
        .iter()
        .filter(|item| item.level >= 2)
        .collect::<Vec<_>>();
    let needle = fragment.to_lowercase();
    if let Some((index, item)) = items.iter().enumerate().find(|(_, item)| {
        let title = item.title.to_lowercase();
        title == needle || title.contains(&needle)
    }) {
        state.outline_active.set(index);
        state.scroll_to_outline(index, item.line);
    }
}

fn flush_plain(runs: &mut Vec<Run>, plain: &mut String) {
    if !plain.is_empty() {
        runs.push(Run::Text(std::mem::take(plain)));
    }
}

fn rich_paragraph(
    state: &AppState,
    text: &str,
    size: f32,
    color: Color,
    line_height: f32,
) -> RichText {
    rich_text(inline_runs(text, Some(state)))
        .font_family(sans())
        .text_size(size)
        .text_color(color)
        .line_height(LineHeight::Relative(line_height))
        .flex_shrink(0.0_f32)
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
    let mut badges = Vec::new();
    if let Some(format) = snippet.format.as_deref()
        && !format.is_empty()
        && format != language
    {
        badges.push(format.to_owned());
    }
    if !kinds.is_empty() {
        badges.push(kinds);
    }
    let meta = badges.join(" · ");
    let id = snippet.id.clone();
    let title = snippet.title.clone();
    let body = snippet.body.clone();
    div()
        .mt(24.0_f32)
        .rounded(10.0_f32)
        .bg(theme().surface_muted)
        .overflow_hidden()
        .hover(|style| style.bg(theme().hover))
        .on_click({
            let state = state.clone();
            move |_| state.open_id(&id, &title, "snippet")
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
                ))
                .child(div().flex_1())
                .child(
                    icon_button(icons::CONTENT_COPY, "コードをコピー").on_click(move |event| {
                        event.stop_propagation();
                        set_clipboard(ClipboardKind::Clipboard, body.clone());
                    }),
                ),
        )
        .child(
            div()
                .px(14.0_f32)
                .pb(11.0_f32)
                .child(code_block(&lines, &language)),
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

/// The outline rows, shared by the margin rail and the standalone panel.
fn outline_key_items(state: &AppState) -> Vec<OutlineItem> {
    let outline = state
        .active_workspace()
        .map(|workspace| workspace.outline)
        .unwrap_or_default();
    let mut items = outline
        .iter()
        .filter(|item| item.level >= 2)
        .cloned()
        .collect::<Vec<_>>();
    if items.is_empty() {
        items = outline;
    }
    items
}

fn outline_nav(state: &AppState) -> (Div, bool) {
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
    let keyboard_focus = state.outline_keyboard.get() || state.outline_open.get();
    for (index, item) in items.iter().enumerate() {
        let active = index == active_outline;
        let row = div()
            .py(5.0_f32)
            .pl(9.0_f32 + (item.level.saturating_sub(2) as f32) * 8.0)
            .pr(6.0_f32)
            .border_l(2.0_f32)
            .border_color(if active {
                theme().accent
            } else {
                Color::TRANSPARENT
            })
            .rounded(4.0_f32)
            .bg(if active && keyboard_focus {
                theme().accent_muted
            } else {
                Color::TRANSPARENT
            })
            .hover(|style| style.bg(theme().surface_muted))
            .cursor(CursorIcon::Pointer)
            .on_click({
                let state = state.clone();
                let line = item.line;
                move |_| {
                    state.outline_active.set(index);
                    state.scroll_to_outline(index, line);
                    state.outline_open.set(false);
                }
            })
            .child(ui_text(
                truncate_chars(&item.title, 26),
                if item.level >= 3 { 11.5 } else { 12.0 },
                if active {
                    theme().text
                } else {
                    theme().text_muted
                },
            ));
        nav = nav.child(tooltip(row, item.title.clone()));
    }
    if items.is_empty() {
        nav = nav.child(
            ui_text("本文に目次はありません。", 11.0, theme().text_disabled)
                .px(9.0_f32)
                .py(5.0_f32),
        );
    }
    (nav, items.is_empty())
}

fn outline_rail_fits(state: &AppState) -> bool {
    let sidebar = if state.sidebar_open.get() { 264.0 } else { 0.0 };
    let inspector = if state.inspector_visible.get() {
        360.0
    } else {
        0.0
    };
    state.viewport.get() - sidebar - inspector >= 900.0
}

fn toggle_outline(state: &AppState) {
    if outline_rail_fits(state) {
        state.outline_open.set(false);
        state
            .outline_keyboard
            .update(|focused| *focused = !*focused);
    } else {
        state.outline_keyboard.set(false);
        state.outline_collapsed.set(false);
        state.outline_open.update(|open| *open = !*open);
    }
}

fn outline_column(state: &AppState) -> Div {
    let (nav, empty) = outline_nav(state);
    // A borderless rail inside the reading block, not an overlay and not an inspector tab.
    // It only disappears when the block would leave the article too narrow; `Ctrl+3` then
    // opens the standalone outline panel.
    let mut rail = div()
        .w(176.0_f32)
        .flex_shrink(0.0_f32)
        .h_full()
        .px(8.0_f32)
        .pt(54.0_f32)
        .pb(22.0_f32)
        .flex()
        .flex_col()
        .child(
            div()
                .mb(10.0_f32)
                .px(9.0_f32)
                .flex()
                .flex_row()
                .items_center()
                .gap(6.0_f32)
                .child(icon(icons::TOC, 14.0).text_color(theme().text_muted))
                .child(section_label("目次")),
        )
        .child(div().flex_1().min_h(0.0_f32).overflow_y_scroll().child(nav));
    if !outline_rail_fits(state)
        || empty
        || state.outline_open.get()
        || state.outline_collapsed.get()
    {
        rail = rail.hidden();
    }
    rail
}

/// The outline as its own floating surface. It is separate from the inspector so reading
/// structure and page properties never compete for the same segmented control.
fn outline_panel(state: &AppState) -> Show {
    let state = state.clone();
    show(
        move || state.outline_open.get(),
        move || {
            let close = state.clone();
            let (nav, _) = outline_nav(&state);
            div()
                .absolute()
                .right(16.0_f32)
                .top(56.0_f32)
                .bottom(28.0_f32)
                .w(280.0_f32)
                .z_index(30)
                .rounded(12.0_f32)
                .bg(theme().overlay)
                .border(1.0_f32)
                .border_color(theme().border_strong)
                .shadow_lg()
                .px(14.0_f32)
                .py(14.0_f32)
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(7.0_f32)
                        .child(icon(icons::TOC, 15.0).text_color(theme().accent_text))
                        .child(
                            ui_text("目次", 13.0, theme().text).font_weight(FontWeight::SEMIBOLD),
                        )
                        .child(div().flex_1())
                        .child(
                            div()
                                .w(26.0_f32)
                                .h(26.0_f32)
                                .rounded(6.0_f32)
                                .flex()
                                .items_center()
                                .justify_center()
                                .hover(|style| style.bg(theme().surface_muted))
                                .cursor(CursorIcon::Pointer)
                                .on_click(move |_| close.outline_open.set(false))
                                .child(icon(icons::CLOSE, 15.0).text_color(theme().text_disabled)),
                        ),
                )
                .child(
                    div()
                        .mt(10.0_f32)
                        .flex_1()
                        .min_h(0.0_f32)
                        .overflow_y_scroll()
                        .child(nav),
                )
        },
    )
}

// ---------------------------------------------------------------------------
// Inspector
// ---------------------------------------------------------------------------

fn inspector_column(state: &AppState) -> Option<Div> {
    if !state.inspector_visible.get() {
        return None;
    }
    let mode = state.inspector.get();
    let close_state = state.clone();
    let mut column = div()
        .w(360.0_f32)
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
                .flex()
                .flex_row()
                .items_center()
                .gap(6.0_f32)
                .child(
                    div()
                        .flex_1()
                        .p(2.0_f32)
                        .rounded(7.0_f32)
                        .bg(theme().surface_muted)
                        .flex()
                        .flex_row()
                        .gap(2.0_f32)
                        .child(segment_button(
                            state,
                            icons::LINK,
                            "リンク",
                            "Ctrl+1",
                            Inspector::Links,
                            mode,
                        ))
                        .child(segment_button(
                            state,
                            icons::INFO,
                            "ページ情報",
                            "Ctrl+2",
                            Inspector::Info,
                            mode,
                        )),
                )
                .child(
                    div()
                        .w(26.0_f32)
                        .h(26.0_f32)
                        .rounded(6.0_f32)
                        .flex()
                        .items_center()
                        .justify_center()
                        .hover(|style| style.bg(theme().surface_muted))
                        .cursor(CursorIcon::Pointer)
                        .on_click(move |_| close_state.inspector_visible.set(false))
                        .child(icon(icons::CLOSE, 16.0).text_color(theme().text_disabled)),
                ),
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
    symbol: &'static anti_webui_symbols_outlined::Symbol,
    label: &str,
    shortcut: &str,
    value: Inspector,
    current: Inspector,
) -> Div {
    let active = value == current;
    let button = div()
        .flex_1()
        .h(28.0_f32)
        .rounded(6.0_f32)
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
        .cursor(CursorIcon::Pointer)
        .on_click({
            let state = state.clone();
            move |_| state.inspector.set(value)
        })
        .child(icon(symbol, 14.0).text_color(if active {
            theme().text
        } else {
            theme().text_muted
        }))
        .child(ui_text(
            label,
            11.0,
            if active {
                theme().text
            } else {
                theme().text_muted
            },
        ));
    tooltip(button, format!("{label} {shortcut}"))
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
                .cursor(CursorIcon::Pointer)
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
                .child(ui_text("Enter 開く", 9.0, theme().text_disabled)),
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
        related = related.child(link_row(state, summary, None));
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
        groups = groups.child(link_row(state, summary, Some(icons::CALL_MADE)));
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
        groups = groups.child(link_row(state, summary, Some(icons::LINK)));
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
        .cursor(CursorIcon::Pointer)
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

fn link_row(
    state: &AppState,
    summary: &Summary,
    arrow: Option<&'static anti_webui_symbols_outlined::Symbol>,
) -> Div {
    let click_state = state.clone();
    let summary = summary.clone();
    let click_summary = summary.clone();
    let updated = summary.updated_label.clone();
    let mut row = div()
        .py(4.0_f32)
        .flex()
        .flex_row()
        .items_center()
        .gap(8.0_f32)
        .border_b(1.0_f32)
        .border_color(rgb(0x2f2f31))
        .hover(|style| style.bg(theme().surface_muted))
        .cursor(CursorIcon::Pointer)
        .on_click(move |_| click_state.open_summary(&click_summary))
        .child(ui_text(truncate_chars(&summary.title, 30), 11.5, theme().text_muted).flex_1());
    row = match arrow {
        Some(arrow) => row.child(icon(arrow, 15.0).text_color(theme().text_disabled)),
        None => row.child(ui_text(updated, 9.5, theme().text_disabled)),
    };
    context_menu(row, link_menu_entries(state, &summary))
}

fn inspector_info(state: &AppState) -> Div {
    let workspace = state.active_workspace();
    let editor = state.editor.get();
    let source = state.source.get();
    let Some(workspace) = workspace else {
        return div().child(section_label("基本情報")).child(
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
        .child(section_label("基本情報"))
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
        ));
    if !workspace.page.aliases.is_empty() {
        let aliases = workspace.page.aliases.join(", ");
        content = content.child(info_row("エイリアス", &aliases));
    }
    content = content
        .child(divider())
        .child(section_label("連携"))
        .child(
            div()
                .mt(6.0_f32)
                .flex()
                .items_center()
                .gap(7.0_f32)
                .child(dot(if state.providers.get().is_empty() {
                    theme().text_disabled
                } else {
                    theme().success
                }))
                .child(ui_text(
                    if state.providers.get().is_empty() {
                        "Interpreter は未登録"
                    } else {
                        "Interpreter を検出"
                    },
                    12.0,
                    theme().text_muted,
                )),
        )
        .child(
            ui_text(
                if state.providers.get().is_empty() {
                    "接続すると使えます。".to_owned()
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
                    format!("Ctrl+E で開きます。注意: {warning}")
                } else {
                    "Ctrl+E で現在のページを Neovide で開きます。".to_owned()
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
                .child(
                    icon(
                        if source != Source::Server {
                            icons::CLOUD_OFF
                        } else {
                            icons::CHECK_CIRCLE
                        },
                        14.0,
                    )
                    .text_color(if source != Source::Server {
                        rgb(0xe0af68)
                    } else {
                        theme().success
                    }),
                )
                .child(ui_text(
                    if source != Source::Server {
                        source_label(source)
                    } else {
                        "閲覧ビュー · 保存は Neovim"
                    },
                    12.0,
                    theme().text_muted,
                )),
        )
        .child(
            ui_text(
                if source != Source::Server {
                    "ローカルサーバーに接続できません。"
                } else {
                    "競合情報はまだありません。"
                },
                10.5,
                theme().text_disabled,
            )
            .mt(4.0_f32)
            .line_height(LineHeight::Relative(1.45_f32)),
        )
        .child(if source != Source::Server {
            let retry = state.clone();
            icon_text_button(icons::REFRESH, "再試行", 10.5, theme().accent_text)
                .mt(6.0_f32)
                .on_click(move |_| retry.refresh())
        } else {
            div()
        });
    if let Some(error) = &editor.error {
        content = content.child(
            ui_text(error, 10.5, rgb(0xff453a))
                .mt(8.0_f32)
                .line_height(LineHeight::Relative(1.45_f32)),
        );
    }
    if let Some(workspace) = state.active_workspace() {
        let id = workspace.page.id.clone();
        let rev = workspace.page.rev;
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
        content = content.child(divider());
        if armed {
            let confirm = state.clone();
            let cancel = state.clone();
            content = content
                .child(ui_text("削除しますか？元に戻せません。", 11.0, rgb(0xff8f8a)).mb(8.0_f32))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap(6.0_f32)
                        .child(
                            div()
                                .flex_1()
                                .h(28.0_f32)
                                .rounded(5.0_f32)
                                .bg(rgb(0x5a1d1d))
                                .border(1.0_f32)
                                .border_color(rgb(0xff453a))
                                .flex()
                                .items_center()
                                .justify_center()
                                .hover(|style| style.bg(rgb(0x7a2424)))
                                .cursor(CursorIcon::Pointer)
                                .on_click(move |_| {
                                    confirm.delete_content(&id, rev);
                                    confirm.delete_armed.set(false);
                                })
                                .child(ui_text("削除", 11.5, rgb(0xffb4b4))),
                        )
                        .child(
                            div()
                                .flex_1()
                                .h(28.0_f32)
                                .rounded(5.0_f32)
                                .bg(theme().surface_muted)
                                .flex()
                                .items_center()
                                .justify_center()
                                .hover(|style| style.bg(theme().hover))
                                .cursor(CursorIcon::Pointer)
                                .on_click(move |_| cancel.delete_armed.set(false))
                                .child(ui_text("キャンセル", 11.5, theme().text_muted)),
                        ),
                );
        } else {
            let arm = state.clone();
            content = content.child(
                icon_text_button(icons::DELETE, "削除", 11.5, rgb(0xe5484d))
                    .on_click(move |_| arm.delete_armed.set(true)),
            );
        }
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

// Status bar
// ---------------------------------------------------------------------------

/// A short confirmation for actions that otherwise have no visible result, such as copying a
/// wiki link from a context menu.
fn notify(state: &AppState, message: impl Into<String>) {
    let message = message.into();
    state.toast.set(Some(message.clone()));
    let toast = state.toast;
    set_timeout(Duration::from_millis(1900), move || {
        if toast.get_untracked().as_deref() == Some(message.as_str()) {
            toast.set(None);
        }
    });
}

fn toast(state: &AppState) -> Show {
    let state = state.clone();
    show(
        move || state.toast.get().is_some(),
        move || {
            let message = state.toast.get().unwrap_or_default();
            div()
                .absolute()
                .left(0.0_f32)
                .right(0.0_f32)
                .bottom(80.0_f32)
                .z_index(60)
                .flex()
                .justify_center()
                .pointer_events_none()
                .child(
                    div()
                        .px(12.0_f32)
                        .h(32.0_f32)
                        .rounded(8.0_f32)
                        .bg(theme().overlay)
                        .border(1.0_f32)
                        .border_color(theme().border_strong)
                        .shadow_lg()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(7.0_f32)
                        .child(icon(icons::CHECK_CIRCLE, 15.0).text_color(theme().success))
                        .child(ui_text(message, 11.5, theme().text)),
                )
        },
    )
}

fn shortcut_row(label: &str, keys: &str) -> Div {
    div()
        .h(27.0_f32)
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap(18.0_f32)
        .border_b(1.0_f32)
        .border_color(rgba(0xffffff08))
        .child(ui_text(label, 12.0, theme().text_muted))
        .child(kbd(keys))
}

fn shortcut_group(title: &str, rows: &[(&str, &str)]) -> Div {
    let mut group = div()
        .flex()
        .flex_col()
        .child(div().mb(6.0_f32).child(section_label(title)));
    for (label, keys) in rows {
        group = group.child(shortcut_row(label, keys));
    }
    group
}

fn shortcuts_overlay(state: &AppState) -> Show {
    let state = state.clone();
    show(
        move || state.shortcuts.get(),
        move || {
            let close = state.clone();
            div()
                .absolute()
                .left(0.0_f32)
                .right(0.0_f32)
                .top(0.0_f32)
                .bottom(0.0_f32)
                .z_index(45)
                .bg(rgba(0x000000a6))
                .flex()
                .items_center()
                .justify_center()
                .on_click(move |_| close.shortcuts.set(false))
                .child(
                    div()
                        .w(620.0_f32)
                        .p(22.0_f32)
                        .rounded(16.0_f32)
                        .bg(theme().overlay)
                        .border(1.0_f32)
                        .border_color(theme().border_strong)
                        .shadow_lg()
                        .on_click(|event| event.stop_propagation())
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(9.0_f32)
                                .child(icon(icons::KEYBOARD, 20.0).text_color(theme().accent_text))
                                .child(
                                    ui_text("キーボードショートカット", 17.0, theme().text)
                                        .font_weight(FontWeight::SEMIBOLD),
                                )
                                .child(div().flex_1())
                                .child(
                                    div()
                                        .w(28.0_f32)
                                        .h(28.0_f32)
                                        .rounded(7.0_f32)
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .hover(|style| style.bg(theme().surface_muted))
                                        .cursor(CursorIcon::Pointer)
                                        .on_click({
                                            let state = state.clone();
                                            move |_| state.shortcuts.set(false)
                                        })
                                        .child(
                                            icon(icons::CLOSE, 16.0)
                                                .text_color(theme().text_disabled),
                                        ),
                                ),
                        )
                        .child(div().h(18.0_f32))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .gap(32.0_f32)
                                .child(
                                    div()
                                        .flex_1()
                                        .flex()
                                        .flex_col()
                                        .gap(14.0_f32)
                                        .child(shortcut_group(
                                            "移動",
                                            &[
                                                ("検索", "Ctrl+K"),
                                                ("クイックオープン", "Ctrl+P"),
                                                ("戻る", "Ctrl+["),
                                                ("進む", "Ctrl+]"),
                                            ],
                                        ))
                                        .child(shortcut_group(
                                            "ページ",
                                            &[
                                                ("新規ページ", "Ctrl+N"),
                                                ("次のタブ", "Ctrl+Tab"),
                                                ("前のタブ", "Ctrl+Shift+Tab"),
                                                ("タブを閉じる", "Ctrl+W"),
                                                ("再読み込み", "Ctrl+R"),
                                            ],
                                        )),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .flex()
                                        .flex_col()
                                        .gap(14.0_f32)
                                        .child(shortcut_group(
                                            "表示",
                                            &[
                                                ("情報パネル", "Ctrl+I"),
                                                ("サイドバー", "Ctrl+\\"),
                                                ("リンク", "Ctrl+1"),
                                                ("情報タブ", "Ctrl+2"),
                                                ("目次", "Ctrl+3"),
                                                ("2-hop リンク", "Ctrl+Alt+L"),
                                            ],
                                        ))
                                        .child(shortcut_group(
                                            "編集",
                                            &[("Neovide で編集", "Ctrl+E")],
                                        )),
                                ),
                        )
                        .child(
                            div()
                                .mt(18.0_f32)
                                .pt(12.0_f32)
                                .border_t(1.0_f32)
                                .border_color(theme().border)
                                .flex()
                                .justify_between()
                                .child(ui_text(
                                    "入力中は一部のショートカットを無効にします。",
                                    10.0,
                                    theme().text_disabled,
                                ))
                                .child(ui_text("? / Esc で閉じる", 10.0, theme().text_disabled)),
                        ),
                )
        },
    )
}

fn status_bar(state: &AppState) -> Div {
    let editor = state.editor.get();
    let source = state.source.get();
    let error = editor.error.clone();
    let source_label = match source {
        Source::Server => "ローカルサーバー接続中",
        Source::Offline => "オフラインキャッシュ",
        Source::Fallback => "内蔵サンプル",
        Source::Empty => "未接続",
    };
    let open = state.clone();
    let editor_status = div()
        .h(22.0_f32)
        .px(7.0_f32)
        .rounded(5.0_f32)
        .flex()
        .flex_row()
        .items_center()
        .gap(6.0_f32)
        .hover(|style| style.bg(theme().surface_muted))
        .cursor(CursorIcon::Pointer)
        .on_click(move |_| open.open_editor())
        .access(|access| access.label("Neovide で編集 Ctrl+E"))
        .child(icon(icons::EDIT, 13.0).text_color(if editor.attached {
            theme().success
        } else {
            theme().text_disabled
        }))
        .child(ui_text(
            if editor.attached {
                "Neovide 接続済み"
            } else {
                "Neovide 未接続"
            },
            10.0,
            if editor.attached {
                theme().success_text
            } else {
                theme().text_disabled
            },
        ))
        .child(kbd("Ctrl+E"));
    div()
        .h(28.0_f32)
        .px(16.0_f32)
        .z_index(25)
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .bg(theme().page)
        .border_t(1.0_f32)
        .border_color(theme().border)
        .child(
            div()
                .flex_1()
                .min_w(0.0_f32)
                .flex()
                .flex_row()
                .items_center()
                .gap(7.0_f32)
                .child(
                    icon(
                        if source != Source::Server {
                            icons::CLOUD_OFF
                        } else {
                            icons::CHECK_CIRCLE
                        },
                        14.0,
                    )
                    .text_color(if source != Source::Server {
                        rgb(0xe0af68)
                    } else {
                        theme().success
                    }),
                )
                .child(ui_text(source_label, 10.0, theme().text_muted))
                .child(if let Some(error) = error {
                    ui_text(
                        format!("· {}", truncate_chars(&error, 52)),
                        10.0,
                        rgb(0xff6b62),
                    )
                } else {
                    ui_text("", 10.0, theme().text_disabled)
                }),
        )
        .child(tooltip(editor_status, "Neovide で編集 Ctrl+E"))
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
    let mut snapshot_path = None;
    let mut page = None;
    let mut search = false;
    let mut edit = false;
    let mut shelf = false;
    let mut inspector = None;
    let mut tab = None;
    let mut shortcuts = false;
    let mut query_text = None;
    let mut focus_outline = false;
    let mut focus_tabs = false;
    let mut outline = false;
    let mut empty = false;
    let mut width = WINDOW_WIDTH;
    let mut height = WINDOW_HEIGHT;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--snapshot" => snapshot_path = Some(args.next().expect("--snapshot requires a path")),
            "--page" => page = Some(args.next().expect("--page requires a title")),
            "--search" => search = true,
            "--edit" => edit = true,
            "--shelf" => shelf = true,
            "--links" => inspector = Some(Inspector::Links),
            "--info" => inspector = Some(Inspector::Info),
            "--empty" => empty = true,
            "--outline" => outline = true,
            "--query" => query_text = args.next(),
            "--focus-outline" => focus_outline = true,
            "--focus-tabs" => focus_tabs = true,
            "--shortcuts" => shortcuts = true,
            "--tab" => {
                tab = Some(
                    args.next()
                        .expect("--tab requires a number")
                        .parse::<usize>()
                        .expect("--tab must be a number"),
                );
            }
            "--width" => {
                width = args
                    .next()
                    .expect("--width requires a number")
                    .parse()
                    .expect("--width must be a number");
            }
            "--height" => {
                height = args
                    .next()
                    .expect("--height requires a number")
                    .parse()
                    .expect("--height must be a number");
            }
            other => panic!("unknown argument: {other}"),
        }
    }
    let snapshot = snapshot_path.is_some();

    let _ = fonts();
    set_theme(app_theme());
    let state = boot(snapshot, shelf);
    if let Some(index) = tab {
        state.switch_tab(index);
    }
    if shortcuts {
        state.shortcuts.set(true);
    }
    if outline {
        state.outline_open.set(true);
    }
    if focus_outline {
        state.outline_keyboard.set(true);
    }
    if focus_tabs {
        state.switch_tab(1);
        state.tab_focus.set(true);
    }
    if empty {
        state.summaries.set(Vec::new());
        state.tabs.set(Vec::new());
        state.active.set(0);
        state.source.set(Source::Empty);
    }
    if let Some(title) = page
        && let Some(summary) = state
            .summaries
            .get_untracked()
            .into_iter()
            .find(|summary| summary.title == title)
    {
        state.open_summary(&summary);
    }
    if search {
        state.open_palette(PaletteMode::Search);
        if let Some(query) = query_text {
            state.query.set(query);
            state.refresh_results();
        }
    }
    if edit {
        state.open_editor();
    }
    if let Some(mode) = inspector {
        state.inspector.set(mode);
        state.inspector_visible.set(true);
    }

    if let Some(out) = snapshot_path {
        anti_webui::app::headless::render_png(out, width, height, 1.0, move || layout(&state))
            .unwrap();
        return;
    }

    App::new()
        .title("Harmion Wiki")
        .size(width as i32, height as i32)
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

    #[test]
    fn splits_wiki_blocks_out_of_markdown() {
        let body = "# Title\n\nintro\n\n## Section\n\ntext\n\n```rust\nfn main() {}\n```\n\n!```ledger||abc\n```\n\n![[p-home]]\n";
        let segments = split_segments(body, "Title");
        assert!(segments.iter().any(
            |segment| matches!(segment, BodySegment::Heading(2, title) if title == "Section")
        ));
        assert!(segments.iter().any(
            |segment| matches!(segment, BodySegment::Markdown(text) if text.contains("intro"))
        ));
        assert!(segments.iter().any(
            |segment| matches!(segment, BodySegment::Code(language, _) if language == "rust")
        ));
        assert!(
            segments
                .iter()
                .any(|segment| matches!(segment, BodySegment::Embed(id) if id == "abc"))
        );
        assert!(
            segments
                .iter()
                .any(|segment| matches!(segment, BodySegment::Transclude(id) if id == "p-home"))
        );
    }

    #[test]
    fn rewrites_wiki_links_for_markdown_view() {
        assert_eq!(
            wiki_links_to_markdown("[[p-goals|目標]] and [[p-home]]"),
            "[目標](wiki:p-goals) and [p-home](wiki:p-home)"
        );
    }
}
