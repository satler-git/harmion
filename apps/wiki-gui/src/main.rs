use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::OnceLock;

use anti_webui::prelude::*;
use serde_json::{Value, json};

const DEFAULT_SOCKET: &str = "/tmp/harmion-wiki-m0.sock";
const HOME_ID: &str = "p-home";
const JOURNAL_ID: &str = "3f2a9c1e00000000000000000000000000000000000000000000000000000001";

mod bundled {
    include!(concat!(env!("OUT_DIR"), "/fonts.rs"));
}

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
        page: rgb(0x12100e),
        surface: rgb(0x181512),
        surface_muted: rgb(0x201c18),
        overlay: rgb(0x241f1a),
        hover: rgb(0x2a251f),
        pressed: rgb(0x332c25),
        selected: rgb(0x2a2119),
        text: rgb(0xeee7dc),
        text_muted: rgb(0x9c9183),
        text_disabled: rgb(0x665e54),
        on_accent: rgb(0x1a120d),
        border: rgb(0x2e2822),
        border_strong: rgb(0x4a4036),
        accent: rgb(0xd08a5a),
        accent_hover: rgb(0xe09a68),
        accent_pressed: rgb(0xb97a4f),
        accent_text: rgb(0xe0a273),
        accent_muted: rgb(0x3a2a1f),
        success: rgb(0x8fae8b),
        success_muted: rgb(0x1f2a1f),
        success_text: rgb(0x9fc49a),
        ..Theme::dark()
    }
}

#[derive(Clone)]
struct Snapshot {
    pages: Vec<String>,
    snippets: Vec<String>,
    page_title: String,
    before: Vec<String>,
    snippet_title: String,
    snippet_id: String,
    snippet_meta: String,
    snippet_lines: Vec<String>,
}

impl Snapshot {
    fn fallback() -> Self {
        Self {
            pages: vec!["家計の記録".to_owned(), "2026年の目標".to_owned()],
            snippets: vec!["買い物ジャーナル".to_owned(), "予算クエリ".to_owned()],
            page_title: "家計の記録".to_owned(),
            before: vec!["毎月の支出を hledger のジャーナルで管理している。".to_owned()],
            snippet_title: "買い物ジャーナル".to_owned(),
            snippet_id: JOURNAL_ID.to_owned(),
            snippet_meta: "HLEDGER / ledger / ledger, free".to_owned(),
            snippet_lines: vec![
                "2026-11-16 * 買い物".to_owned(),
                "    支出:食費   1,200 JPY".to_owned(),
                "    資産:現金  -1,200 JPY".to_owned(),
                "2026-11-17 * 電車".to_owned(),
                "    支出:交通     420 JPY".to_owned(),
                "    資産:現金    -420 JPY".to_owned(),
            ],
        }
    }
}

fn rpc(method: &str, params: Value) -> Result<Value, String> {
    let path = std::env::var("WIKI_SOCKET").unwrap_or_else(|_| DEFAULT_SOCKET.to_owned());
    let mut stream =
        UnixStream::connect(&path).map_err(|error| format!("connect {path}: {error}"))?;
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
    let response: Value = serde_json::from_str(&line).map_err(|error| error.to_string())?;
    if let Some(error) = response.get("error") {
        return Err(error.to_string());
    }
    Ok(response.get("result").cloned().unwrap_or(Value::Null))
}

