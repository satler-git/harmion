use std::io::Cursor;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::LspServer;
use crate::completion;
use crate::framing::{read_message, write_message};
use crate::server::ServerClient;
use crate::test_support::{
    DUP_A_ID, DUP_TITLE, FakeServer, GOALS_ID, GOALS_TITLE, MISSING_ID, PAGE_ID, RECEIPTS_ID,
    SNIPPET_ID, SNIPPET_TITLE,
};
use crate::text;
use crate::uri::wiki_uri;

fn frame(messages: &[Value]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for message in messages {
        write_message(&mut bytes, message).expect("frame message");
    }
    bytes
}

fn run_lsp(messages: Vec<Value>, socket: PathBuf) -> (i32, Vec<Value>) {
    let input = frame(&messages);
    let mut output = Vec::new();
    let server = LspServer::new(Cursor::new(input), &mut output, socket);
    let (code, writer) = server.run().expect("run lsp");
    drop(writer);

    let mut reader = Cursor::new(output);
    let mut frames = Vec::new();
    while let Some(message) = read_message(&mut reader).expect("read framed response") {
        frames.push(message);
    }
    (code, frames)
}

fn initialize_message(socket: Option<&Path>) -> Value {
    let mut params = json!({ "processId": null, "capabilities": {} });
    if let Some(socket) = socket {
        params["initializationOptions"] = json!({ "socket": socket.to_string_lossy() });
    }
    json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": params })
}

fn did_open(uri: &str, text: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": uri,
                "languageId": "markdown",
                "version": 1,
                "text": text
            }
        }
    })
}

fn request(id: u64, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

fn shutdown_and_exit() -> Vec<Value> {
    vec![
        request(90, "shutdown", json!(null)),
        json!({ "jsonrpc": "2.0", "method": "exit" }),
    ]
}

fn response<'a>(frames: &'a [Value], id: u64) -> &'a Value {
    frames
        .iter()
        .find(|message| message.get("id").and_then(Value::as_u64) == Some(id))
        .unwrap_or_else(|| panic!("missing response for id {id}: {frames:#?}"))
}

fn diagnostics_for<'a>(frames: &'a [Value], uri: &str) -> &'a [Value] {
    frames
        .iter()
        .rev()
        .filter(|message| message["method"] == "textDocument/publishDiagnostics")
        .find(|message| message["params"]["uri"] == uri)
        .and_then(|message| message["params"]["diagnostics"].as_array())
        .map(Vec::as_slice)
        .unwrap_or_else(|| panic!("missing diagnostics for {uri}: {frames:#?}"))
}

fn codes(diagnostics: &[Value]) -> Vec<&str> {
    diagnostics
        .iter()
        .filter_map(|diagnostic| diagnostic["code"].as_str())
        .collect()
}

fn position_of(text: &str, needle: &str) -> Value {
    let byte = text.find(needle).expect("needle is in document") + 1;
    let position = text::position_at(text, byte);
    json!({ "line": position.line, "character": position.character })
}

#[test]
fn server_client_reads_list_search_open_and_backlinks() {
    let fake = FakeServer::start();
    let client = ServerClient::new(fake.path());

    let items = client.list().expect("list");
    assert!(items.iter().any(|content| content.id == PAGE_ID));

    let resolved = client
        .resolve_target(GOALS_TITLE)
        .expect("search by title")
        .expect("title resolves");
    assert_eq!(resolved.id, GOALS_ID);

    let opened = client.open(SNIPPET_ID).expect("open by id");
    assert_eq!(opened.title, SNIPPET_TITLE);
    assert_eq!(opened.language.as_deref(), Some("ledger"));

    let backlinks = client
        .workspace_backlinks(&GOALS_ID)
        .expect("workspace backlinks");
    assert!(
        backlinks.iter().any(|content| content.id == PAGE_ID),
        "p-home should backlink p-goals: {backlinks:#?}"
    );
}

#[test]
fn completion_offers_titles_providers_presets_and_kinds() {
    let fake = FakeServer::start();
    let client = ServerClient::new(fake.path());
    let index = client.list().expect("list");

    let complete = |document: &str| {
        completion::complete(
            document,
            text::position_at(document, document.len()),
            &index,
        )
        .into_iter()
        .map(|item| item.label)
        .collect::<Vec<_>>()
    };

    // `[[` after Japanese text: the UTF-16 position conversion must still land
    // inside the open link.
    assert!(complete("日本語 [[家").contains(&"家計の記録".to_owned()));
    assert!(complete("関連 ![[買").contains(&SNIPPET_TITLE.to_owned()));
    assert!(complete("[[2026").contains(&GOALS_TITLE.to_owned()));

    assert!(complete("![led").contains(&"ledger/balance".to_owned()));
    assert!(complete("![wiki/").contains(&"wiki/backlinks".to_owned()));

    assert!(complete("!```h").contains(&"hledger".to_owned()));
    assert!(complete("!```ledger|le").contains(&"ledger".to_owned()));
    assert!(!complete("!```ledger|le").contains(&"free".to_owned()));

    assert!(
        complete("[[target|lab").is_empty(),
        "labels are free text, not titles"
    );
}

