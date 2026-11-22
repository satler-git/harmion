use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

use anti_webui::prelude::*;
use serde_json::{Value, json};

const DEFAULT_SOCKET: &str = "/tmp/harmion-wiki-m0.sock";
const HOME_ID: &str = "p-home";

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
    after: Vec<String>,
}

impl Snapshot {
    fn fallback() -> Self {
        Self {
            pages: vec!["家計の記録".to_owned(), "2026年の目標".to_owned()],
            snippets: vec!["買い物ジャーナル".to_owned(), "予算クエリ".to_owned()],
            page_title: "家計の記録".to_owned(),
            before: vec!["毎月の支出は hledger のジャーナルを埋め込んで管理している。".to_owned()],
            snippet_title: "買い物ジャーナル".to_owned(),
            snippet_id: "3f2a9c1e00000000000000000000000000000000000000000000000000000001"
                .to_owned(),
            snippet_meta: "hledger / ledger / kinds: wiki, ledger, free".to_owned(),
            snippet_lines: vec![
                "2026-11-16 * 買い物".to_owned(),
                "    支出:食費   1,200 JPY".to_owned(),
                "    資産:現金  -1,200 JPY".to_owned(),
            ],
            after: vec!["残高 ¥12,400（ledger/balance）".to_owned()],
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
    let after = clean_lines(&body[block.end..]);

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
        format!("{format} / {language} / kinds: {kinds}")
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
        snippet_id: snippet_id.clone(),
        snippet_meta,
        snippet_lines,
        after,
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

fn header() -> Div {
    div()
        .px(18.0_f32)
        .py(10.0_f32)
        .bg(theme().surface)
        .border_b(1.0_f32)
        .border_color(theme().border)
        .child(
            text("Harmion Wiki — M0")
                .text_size(15.0_f32)
                .text_color(theme().text),
        )
}

fn text_lines(lines: &[String], muted: bool) -> Div {
    let mut column = div().flex().flex_col().gap(8.0_f32);
    for line in lines {
        let mut label = text(line.clone());
        if muted {
            label = label.text_color(theme().text_muted);
        }
        column = column.child(label);
    }
    column
}

fn sidebar(snapshot: &Snapshot) -> Div {
    let mut column = div()
        .w(220.0)
        .h_full()
        .p(14.0_f32)
        .bg(theme().surface)
        .text_color(theme().text)
        .border_r(1.0_f32)
        .border_color(theme().border)
        .flex()
        .flex_col()
        .gap(6.0_f32)
        .child(
            text("ページ")
                .text_size(11.0_f32)
                .text_color(theme().text_muted),
        );
    for page in &snapshot.pages {
        column = column.child(text(page.clone()));
    }
    column = column.child(
        text("スニペット")
            .text_size(11.0_f32)
            .text_color(theme().text_muted),
    );
    for snippet in &snapshot.snippets {
        column = column.child(text(snippet.clone()));
    }
    column
}

fn article(snapshot: &Snapshot) -> Div {
    let mut column = div()
        .flex_1()
        .h_full()
        .p(28.0_f32)
        .bg(theme().page)
        .text_color(theme().text)
        .flex()
        .flex_col()
        .gap(12.0_f32)
        .child(
            text(snapshot.page_title.clone())
                .text_size(30.0_f32)
                .text_color(theme().text),
        )
        .child(text_lines(&snapshot.before, true));

    let mut embed = div()
        .p(14.0_f32)
        .rounded(8.0_f32)
        .bg(theme().surface)
        .text_color(theme().text)
        .border(1.0_f32)
        .border_color(theme().accent)
        .flex()
        .flex_col()
        .gap(4.0_f32)
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    text(snapshot.snippet_title.clone())
                        .text_size(13.0_f32)
                        .text_color(theme().text),
                )
                .child(button("Neovide で開く").on_click({
                    let id = snapshot.snippet_id.clone();
                    move |_| open_in_nvim(&id)
                })),
        )
        .child(
            text(snapshot.snippet_meta.clone())
                .text_size(11.0_f32)
                .text_color(theme().accent_text),
        );
    for line in &snapshot.snippet_lines {
        embed = embed.child(text(line.clone()));
    }
    column = column
        .child(embed)
        .child(text_lines(&snapshot.after, false));
    column
}

fn rail() -> Div {
    div()
        .w(200.0)
        .h_full()
        .p(14.0_f32)
        .bg(theme().surface)
        .text_color(theme().text)
        .border_l(1.0_f32)
        .border_color(theme().border)
        .flex()
        .flex_col()
        .gap(6.0_f32)
        .child(
            text("目次")
                .text_size(11.0_f32)
                .text_color(theme().text_muted),
        )
        .child(text("今月の支出"))
        .child(text("埋め込みジャーナル"))
        .child(text("残高"))
        .child(
            text("Interpreter")
                .text_size(11.0_f32)
                .text_color(theme().text_muted),
        )
        .child(text("ledger/balance ●"))
        .child(text("wiki/backlinks ●"))
        .child(text("wiki/outline ●"))
        .child(text("wiki/transclude ●"))
}

fn layout(snapshot: &Snapshot) -> Div {
    div()
        .w_full()
        .h_full()
        .flex()
        .flex_col()
        .bg(theme().page)
        .text_color(theme().text)
        .child(header())
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

    set_theme(Theme::dark());
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