fn clean_lines(text: &str) -> Vec<String> {
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

fn load_from_server() -> Result<Snapshot, String> {
    let list = rpc("list", json!({}))?;
    let items = list
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| "list result has no items".to_owned())?;
    let mut pages = Vec::new();
    let mut snippets = Vec::new();
    for item in items {
        let Some(title) = item.get("title").and_then(Value::as_str) else {
            continue;
        };
        match item.get("type").and_then(Value::as_str) {
            Some("page") => pages.push(title.to_owned()),
            Some("snippet") => snippets.push(title.to_owned()),
            _ => {}
        }
    }

    let page = rpc("open", json!({ "id": HOME_ID }))?;
    let page_title = page
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("家計の記録")
        .to_owned();
    let body = page.get("body").and_then(Value::as_str).unwrap_or_default();
    let block = harmion_wiki_core::scan_embedded_blocks(body)
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|block| block.spec.id.is_some())
        .ok_or_else(|| "page has no canonical embedded block".to_owned())?;
    let snippet_id = block.spec.id.clone().expect("filtered on id");
    let before = clean_lines(&body[..block.start]);

    let snippet = rpc("open", json!({ "id": snippet_id }))?;
    let snippet_title = snippet
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("snippet")
        .to_owned();
    let format = snippet
        .get("format")
        .and_then(Value::as_str)
        .unwrap_or("text");
    let language = snippet
        .get("language")
        .and_then(Value::as_str)
        .unwrap_or(format);
    let kinds = snippet
        .get("kinds")
        .and_then(Value::as_array)
        .map(|kinds| {
            kinds
                .iter()
                .filter_map(Value::as_str)
                .filter(|kind| *kind != "wiki")
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    let snippet_meta = if kinds.is_empty() {
        format!("{format} / {language}")
    } else {
        format!("{format} / {language} / {kinds}")
    };
    let snippet_lines = clean_lines(
        snippet
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    );

    Ok(Snapshot {
        pages,
        snippets,
        page_title,
        before,
        snippet_title,
        snippet_id,
        snippet_meta,
        snippet_lines,
    })
}

fn load() -> Snapshot {
    match load_from_server() {
        Ok(snapshot) => {
            eprintln!(
                "wiki-gui: loaded from {}",
                std::env::var("WIKI_SOCKET").unwrap_or_else(|_| DEFAULT_SOCKET.to_owned())
            );
            snapshot
        }
        Err(error) => {
            eprintln!("wiki-gui: server unavailable ({error}); using built-in fixture");
            Snapshot::fallback()
        }
    }
}

fn dot(color: Color) -> Div {
    div().w(6.0).h(6.0).rounded(3.0).bg(color)
}

fn chip(label: &str) -> Div {
    div()
        .px(7.0)
        .py(2.0)
        .rounded(4.0)
        .bg(theme().surface_muted)
        .border(1.0)
        .border_color(theme().border)
        .child(
            text(label.to_owned())
                .font_family(sans())
                .text_size(10.0)
                .text_color(theme().text_muted),
        )
}

fn section_label(label: &str) -> Text {
    text(label.to_owned())
        .font_family(sans())
        .text_size(10.0)
        .letter_spacing(0.9)
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme().text_muted)
}

fn nav_row(label: &str, active: bool) -> Div {
    div()
        .px(10.0)
        .py(7.0)
        .rounded(6.0)
        .border_l(2.0)
        .border_color(if active {
            theme().accent
        } else {
            Color::TRANSPARENT
        })
        .bg(if active {
            theme().selected
        } else {
            Color::TRANSPARENT
        })
        .child(
            text(label.to_owned())
                .font_family(sans())
                .text_size(12.5)
                .text_color(if active {
                    theme().text
                } else {
                    theme().text_muted
                }),
        )
}

fn header(snapshot: &Snapshot) -> Div {
    div()
        .h(48.0)
        .px(20.0)
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .bg(theme().surface)
        .border_b(1.0)
        .border_color(theme().border)
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(10.0)
                .child(
                    div()
                        .w(18.0)
                        .h(18.0)
                        .rounded(5.0)
                        .bg(theme().accent)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            text("W")
                                .font_family(sans())
                                .text_size(10.0)
                                .font_weight(FontWeight::BOLD)
                                .text_color(theme().on_accent),
                        ),
                )
                .child(
                    text("Harmion")
                        .font_family(sans())
                        .text_size(13.5)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme().text),
                )
                .child(
                    text("/")
                        .font_family(sans())
                        .text_size(12.0)
                        .text_color(theme().text_disabled),
                )
                .child(
                    text("家計")
                        .font_family(sans())
                        .text_size(12.0)
                        .text_color(theme().text_muted),
                ),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(10.0)
                .child(
                    div()
                        .w(238.0)
                        .h(30.0)
                        .px(10.0)
                        .rounded(6.0)
                        .bg(theme().surface_muted)
                        .border(1.0)
                        .border_color(theme().border)
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(6.0)
                                .child(
                                    text("⌕")
                                        .font_family(sans())
                                        .text_size(12.0)
                                        .text_color(theme().text_muted),
                                )
                                .child(
                                    text("検索")
                                        .font_family(sans())
                                        .text_size(11.5)
                                        .text_color(theme().text_muted),
                                ),
                        )
                        .child(
                            text("⌘K")
                                .font_family(sans())
                                .text_size(10.0)
                                .text_color(theme().text_disabled),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(5.0)
                        .child(dot(theme().success))
                        .child(
                            text("同期済み")
                                .font_family(sans())
                                .text_size(10.5)
                                .text_color(theme().text_muted),
                        ),
                )
                .child(quiet_button("Neovide").on_click({
                    let id = snapshot.snippet_id.clone();
                    move |_| open_in_nvim(&id)
                })),
        )
}

