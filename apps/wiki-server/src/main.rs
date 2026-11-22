use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

use clap::{Parser, Subcommand};
use harmion_wiki_core::{LinkIndex, outline, parse_links, scan_embedded_blocks};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const SOCKET_DEFAULT: &str = "/tmp/harmion-wiki-m0.sock";
const JOURNAL_ID: &str = "3f2a9c1e00000000000000000000000000000000000000000000000000000001";
const BUDGET_ID: &str = "3f2a9c1e00000000000000000000000000000000000000000000000000000002";
const MONTHLY_ID: &str = "3f2a9c1e00000000000000000000000000000000000000000000000000000003";
const CREDIT_ID: &str = "3f2a9c1e00000000000000000000000000000000000000000000000000000004";
const POWER_ID: &str = "3f2a9c1e00000000000000000000000000000000000000000000000000000005";

#[derive(Debug, Parser)]
#[command(name = "wiki-server", about = "Wiki local JSON-RPC server")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Serve {
        #[arg(long, default_value = SOCKET_DEFAULT)]
        socket: PathBuf,
    },
    List {
        #[arg(long, default_value = SOCKET_DEFAULT)]
        socket: PathBuf,
    },
    Open {
        id: String,
        #[arg(long, default_value = SOCKET_DEFAULT)]
        socket: PathBuf,
    },
    Workspace {
        id: String,
        #[arg(long, default_value = SOCKET_DEFAULT)]
        socket: PathBuf,
    },
    Search {
        query: String,
        #[arg(long, default_value = SOCKET_DEFAULT)]
        socket: PathBuf,
    },
    Save {
        id: String,
        #[arg(long)]
        rev: u64,
        #[arg(long, default_value = SOCKET_DEFAULT)]
        socket: PathBuf,
    },
}

#[derive(Debug, Clone, Serialize)]
struct Content {
    id: String,
    #[serde(rename = "type")]
    content_type: String,
    title: String,
    body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<String>,
    kinds: Vec<String>,
    rev: u64,
    created: String,
    updated_at: i64,
    updated_label: String,
}

