//! End-to-end socket test: start the real binary, exercise the M0 + M1/M3
//! methods, then restart against the same `--data` file.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

struct Server {
    child: Child,
    socket: PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn temp_root() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "wiki-socket-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn start(root: &Path) -> Server {
    let socket = root.join("wiki.sock");
    let data = root.join("wiki.json");
    let child = Command::new(env!("CARGO_BIN_EXE_wiki-server"))
        .arg("serve")
        .arg("--socket")
        .arg(&socket)
        .arg("--data")
        .arg(&data)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn wiki-server");
    let server = Server { child, socket };
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if UnixStream::connect(&server.socket).is_ok() {
            return server;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!(
        "wiki-server did not start listening on {}",
        server.socket.display()
    );
}

fn rpc(socket: &Path, method: &str, params: Value) -> Value {
    let mut stream = UnixStream::connect(socket).expect("connect");
    let request = json!({ "id": 1, "method": method, "params": params });
    let mut bytes = serde_json::to_vec(&request).unwrap();
    bytes.push(b'\n');
    stream.write_all(&bytes).unwrap();
    stream.flush().unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    serde_json::from_str(&line).expect("valid JSON response")
}

fn ok(socket: &Path, method: &str, params: Value) -> Value {
    let response = rpc(socket, method, params);
    assert!(
        response.get("error").is_none(),
        "{method} failed: {response}"
    );
    response["result"].clone()
}

fn cli(socket: &Path, args: &[&str], stdin: &str) -> Value {
    let mut child = Command::new(env!("CARGO_BIN_EXE_wiki-server"))
        .args(args)
        .arg("--socket")
        .arg(socket)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn wiki-server CLI");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(stdin.as_bytes())
        .expect("write CLI stdin");
    let output = child.wait_with_output().expect("CLI output");
    assert!(
        output.status.success(),
        "CLI {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("CLI returns JSON")
}

#[test]
fn cli_reads_stdin_once_and_exposes_create_convert_save_and_delete() {
    let root = temp_root();
    let server = start(&root);
    let socket = server.socket.clone();

    let page = cli(
        &socket,
        &["create_page", "CLI page", "--tags", "a,b"],
        "# CLI\n\n!```rust|code\nfn main() {}\n```\n",
    );
    let page_id = page["id"].as_str().unwrap().to_owned();
    assert_eq!(page["type"], "page");
    assert_eq!(page["title"], "CLI page");
    assert_eq!(page["tags"], json!(["a", "b"]));
    assert_eq!(page["body"], "# CLI\n\n!```rust|code\nfn main() {}\n```\n");

    let report = cli(
        &socket,
        &["convert_fence", &page_id, "--fence-index", "0"],
        "",
    );
    assert_eq!(report["snippet"]["title"], "fn main() {}");
    assert_eq!(report["snippet"]["format"], "rust");
    assert_eq!(report["snippet"]["kinds"], json!(["wiki", "code"]));
    assert_eq!(report["page"]["rev"], 2);
    let snippet_id = report["snippet"]["id"].as_str().unwrap().to_owned();

    // A stdin body must arrive exactly once and must not be replaced by EOF.
    let saved = cli(
        &socket,
        &["save", &page_id, "--rev", "2"],
        "# Saved by CLI\n",
    );
    assert_eq!(saved["body"], "# Saved by CLI\n");
    assert_eq!(saved["rev"], 3);
    assert_eq!(
        ok(&socket, "open", json!({ "id": page_id }))["body"],
        "# Saved by CLI\n"
    );

    let snippet = cli(
        &socket,
        &["create_snippet", "CLI snippet", "--format", "hledger"],
        "2026-11-16 * z\n",
    );
    assert_eq!(snippet["type"], "snippet");
    assert_eq!(snippet["body"], "2026-11-16 * z\n");
    assert_eq!(snippet["format"], "hledger");
    assert_eq!(snippet["language"], "ledger");
    assert_eq!(snippet["kinds"], json!(["wiki", "ledger"]));

    let deleted = cli(&socket, &["delete", &snippet_id], "");
    assert_eq!(deleted["deleted"], true);
    assert_eq!(
        rpc(&socket, "open", json!({ "id": snippet_id }))["error"]["code"],
        404
    );

    drop(server);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn socket_round_trip_create_convert_save_export_and_restart() {
    let root = temp_root();
    let server = start(&root);
    let socket = server.socket.clone();

    let status = ok(&socket, "status", json!({}));
    assert_eq!(status["backend"], "json");
    assert_eq!(status["contents"]["pages"], 16);
    assert_eq!(status["contents"]["snippets"], 5);
    assert_eq!(
        status["persistence"],
        root.join("wiki.json").display().to_string()
    );
    let socket_mode = std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777;
    assert_eq!(socket_mode, 0o600);

    let list = ok(&socket, "list", json!({}));
    assert!(
        list["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == "p-home")
    );
    let pages = ok(&socket, "list", json!({ "type": "page" }));
    assert!(
        pages["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["type"] == "page")
    );

    let hits = ok(&socket, "search", json!({ "query": "電気" }));
    assert!(
        hits["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == "p-electricity")
    );
    assert!(
        hits["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["title"] == "電気代")
    );

    let workspace = ok(&socket, "workspace", json!({ "id": "p-home" }));
    for key in [
        "page", "before", "after", "snippet", "outline", "links", "related", "recent", "info",
    ] {
        assert!(workspace.get(key).is_some(), "workspace missing {key}");
    }
    assert_eq!(workspace["page"]["title"], "家計の記録");
    assert_eq!(
        workspace["snippet"]["id"],
        "3f2a9c1e00000000000000000000000000000000000000000000000000000001"
    );
    assert!(workspace["before"].as_array().unwrap().iter().any(|line| {
        line.as_str()
            .is_some_and(|line| line.contains("毎月の支出"))
    }));
    assert!(workspace["info"]["word_count"].as_u64().unwrap() > 0);

    let created_snippet = ok(
        &socket,
        "create_snippet",
        json!({ "title": "Socket snippet", "body": "2026-11-16 * y\n", "format": "hledger" }),
    );
    assert_eq!(created_snippet["type"], "snippet");
    assert_eq!(created_snippet["format"], "hledger");
    assert_eq!(created_snippet["language"], "ledger");
    assert_eq!(created_snippet["kinds"], json!(["wiki", "ledger"]));

    let created = ok(
        &socket,
        "create_page",
        json!({
            "title": "Integration page",
            "body": "# Integration\n\n!```rust|code\nfn main() {}\n```\n"
        }),
    );
    let page_id = created["id"].as_str().unwrap().to_owned();
    assert_eq!(page_id.len(), 64);
    assert_eq!(created["type"], "page");
    assert_eq!(created["rev"], 1);

    let report = ok(
        &socket,
        "convert_fence",
        json!({ "page_id": page_id, "fence_index": 0 }),
    );
    let snippet_id = report["snippet"]["id"].as_str().unwrap().to_owned();
    assert_eq!(report["snippet"]["type"], "snippet");
    assert_eq!(report["snippet"]["format"], "rust");
    assert_eq!(report["snippet"]["language"], "rust");
    assert_eq!(report["snippet"]["kinds"], json!(["wiki", "code"]));
    assert!(
        report["before_body"]
            .as_str()
            .unwrap()
            .contains("fn main() {}")
    );
    assert_eq!(
        report["after_body"].as_str().unwrap(),
        format!("# Integration\n\n!```rust|code||{snippet_id}\n```\n")
    );
    assert_eq!(report["page"]["rev"], 2);

    let opened = ok(&socket, "open", json!({ "id": page_id }));
    assert_eq!(opened["body"], report["after_body"]);
    assert_eq!(opened["rev"], 2);

    let saved = ok(
        &socket,
        "save",
        json!({ "id": page_id, "rev": 2, "body": "# Saved\n\ntext\n" }),
    );
    assert_eq!(saved["rev"], 3);
    assert_eq!(saved["body"], "# Saved\n\ntext\n");

    let conflict = rpc(
        &socket,
        "save",
        json!({ "id": page_id, "rev": 2, "body": "stale\n" }),
    );
    assert_eq!(conflict["error"]["code"], 409);
    assert_eq!(conflict["error"]["data"]["type"], "revision_conflict");
    assert_eq!(conflict["error"]["data"]["provided_rev"], 2);
    assert_eq!(conflict["error"]["data"]["current_rev"], 3);
    assert_eq!(
        ok(&socket, "open", json!({ "id": page_id }))["body"],
        "# Saved\n\ntext\n"
    );

    let conflicts = ok(&socket, "conflicts", json!({ "id": page_id }));
    assert_eq!(conflicts["supported"], false);

    let export_dir = root.join("export");
    let export = ok(
        &socket,
        "export",
        json!({ "dir": export_dir.display().to_string() }),
    );
    assert_eq!(export["raw"], false);
    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(export_dir.join("manifest.json")).unwrap())
            .unwrap();
    assert!(
        manifest["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| { entry["id"] == page_id.as_str() && entry["type"] == "page" })
    );
    let raw_export = ok(
        &socket,
        "export",
        json!({ "dir": export_dir.join("raw").display().to_string(), "raw": true }),
    );
    let page_file = raw_export["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| {
            file["path"]
                .as_str()
                .is_some_and(|path| path.starts_with("pages/") && path.contains(&page_id[..8]))
        })
        .unwrap();
    assert!(page_file["content"].as_str().unwrap().contains("# Saved"));

    let transcluded = ok(&socket, "transclude", json!({ "id": "p-home", "depth": 1 }));
    assert!(transcluded["body"].as_str().unwrap().contains("2026-11-16"));
    assert!(!transcluded["children"].as_array().unwrap().is_empty());

    let deleted = ok(&socket, "delete", json!({ "id": snippet_id }));
    assert_eq!(deleted["deleted"], true);
    assert_eq!(
        rpc(&socket, "open", json!({ "id": snippet_id }))["error"]["code"],
        404
    );

    drop(server);

    // Restart against the same data file: the created page and its final body
    // must survive, while the deleted snippet must not return.
    let server = start(&root);
    let reopened = ok(&server.socket, "open", json!({ "id": page_id }));
    assert_eq!(reopened["body"], "# Saved\n\ntext\n");
    assert_eq!(reopened["rev"], 3);
    assert_eq!(
        rpc(&server.socket, "open", json!({ "id": snippet_id }))["error"]["code"],
        404
    );

    drop(server);
    let _ = std::fs::remove_dir_all(root);
}