fn sidebar(snapshot: &Snapshot) -> Div {
    let mut sidebar = div()
        .w(222.0)
        .h_full()
        .p(16.0)
        .bg(theme().page)
        .border_r(1.0)
        .border_color(theme().border)
        .flex()
        .flex_col()
        .gap(2.0)
        .child(section_label("ページ"));
    for (index, page) in snapshot.pages.iter().enumerate() {
        sidebar = sidebar.child(nav_row(page, index == 1 || page == "家計の記録"));
    }
    sidebar = sidebar
        .child(div().h(18.0))
        .child(section_label("スニペット"));
    for snippet in &snapshot.snippets {
        sidebar = sidebar.child(
            div()
                .px(10.0)
                .py(6.0)
                .rounded(6.0)
                .flex()
                .items_center()
                .gap(7.0)
                .child(
                    text("▤")
                        .font_family(sans())
                        .text_size(10.0)
                        .text_color(theme().text_disabled),
                )
                .child(
                    text(snippet.clone())
                        .font_family(sans())
                        .text_size(12.0)
                        .text_color(theme().text_muted),
                ),
        );
    }
    sidebar
        .child(div().flex_1())
        .child(
            text("⌘K  検索")
                .font_family(sans())
                .text_size(10.0)
                .text_color(theme().text_disabled),
        )
        .child(
            text("⌘P  クイックオープン")
                .font_family(sans())
                .text_size(10.0)
                .text_color(theme().text_disabled),
        )
}

fn embed(snapshot: &Snapshot) -> Div {
    let mut lines = div().py(8.0).flex().flex_col();
    for (index, line) in snapshot.snippet_lines.iter().enumerate() {
        let trimmed = line.trim_start();
        let color = if trimmed.starts_with("支出") || trimmed.starts_with("資産") {
            theme().text_muted
        } else {
            theme().text
        };
        lines = lines.child(
            div()
                .px(16.0)
                .py(2.0)
                .flex()
                .flex_row()
                .items_start()
                .gap(14.0)
                .child(
                    text(format!("{:>2}", index + 1))
                        .font_family(mono())
                        .text_size(11.0)
                        .text_color(theme().text_disabled),
                )
                .child(
                    text(line.clone())
                        .font_family(mono())
                        .text_size(12.5)
                        .text_color(color),
                ),
        );
    }

    div()
        .mt(22.0)
        .rounded(10.0)
        .bg(theme().surface)
        .border(1.0)
        .border_color(theme().border)
        .overflow_hidden()
        .child(
            div()
                .px(16.0)
                .py(11.0)
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .border_b(1.0)
                .border_color(theme().border)
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(8.0)
                        .child(
                            text("▤")
                                .font_family(sans())
                                .text_size(12.0)
                                .text_color(theme().accent),
                        )
                        .child(
                            text(snapshot.snippet_title.clone())
                                .font_family(sans())
                                .text_size(12.5)
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme().text),
                        )
                        .child(chip("HLEDGER"))
                        .child(chip("ledger, free")),
                )
                .child(quiet_button("Neovide で開く").on_click({
                    let id = snapshot.snippet_id.clone();
                    move |_| open_in_nvim(&id)
                })),
        )
        .child(lines)
        .child(
            div()
                .px(16.0)
                .py(8.0)
                .border_t(1.0)
                .border_color(theme().border)
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    text("3f2a9c1e…  ·  最終更新 14:31")
                        .font_family(mono())
                        .text_size(9.5)
                        .text_color(theme().text_disabled),
                )
                .child(
                    text("snippet")
                        .font_family(sans())
                        .text_size(9.5)
                        .text_color(theme().text_disabled),
                ),
        )
}