#[test]
fn diagnostics_report_server_backed_errors() {
    let fake = FakeServer::start();
    let document = format!(
        "# 家計の記録\n\n[[{GOALS_TITLE}]] [[no-such-page]] ![[missing-embed]]\n\n!```ledger||{MISSING_ID}\n```\n"
    );
    let mut messages = vec![
        initialize_message(Some(fake.path())),
        did_open("wiki://p-home", &document),
    ];
    messages.extend(shutdown_and_exit());

    let (code, frames) = run_lsp(messages, fake.path().to_path_buf());
    assert_eq!(code, 0);
    let diagnostics = diagnostics_for(&frames, "wiki://p-home");
    let codes = codes(diagnostics);
    assert!(codes.contains(&"unresolved-link"), "{diagnostics:#?}");
    assert!(codes.contains(&"missing-snippet"), "{diagnostics:#?}");
    assert_eq!(
        codes
            .iter()
            .filter(|code| **code == "unresolved-link")
            .count(),
        2,
        "both the plain and embedded unresolved links should be reported: {diagnostics:#?}"
    );
}

#[test]
fn diagnostics_report_duplicate_titles() {
    let fake = FakeServer::start();
    let document = format!("# {DUP_TITLE}\n\n重複している。\n");
    let mut messages = vec![
        initialize_message(Some(fake.path())),
        did_open(&format!("wiki://{DUP_A_ID}"), &document),
    ];
    messages.extend(shutdown_and_exit());

    let (code, frames) = run_lsp(messages, fake.path().to_path_buf());
    assert_eq!(code, 0);
    let diagnostics = diagnostics_for(&frames, &format!("wiki://{DUP_A_ID}"));
    assert!(
        codes(diagnostics).contains(&"duplicate-title"),
        "{diagnostics:#?}"
    );
}

#[test]
fn diagnostics_and_navigation_degrade_without_server() {
    let absent = std::env::temp_dir().join(format!(
        "harmion-wiki-lsp-absent-{}-{}.sock",
        std::process::id(),
        12345
    ));
    let _ = std::fs::remove_file(&absent);

    let document = "!```ledger\nunclosed body\n\n[[somewhere]]\n";
    let mut messages = vec![did_open("wiki://p-home", document)];
    let position = position_of(document, "somewhere");
    messages.push(request(
        2,
        "textDocument/definition",
        json!({ "textDocument": { "uri": "wiki://p-home" }, "position": position }),
    ));
    messages.push(request(
        3,
        "textDocument/references",
        json!({ "textDocument": { "uri": "wiki://p-home" }, "position": { "line": 0, "character": 0 } }),
    ));
    messages.insert(0, initialize_message(Some(&absent)));
    messages.extend(shutdown_and_exit());

    let (code, frames) = run_lsp(messages, absent);
    assert_eq!(code, 0);
    let diagnostics = diagnostics_for(&frames, "wiki://p-home");
    let codes = codes(diagnostics);
    assert!(codes.contains(&"invalid-fence"), "{diagnostics:#?}");
    assert!(!codes.contains(&"unresolved-link"), "{diagnostics:#?}");
    assert_eq!(response(&frames, 2)["result"], json!([]));
    assert_eq!(response(&frames, 3)["result"], json!([]));
}

