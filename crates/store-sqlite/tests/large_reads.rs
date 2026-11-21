use harmion_domain::*;
use harmion_node::{
    Error,
    access::{Credential, Grants},
    application::{self as app, Host, TokenPurpose},
    storage::{Identity, Store, read_graph},
};
use harmion_protocol::{Command, EditToken, LogEpoch, RequestId, SaveRequest, save_result_json};
use harmion_store_sqlite::SqliteStore;
use std::{collections::BTreeMap, path::PathBuf};

struct TestHost(u64);
impl Host for TestHost {
    fn random_bytes(&mut self) -> Result<[u8; 32], Error> {
        self.0 += 1;
        let mut bytes = [0; 32];
        bytes[24..].copy_from_slice(&self.0.to_be_bytes());
        Ok(bytes)
    }
    fn now_micros(&self) -> i64 {
        100
    }
}
fn identity() -> Identity {
    Identity {
        cluster: ClusterId([1; 32]),
        node: NodeId([1; 32]),
        epoch: LogEpoch([1; 32]),
    }
}
struct Test {
    path: PathBuf,
    store: SqliteStore,
    host: TestHost,
    admin: Credential,
}
impl Test {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("harmion-large-reads-{}-{name}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        let mut store = SqliteStore::open(path.join("node.db"), identity()).unwrap();
        let mut host = TestHost(0);
        let (_, admin) = app::bootstrap_admin(&mut store, &mut host, 1000).unwrap();
        Self {
            path,
            store,
            host,
            admin,
        }
    }
    fn save(&mut self, request_id: u8, command: Command) -> app::SaveResult {
        app::save(
            &mut self.store,
            &mut self.host,
            &self.admin,
            &request(request_id, command),
        )
        .unwrap()
    }
    fn counts(&self) -> [i64; 4] {
        let db = rusqlite::Connection::open(self.path.join("node.db")).unwrap();
        [
            "edit_tokens",
            "token_basis",
            "request_results",
            "accept_log",
        ]
        .map(|table| {
            db.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
        })
    }
    fn token(&mut self, token: EditToken) -> app::TokenState {
        self.store
            .transaction(|tx| Ok(tx.token(token)?.unwrap()))
            .unwrap()
    }
}
impl Drop for Test {
    fn drop(&mut self) {
        assert_eq!(self.path.parent(), Some(std::env::temp_dir().as_path()));
        assert!(
            self.path
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("harmion-large-reads-")
        );
        std::fs::remove_dir_all(&self.path).unwrap();
    }
}
fn content() -> ContentId {
    ContentId([9; 32])
}
fn request(id: u8, command: Command) -> SaveRequest {
    SaveRequest {
        request_id: RequestId([id; 32]),
        content_id: content(),
        command,
    }
}
fn body(text: String) -> BTreeMap<Field, Value> {
    BTreeMap::from([(Field::Body, Value::Body(text))])
}