fn article(snapshot: &Snapshot) -> Div {
    let mut body = div()
        .w_full()
        .max_w(720.0)
        .px(40.0)
        .py(44.0)
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(7.0)
                .child(
                    text("ノート")
                        .font_family(sans())
                        .text_size(10.5)
                        .text_color(theme().text_muted),
                )
                .child(
                    text("/")
                        .font_family(sans())
                        .text_size(10.5)
                        .text_color(theme().text_disabled),
                )
                .child(
                    text("家計")
                        .font_family(sans())
                        .text_size(10.5)
                        .text_color(theme().text_muted),
                ),
        )
        .child(
            text(snapshot.page_title.clone())
                .mt(14.0)
                .font_family(sans())
                .text_size(36.0)
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme().text),
        )
        .child(
            div()
                .mt(10.0)
                .flex()
                .flex_row()
                .items_center()
                .gap(9.0)
                .child(
                    text("2026年11月")
                        .font_family(sans())
                        .text_size(10.5)
                        .text_color(theme().text_muted),
                )
                .child(dot(theme().text_disabled))
                .child(
                    text("更新 14:31")
                        .font_family(sans())
                        .text_size(10.5)
                        .text_color(theme().text_muted),
                )
                .child(dot(theme().text_disabled))
                .child(
                    text("3 バックリンク")
                        .font_family(sans())
                        .text_size(10.5)
                        .text_color(theme().text_muted),
                ),
        )
        .child(
            div()
                .mt(14.0)
                .flex()
                .flex_row()
                .gap(6.0)
                .child(chip("家計"))
                .child(chip("ledger"))
                .child(chip("毎月")),
        );

    for paragraph in snapshot.before.iter().take(2) {
        body = body.child(
            text(paragraph.clone())
                .mt(24.0)
                .font_family(sans())
                .text_size(15.0)
                .line_height(1.9)
                .text_color(theme().text),
        );
    }

    body = body.child(embed(snapshot));
    body = body
        .child(div().mt(30.0).child(section_label("今月の残高")))
        .child(
            div()
                .mt(10.0)
                .flex()
                .flex_row()
                .items_end()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(
                            text("¥12,400")
                                .font_family(sans())
                                .text_size(34.0)
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme().text),
                        )
                        .child(
                            text("2026-11-16 時点")
                                .mt(4.0)
                                .font_family(sans())
                                .text_size(10.5)
                                .text_color(theme().text_muted),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(6.0)
                        .child(dot(theme().success))
                        .child(
                            text("ledger/balance")
                                .font_family(mono())
                                .text_size(10.0)
                                .text_color(theme().text_muted),
                        ),
                ),
        )
        .child(div().mt(30.0).child(section_label("このページを参照")))
        .child(
            div()
                .mt(10.0)
                .flex()
                .flex_row()
                .flex_wrap()
                .gap(6.0)
                .child(chip("2026年の目標"))
                .child(chip("レシート処理"))
                .child(chip("固定費の見直し")),
        )
        .child(div().h(56.0));

    div()
        .flex_1()
        .h_full()
        .overflow_y_scroll()
        .child(div().w_full().flex().justify_center().child(body))
}