#[derive(Debug, Clone, Serialize)]
struct Summary {
    id: String,
    #[serde(rename = "type")]
    content_type: String,
    title: String,
    format: Option<String>,
    language: Option<String>,
    kinds: Vec<String>,
    rev: u64,
    updated_label: String,
    excerpt: String,
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
struct OutlineView {
    level: u8,
    title: String,
    line: usize,
}

#[derive(Debug, Clone, Serialize)]
struct TwoHopView {
    via: Summary,
    target: Summary,
    reason: String,
}

#[derive(Debug, Clone, Serialize)]
struct LinksView {
    outgoing: Vec<Summary>,
    backlinks: Vec<Summary>,
    two_hop: Vec<TwoHopView>,
}

#[derive(Debug, Clone, Serialize)]
struct PageInfo {
    created: String,
    updated: String,
    word_count: usize,
    tags: String,
}

#[derive(Debug, Clone, Serialize)]
struct Workspace {
    page: Content,
    before: Vec<String>,
    after: Vec<String>,
    snippet: Option<Content>,
    outline: Vec<OutlineView>,
    links: LinksView,
    related: Vec<Summary>,
    recent: Vec<Summary>,
    info: PageInfo,
}

#[derive(Debug)]
struct State {
    contents: std::collections::BTreeMap<String, Content>,
}

impl State {
    fn fixtures() -> Self {
        let mut contents = std::collections::BTreeMap::new();
        let mut add = |content: Content| {
            contents.insert(content.id.clone(), content);
        };

        add(Content {
            id: "p-home".to_owned(),
            content_type: "page".to_owned(),
            title: "家計の記録".to_owned(),
            body: format!(
                "# 家計の記録\n\n毎月の支出を hledger のジャーナルで管理している。\n\n## 買い物ジャーナル\n\n!```hledger|ledger,free||{JOURNAL_ID}\n```\n\n## 今月の残高\n\n固定費の見直しは月1回、日曜日にまとめて行う。\n\n## 固定費の見直し\n\n関連: [[p-goals|2026年の目標]] [[p-receipts|レシート処理]] [[p-fixed|固定費の見直し]]\n"
            ),
            format: None,
            language: None,
            kinds: vec!["wiki".to_owned(), "ledger".to_owned()],
            rev: 1,
            created: "2026-10-02".to_owned(),
            updated_at: 202611221431,
            updated_label: "14:31".to_owned(),
        });
        add(Content {
            id: "p-goals".to_owned(),
            content_type: "page".to_owned(),
            title: "2026年の目標".to_owned(),
            body: "# 2026年の目標\n\n家計の記録を毎週見直す。\n\n## 固定費\n\n- [[p-home|家計の記録]]\n- [[p-fixed|固定費の見直し]]\n- [[p-review|月次レビュー]]\n- [[p-reading|読書ノート]]\n- [[p-savings|貯金の目標]]\n".to_owned(),
            format: None,
            language: None,
            kinds: vec!["wiki".to_owned(), "ledger".to_owned()],
            rev: 1,
            created: "2026-10-03".to_owned(),
            updated_at: 202611211800,
            updated_label: "昨日".to_owned(),
        });
        add(Content {
            id: "p-receipts".to_owned(),
            content_type: "page".to_owned(),
            title: "レシート処理".to_owned(),
            body: "# レシート処理\n\nレシートは週末にまとめて処理する。\n\n## 支払い方法\n\n- [[p-home|家計の記録]]\n- [[p-fixed|固定費の見直し]]\n- [[p-credit|クレジットカード]]\n- [[p-emoney|電子マネー]]\n- [[p-returns|返品・交換]]\n".to_owned(),
            format: None,
            language: None,
            kinds: vec!["wiki".to_owned(), "ledger".to_owned()],
            rev: 1,
            created: "2026-10-05".to_owned(),
            updated_at: 202611201200,
            updated_label: "11/20".to_owned(),
        });
        add(Content {
            id: "p-fixed".to_owned(),
            content_type: "page".to_owned(),
            title: "固定費の見直し".to_owned(),
            body: "# 固定費の見直し\n\n毎月の固定費を一覧にする。\n\n## 内訳\n\n- [[p-home|家計の記録]]\n- [[p-goals|2026年の目標]]\n- [[p-electricity|電気代]]\n- [[p-gas|ガス代]]\n- [[p-rent|家賃]]\n- [[p-phone|通信費]]\n- [[p-sub|サブスク]]\n".to_owned(),
            format: None,
            language: None,
            kinds: vec!["wiki".to_owned(), "ledger".to_owned()],
            rev: 1,
            created: "2026-10-06".to_owned(),
            updated_at: 202611191100,
            updated_label: "11/19".to_owned(),
        });
        add(Content {
            id: "p-review".to_owned(),
            content_type: "page".to_owned(),
            title: "月次レビュー".to_owned(),
            body: "# 月次レビュー\n\n1か月の支出を振り返り、翌月の予算を決める。\n\n- [[p-home|家計の記録]]\n- [[p-goals|2026年の目標]]\n".to_owned(),
            format: None,
            language: None,
            kinds: vec!["wiki".to_owned(), "ledger".to_owned()],
            rev: 1,
            created: "2026-10-10".to_owned(),
            updated_at: 202611201900,
            updated_label: "11/20".to_owned(),
        });
        add(Content {
            id: "p-reading".to_owned(),
            content_type: "page".to_owned(),
            title: "読書ノート".to_owned(),
            body: "# 読書ノート\n\n読んだ本の要点を残す。\n\n## 最近の本\n\n- [[p-goals|2026年の目標]]\n- [[p-rust|Rustのメモ]]\n".to_owned(),
            format: None,
            language: None,
            kinds: vec!["wiki".to_owned(), "note".to_owned()],
            rev: 1,
            created: "2026-10-12".to_owned(),
            updated_at: 202611182000,
            updated_label: "昨日".to_owned(),
        });
        add(Content {
            id: "p-rust".to_owned(),
            content_type: "page".to_owned(),
            title: "Rustのメモ".to_owned(),
            body: "# Rustのメモ\n\n所有権と借用のメモ。\n\n```rust\nfn main() {\n    let s = String::from(\"hello\");\n    println!(\"{s}\");\n}\n```\n\n- [[p-reading|読書ノート]]\n".to_owned(),
            format: None,
            language: None,
            kinds: vec!["wiki".to_owned(), "code".to_owned()],
            rev: 1,
            created: "2026-10-15".to_owned(),
            updated_at: 202611171500,
            updated_label: "11/17".to_owned(),
        });
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
            add(Content {
                id: id.to_owned(),
                content_type: "page".to_owned(),
                title: title.to_owned(),
                body: format!(
                    "# {title}\n\n{title}のメモ。\n\n- [[{parent}|関連ページ]]\n- [[p-home|家計の記録]]\n"
                ),
                format: None,
                language: None,
                kinds: vec!["wiki".to_owned(), "ledger".to_owned()],
                rev: 1,
                created: "2026-10-20".to_owned(),
                updated_at,
                updated_label: updated_label.to_owned(),
            });
        }
        add(Content {
            id: JOURNAL_ID.to_owned(),
            content_type: "snippet".to_owned(),
            title: "買い物ジャーナル".to_owned(),
            body: "2026-11-16 * 買い物\n    支出:食費   1,200 JPY\n    資産:現金  -1,200 JPY\n\n2026-11-17 * 電車\n    支出:交通     420 JPY\n    資産:現金    -420 JPY\n".to_owned(),
            format: Some("hledger".to_owned()),
            language: Some("ledger".to_owned()),
            kinds: vec!["wiki".to_owned(), "ledger".to_owned(), "free".to_owned()],
            rev: 1,
            created: "2026-10-02".to_owned(),
            updated_at: 202611221431,
            updated_label: "14:31".to_owned(),
        });
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
            add(Content {
                id: id.to_owned(),
                content_type: "snippet".to_owned(),
                title: title.to_owned(),
                body: body.to_owned(),
                format: Some("hledger".to_owned()),
                language: Some("ledger".to_owned()),
                kinds: kinds.into_iter().map(str::to_owned).collect(),
                rev: 1,
                created: "2026-10-02".to_owned(),
                updated_at,
                updated_label: updated_label.to_owned(),
            });
        }
        Self { contents }
    }

    fn resolve(&self, target: &str) -> Option<&Content> {
        self.contents.get(target).or_else(|| {
            self.contents
                .values()
                .find(|content| content.title == target)
        })
    }

    fn workspace(&self, id: &str) -> Option<Workspace> {
        let page = self.contents.get(id)?.clone();
        let block = scan_embedded_blocks(&page.body)
            .ok()?
            .into_iter()
            .find(|block| block.spec.id.is_some());
        let (before, after, snippet) = match block {
            Some(block) => {
                let before = clean_lines(&page.body[..block.start]);
                let after = clean_lines(&page.body[block.end..]);
                let snippet = block
                    .spec
                    .id
                    .as_deref()
                    .and_then(|id| self.contents.get(id))
                    .cloned();
                (before, after, snippet)
            }
            None => (clean_lines(&page.body), Vec::new(), None),
        };

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
            .map(|item| OutlineView {
                level: item.level,
                title: item.title,
                line: item.line,
            })
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
        recent.sort_by(|a, b| b.updated_label.cmp(&a.updated_label));
        recent.truncate(8);

        let tags = page
            .kinds
            .iter()
            .filter(|kind| kind.as_str() != "wiki")
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        Some(Workspace {
            before,
            after,
            snippet,
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

    fn search(&self, query: &str) -> Vec<Summary> {
        let query = query.to_lowercase();
        self.contents
            .values()
            .filter(|content| {
                content.title.to_lowercase().contains(&query)
                    || content.body.to_lowercase().contains(&query)
            })
            .map(Summary::from)
            .take(20)
            .collect()
    }
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

fn excerpt(body: &str) -> String {
    let line = clean_lines(body).into_iter().next().unwrap_or_default();
    let mut chars = line.chars();
    let excerpt = chars.by_ref().take(80).collect::<String>();
    if chars.next().is_some() {
        format!("{excerpt}…")
    } else {
        excerpt
    }
}

#[derive(Debug, Deserialize)]
struct Request {
    id: u64,
    method: String,
    #[serde(default)]
    params: Value,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    match Cli::parse().command {
        Command::Serve { socket } => serve(&socket),
        Command::List { socket } => print_call(&socket, "list", json!({})),
        Command::Open { id, socket } => print_call(&socket, "open", json!({ "id": id })),
        Command::Workspace { id, socket } => print_call(&socket, "workspace", json!({ "id": id })),
        Command::Search { query, socket } => {
            print_call(&socket, "search", json!({ "query": query }))
        }
        Command::Save { id, rev, socket } => {
            let mut body = String::new();
            std::io::stdin().read_to_string(&mut body)?;
            print_call(
                &socket,
                "save",
                json!({ "id": id, "rev": rev, "body": body }),
            )
        }
    }
}

fn print_call(
    socket: &Path,
    method: &str,
    params: Value,
) -> Result<(), Box<dyn std::error::Error>> {
    let value = call(socket, method, params)?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn serve(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let _ = fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    eprintln!("wiki-server listening on {}", path.display());
    let state = Arc::new(Mutex::new(State::fixtures()));

    for stream in listener.incoming() {
        let stream = match stream {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!("accept failed: {error}");
                continue;
            }
        };
        let state = Arc::clone(&state);
        thread::spawn(move || {
            if let Err(error) = handle_client(stream, state) {
                eprintln!("client failed: {error}");
            }
        });
    }
    Ok(())
}

fn handle_client(mut stream: UnixStream, state: Arc<Mutex<State>>) -> std::io::Result<()> {
    let reader = BufReader::new(stream.try_clone()?);
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(request) => dispatch(&state, request),
            Err(error) => json!({
                "id": null,
                "error": { "code": 400, "message": format!("invalid request: {error}") }
            }),
        };
        let mut bytes = serde_json::to_vec(&response).expect("response serializes");
        bytes.push(b'\n');
        stream.write_all(&bytes)?;
        stream.flush()?;
    }
    Ok(())
}

fn dispatch(state: &Arc<Mutex<State>>, request: Request) -> Value {
    let result: Result<Value, (u16, String)> = match request.method.as_str() {
        "list" => {
            let state = state.lock().expect("state lock");
            let mut items = state
                .contents
                .values()
                .map(Summary::from)
                .collect::<Vec<_>>();
            items.sort_by(|a, b| b.updated_label.cmp(&a.updated_label));
            Ok(json!({ "items": items }))
        }
        "open" => {
            let id = request.params.get("id").and_then(Value::as_str);
            match id.and_then(|id| state.lock().expect("state lock").contents.get(id).cloned()) {
                Some(content) => Ok(serde_json::to_value(content).expect("content serializes")),
                None => Err((404, "content not found".to_owned())),
            }
        }
        "workspace" => {
            let id = request.params.get("id").and_then(Value::as_str);
            match id.and_then(|id| state.lock().expect("state lock").workspace(id)) {
                Some(workspace) => {
                    Ok(serde_json::to_value(workspace).expect("workspace serializes"))
                }
                None => Err((404, "content not found".to_owned())),
            }
        }
        "search" => {
            let query = request
                .params
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or("");
            let state = state.lock().expect("state lock");
            Ok(json!({ "items": state.search(query) }))
        }
        "save" => {
            let id = request.params.get("id").and_then(Value::as_str);
            let rev = request.params.get("rev").and_then(Value::as_u64);
            let body = request.params.get("body").and_then(Value::as_str);
            match (id, rev, body) {
                (Some(id), Some(rev), Some(body)) => {
                    let mut state = state.lock().expect("state lock");
                    match state.contents.get_mut(id) {
                        Some(content) if content.rev == rev => {
                            content.body = body.to_owned();
                            content.rev += 1;
                            content.updated_label = "たった今".to_owned();
                            content.updated_at += 1;
                            Ok(serde_json::to_value(content.clone()).expect("content serializes"))
                        }
                        Some(_) => Err((409, "revision mismatch".to_owned())),
                        None => Err((404, "content not found".to_owned())),
                    }
                }
                _ => Err((400, "save requires id, rev and body".to_owned())),
            }
        }
        other => Err((404, format!("unknown method {other}"))),
    };

    match result {
        Ok(result) => json!({ "id": request.id, "result": result }),
        Err((code, message)) => json!({
            "id": request.id,
            "error": { "code": code, "message": message }
        }),
    }
}

fn call(socket: &Path, method: &str, params: Value) -> Result<Value, Box<dyn std::error::Error>> {
    let mut stream = UnixStream::connect(socket)?;
    let request = json!({ "id": 1, "method": method, "params": params });
    let mut bytes = serde_json::to_vec(&request)?;
    bytes.push(b'\n');
    stream.write_all(&bytes)?;
    stream.flush()?;

    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    let response: Value = serde_json::from_str(&line)?;
    if let Some(error) = response.get("error") {
        return Err(format!("server error: {error}").into());
    }
    Ok(response.get("result").cloned().unwrap_or(Value::Null))
}