#[test]
fn oversized_reads_never_mint_tokens_and_saved_receipts_survive() {
    let mut t = Test::new("oversized");
    let initial = t.save(
        1,
        Command::Create {
            fields: body("\u{1}".repeat(700_000)),
        },
    );
    let base = initial.view.unwrap().edit_token;
    // Two individually legal records create a candidate envelope over 8 MiB,
    // even though the normal get representation still fits.
    t.save(
        2,
        Command::Edit {
            token: base,
            fields: body("\u{2}".repeat(700_000)),
        },
    );
    t.save(
        3,
        Command::Edit {
            token: base,
            fields: body("\u{3}".repeat(700_000)),
        },
    );
    let read = app::get(&mut t.store, &mut t.host, &t.admin, content()).unwrap();
    assert!(read.conflicts.contains(&Field::Body));
    assert_eq!(t.token(base).purpose, TokenPurpose::Edit);
    let before = t.counts();
    let entropy = t.host.0;
    for _ in 0..3 {
        assert!(matches!(
            app::get_conflicts(&mut t.store, &mut t.host, &t.admin, content()),
            Err(Error::ResponseTooLarge)
        ));
        assert!(matches!(
            app::inspect(&mut t.store, &mut t.host, &t.admin, content()),
            Err(Error::ResponseTooLarge)
        ));
    }
    assert_eq!(
        t.host.0, entropy,
        "rejected reads must not even request token entropy"
    );
    assert_eq!(t.counts(), before);
    let edit = request(
        4,
        Command::Edit {
            token: read.edit_token,
            fields: BTreeMap::from([(
                Field::Attribute {
                    namespace: "test".into(),
                    key: "large".into(),
                },
                Value::Attribute(AttributeValue::String("\u{4}".repeat(800_000))),
            )]),
        },
    );
    let saved = app::save(&mut t.store, &mut t.host, &t.admin, &edit).unwrap();
    let receipt = save_result_json(&saved);
    assert_eq!(receipt["saved"], true);
    assert_eq!(receipt["view_error"], "response_too_large");
    assert!(receipt["view"].is_null());
    let current = saved.view.unwrap().edit_token;
    let (_, denied) =
        app::issue_app(&mut t.store, &mut t.host, &t.admin, Grants::default(), 1000).unwrap();
    let before = t.counts();
    let entropy = t.host.0;
    for _ in 0..3 {
        assert!(matches!(
            app::get(&mut t.store, &mut t.host, &t.admin, content()),
            Err(Error::ResponseTooLarge)
        ));
        assert!(matches!(
            app::get_conflicts(&mut t.store, &mut t.host, &t.admin, content()),
            Err(Error::ResponseTooLarge)
        ));
        assert!(matches!(
            app::inspect(&mut t.store, &mut t.host, &t.admin, content()),
            Err(Error::ResponseTooLarge)
        ));
        assert!(matches!(
            app::get(&mut t.store, &mut t.host, &denied, content()),
            Err(Error::NotFound)
        ));
        assert!(matches!(
            app::get_conflicts(&mut t.store, &mut t.host, &denied, content()),
            Err(Error::NotFound)
        ));
        assert!(matches!(
            app::inspect(&mut t.store, &mut t.host, &denied, content()),
            Err(Error::Forbidden)
        ));
        assert!(matches!(
            app::get(&mut t.store, &mut t.host, &t.admin, ContentId([99; 32])),
            Err(Error::NotFound)
        ));
        assert_eq!(
            save_result_json(&app::save(&mut t.store, &mut t.host, &t.admin, &edit).unwrap()),
            receipt
        );
    }
    assert_eq!(t.host.0, entropy);
    assert_eq!(t.counts(), before);
    t.store = SqliteStore::open(t.path.join("node.db"), identity()).unwrap();
    assert!(matches!(
        app::get(&mut t.store, &mut t.host, &t.admin, content()),
        Err(Error::ResponseTooLarge)
    ));
    assert_eq!(t.counts(), before);
    assert_eq!(
        save_result_json(&app::save(&mut t.store, &mut t.host, &t.admin, &edit).unwrap()),
        receipt
    );
    t.save(5, Command::Delete { token: current });
    let before = t.counts();
    assert!(matches!(
        app::get(&mut t.store, &mut t.host, &t.admin, content()),
        Err(Error::NotFound)
    ));
    assert!(matches!(
        app::get_conflicts(&mut t.store, &mut t.host, &t.admin, content()),
        Err(Error::NotFound)
    ));
    assert!(matches!(
        app::inspect(&mut t.store, &mut t.host, &t.admin, content()),
        Err(Error::ResponseTooLarge)
    ));
    assert_eq!(t.counts(), before);
}

#[test]
fn fitting_reads_keep_purpose_and_reusable_fixed_basis() {
    let mut t = Test::new("fitting");
    t.save(
        1,
        Command::Create {
            fields: body("base".into()),
        },
    );
    let edit = app::get(&mut t.store, &mut t.host, &t.admin, content())
        .unwrap()
        .edit_token;
    let resolve = app::get_conflicts(&mut t.store, &mut t.host, &t.admin, content())
        .unwrap()
        .view
        .edit_token;
    let manage = app::inspect(&mut t.store, &mut t.host, &t.admin, content())
        .unwrap()
        .view
        .edit_token;
    let fixed = t.token(edit).basis;
    assert_eq!(t.token(resolve).purpose, TokenPurpose::Resolve);
    assert_eq!(t.token(manage).purpose, TokenPurpose::Manage);
    assert_eq!(t.token(resolve).basis, fixed);
    assert_eq!(t.token(manage).basis, fixed);
    t.save(
        2,
        Command::Edit {
            token: edit,
            fields: body("left".into()),
        },
    );
    t.save(
        3,
        Command::Edit {
            token: edit,
            fields: body("right".into()),
        },
    );
    assert_eq!(t.token(edit).basis, fixed);
    let candidates = t
        .store
        .transaction(|tx| {
            Ok(
                read_graph(tx)?.snapshot(content()).unwrap().fields[&Field::Body]
                    .candidates
                    .len(),
            )
        })
        .unwrap();
    assert_eq!(candidates, 2);
}