#[test]
fn definition_references_hover_and_completion_use_the_server_index() {
    let fake = FakeServer::start();
    let document = format!("# 家計\n\n[[{GOALS_TITLE}]]\n");
    let position = position_of(&document, GOALS_TITLE);
    let mut messages = vec![
        initialize_message(Some(fake.path())),
        did_open("wiki://p-home", &document),
    ];
    messages.push(request(
        2,
        "textDocument/definition",
        json!({ "textDocument": { "uri": "wiki://p-home" }, "position": position }),
    ));
    messages.push(request(
        3,
        "textDocument/references",
        json!({ "textDocument": { "uri": "wiki://p-home" }, "position": position }),
    ));
    messages.push(request(
        4,
        "textDocument/references",
        json!({ "textDocument": { "uri": "wiki://p-home" }, "position": { "line": 0, "character": 0 } }),
    ));
    messages.push(request(
        5,
        "textDocument/hover",
        json!({ "textDocument": { "uri": "wiki://p-home" }, "position": position }),
    ));
    messages.push(request(
        6,
        "textDocument/completion",
        json!({ "textDocument": { "uri": "wiki://p-home" }, "position": { "line": 2, "character": 2 } }),
    ));
    messages.extend(shutdown_and_exit());

    let (code, frames) = run_lsp(messages, fake.path().to_path_buf());
    assert_eq!(code, 0);

    let definition = response(&frames, 2);
    assert_eq!(definition["result"][0]["uri"], wiki_uri(GOALS_ID));

    let target_backlinks = response(&frames, 3);
    assert!(
        target_backlinks["result"]
            .as_array()
            .expect("reference array")
            .iter()
            .any(|location| location["uri"] == wiki_uri(PAGE_ID)),
        "{target_backlinks:#?}"
    );

    let page_backlinks = response(&frames, 4);
    let uris = page_backlinks["result"]
        .as_array()
        .expect("reference array")
        .iter()
        .filter_map(|location| location["uri"].as_str())
        .collect::<Vec<_>>();
    assert!(uris.contains(&wiki_uri(GOALS_ID).as_str()), "{uris:?}");
    assert!(uris.contains(&wiki_uri(RECEIPTS_ID).as_str()), "{uris:?}");

    let hover = response(&frames, 5);
    let markdown = hover["result"]["contents"]["value"]
        .as_str()
        .expect("markdown hover");
    assert!(markdown.contains(GOALS_TITLE), "{markdown}");
    assert!(markdown.contains("**type** `page`"), "{markdown}");
    assert!(markdown.contains("家計の記録を毎週見直す"), "{markdown}");

    let completion = response(&frames, 6);
    let labels = completion["result"]["items"]
        .as_array()
        .expect("completion items")
        .iter()
        .filter_map(|item| item["label"].as_str())
        .collect::<Vec<_>>();
    assert!(labels.contains(&GOALS_TITLE), "{labels:?}");
    assert!(labels.contains(&"家計の記録"), "{labels:?}");
}

#[test]
fn hover_reports_snippet_metadata_and_provider_schema() {
    let fake = FakeServer::start();
    let document = format!(
        "# 家計\n\n!```hledger|ledger,free||{SNIPPET_ID}\n```\n\n![ledger/balance(query(before: 2026-11-16))]\n"
    );
    let mut messages = vec![
        initialize_message(Some(fake.path())),
        did_open("wiki://p-home", &document),
    ];
    let fence_line = document
        .lines()
        .position(|line| line.contains(SNIPPET_ID))
        .unwrap() as u32;
    let provider_line = document
        .lines()
        .position(|line| line.contains("ledger/balance"))
        .unwrap() as u32;
    messages.push(request(
        2,
        "textDocument/hover",
        json!({
            "textDocument": { "uri": "wiki://p-home" },
            "position": { "line": fence_line, "character": 10 }
        }),
    ));
    messages.push(request(
        3,
        "textDocument/hover",
        json!({
            "textDocument": { "uri": "wiki://p-home" },
            "position": { "line": provider_line, "character": 5 }
        }),
    ));
    messages.extend(shutdown_and_exit());

    let (code, frames) = run_lsp(messages, fake.path().to_path_buf());
    assert_eq!(code, 0);

    let fence_hover = response(&frames, 2)["result"]["contents"]["value"]
        .as_str()
        .expect("fence hover");
    assert!(fence_hover.contains(SNIPPET_TITLE), "{fence_hover}");
    assert!(
        fence_hover.contains("**format** `hledger`"),
        "{fence_hover}"
    );
    assert!(
        fence_hover.contains("**language** `ledger`"),
        "{fence_hover}"
    );

    let provider_hover = response(&frames, 3)["result"]["contents"]["value"]
        .as_str()
        .expect("provider hover");
    assert!(
        provider_hover.contains("ledger/balance"),
        "{provider_hover}"
    );
    assert!(provider_hover.contains("**Arguments**"), "{provider_hover}");
    assert!(provider_hover.contains("query(...)"), "{provider_hover}");
}

#[test]
fn initialize_advertises_the_mvp_capabilities_and_exits_cleanly() {
    let fake = FakeServer::start();
    let mut messages = vec![initialize_message(Some(fake.path()))];
    messages.extend(shutdown_and_exit());

    let (code, frames) = run_lsp(messages, fake.path().to_path_buf());
    assert_eq!(code, 0);
    let result = &response(&frames, 1)["result"];
    assert_eq!(result["capabilities"]["positionEncoding"], "utf-16");
    assert_eq!(
        result["capabilities"]["textDocumentSync"]["openClose"],
        true
    );
    assert_eq!(result["capabilities"]["definitionProvider"], true);
    assert_eq!(result["capabilities"]["referencesProvider"], true);
    let triggers = result["capabilities"]["completionProvider"]["triggerCharacters"]
        .as_array()
        .expect("trigger characters");
    assert!(triggers.iter().any(|trigger| trigger == "["));
    assert!(triggers.iter().any(|trigger| trigger == "!"));
    assert_eq!(result["serverInfo"]["name"], "wiki-lsp");
}