fn rail() -> Div {
    div()
        .w(240.0)
        .h_full()
        .p(18.0)
        .bg(theme().page)
        .border_l(1.0)
        .border_color(theme().border)
        .flex()
        .flex_col()
        .child(section_label("目次"))
        .child(
            div()
                .mt(10.0)
                .pl(10.0)
                .py(4.0)
                .border_l(2.0)
                .border_color(theme().accent)
                .child(
                    text("今月の支出")
                        .font_family(sans())
                        .text_size(12.0)
                        .text_color(theme().text),
                ),
        )
        .child(
            div()
                .pl(10.0)
                .py(4.0)
                .border_l(2.0)
                .border_color(Color::TRANSPARENT)
                .child(
                    text("埋め込みジャーナル")
                        .font_family(sans())
                        .text_size(12.0)
                        .text_color(theme().text_muted),
                ),
        )
        .child(
            div()
                .pl(10.0)
                .py(4.0)
                .border_l(2.0)
                .border_color(Color::TRANSPARENT)
                .child(
                    text("残高")
                        .font_family(sans())
                        .text_size(12.0)
                        .text_color(theme().text_muted),
                ),
        )
        .child(div().h(1.0).my(18.0).bg(theme().border))
        .child(section_label("接続"))
        .child(
            div()
                .mt(10.0)
                .flex()
                .flex_col()
                .gap(9.0)
                .child(provider("ledger/balance"))
                .child(provider("wiki/backlinks"))
                .child(provider("wiki/outline"))
                .child(provider("wiki/transclude")),
        )
        .child(div().h(1.0).my(18.0).bg(theme().border))
        .child(section_label("エディタ"))
        .child(
            div()
                .mt(10.0)
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    text("Neovide")
                        .font_family(sans())
                        .text_size(11.5)
                        .text_color(theme().text_muted),
                )
                .child(
                    text("外部")
                        .font_family(sans())
                        .text_size(9.5)
                        .text_color(theme().text_disabled),
                ),
        )
        .child(div().flex_1())
        .child(
            div()
                .flex()
                .items_center()
                .gap(6.0)
                .child(dot(theme().success))
                .child(
                    text("競合なし")
                        .font_family(sans())
                        .text_size(10.0)
                        .text_color(theme().text_muted),
                ),
        )
}

fn provider(name: &str) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .child(
            div()
                .flex()
                .items_center()
                .gap(7.0)
                .child(dot(theme().success))
                .child(
                    text(name.to_owned())
                        .font_family(mono())
                        .text_size(9.5)
                        .text_color(theme().text_muted),
                ),
        )
        .child(
            text("接続中")
                .font_family(sans())
                .text_size(9.0)
                .text_color(theme().text_disabled),
        )
}

fn status_bar() -> Div {
    div()
        .h(26.0)
        .px(20.0)
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .bg(theme().surface)
        .border_t(1.0)
        .border_color(theme().border)
        .child(
            div()
                .flex()
                .items_center()
                .gap(7.0)
                .child(dot(theme().success))
                .child(
                    text("保存済み  ·  同期済み")
                        .font_family(sans())
                        .text_size(9.5)
                        .text_color(theme().text_muted),
                ),
        )
        .child(
            text("4 providers  ·  家計の記録")
                .font_family(sans())
                .text_size(9.5)
                .text_color(theme().text_disabled),
        )
}

fn layout(snapshot: &Snapshot) -> Div {
    div()
        .w_full()
        .h_full()
        .flex()
        .flex_col()
        .bg(theme().page)
        .text_color(theme().text)
        .child(header(snapshot))
        .child(
            div()
                .flex()
                .flex_row()
                .flex_1()
                .min_h(0.0)
                .child(sidebar(snapshot))
                .child(article(snapshot))
                .child(rail()),
        )
        .child(status_bar())
}

fn open_in_nvim(id: &str) {
    let server =
        std::env::var("WIKI_NVIM_SERVER").unwrap_or_else(|_| "target/nvim.sock".to_owned());
    let result = std::process::Command::new("nvim")
        .args([
            "--server",
            &server,
            "--remote-send",
            &format!(":WikiOpen {id}<CR>"),
        ])
        .status();
    if let Err(error) = result {
        eprintln!("wiki-gui: could not open Neovim at {server}: {error}");
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let snapshot_path = if args.next().as_deref() == Some("--snapshot") {
        Some(args.next().expect("--snapshot requires a path"))
    } else {
        None
    };

    let _ = fonts();
    set_theme(app_theme());
    let snapshot = load();

    if let Some(out) = snapshot_path {
        anti_webui::app::headless::render_png(out, 1440, 900, 1.0, move || layout(&snapshot))
            .unwrap();
        return;
    }

    App::new()
        .title("Harmion Wiki")
        .size(1440, 900)
        .run(move || layout(&snapshot))
        .unwrap();
}
