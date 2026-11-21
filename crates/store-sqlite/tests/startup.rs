use harmion_domain::*;
use harmion_node::{
    Error,
    storage::{Identity, Store, accept_records, read_graph},
};
use harmion_protocol::{LogEpoch, Record};
use harmion_store_sqlite::SqliteStore;
use rusqlite::{Connection, params};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
fn identity() -> Identity {
    Identity {
        cluster: ClusterId([1; 32]),
        node: NodeId([1; 32]),
        epoch: LogEpoch([1; 32]),
    }
}
fn ids(values: &[u8]) -> BTreeSet<UpdateId> {
    values.iter().map(|n| UpdateId([*n; 32])).collect()
}
fn record(
    id: u8,
    content: u8,
    root: u8,
    action: Action,
    basis: &[u8],
    body: Option<(&str, &[u8])>,
) -> Record {
    Record::from_update(Update {
        format_version: 1,
        cluster_id: identity().cluster,
        content_id: ContentId([content; 32]),
        root_id: UpdateId([root; 32]),
        update_id: UpdateId([id; 32]),
        author_node_id: identity().node,
        edited_at: i64::from(id),
        action,
        basis: ids(basis),
        changes: body
            .map(|(body, replaces)| {
                BTreeMap::from([(
                    Field::Body,
                    Change {
                        value: Value::Body(body.into()),
                        replaces: ids(replaces),
                    },
                )])
            })
            .unwrap_or_default(),
    })
    .unwrap()
}
struct Test {
    path: PathBuf,
}
impl Test {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "harmion-startup-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        let mut store = SqliteStore::open(path.join("node.db"), identity()).unwrap();
        let records = [
            record(1, 10, 1, Action::Create, &[], Some(("a", &[]))),
            record(2, 20, 2, Action::Create, &[], Some(("b", &[]))),
            record(3, 10, 1, Action::Edit, &[1], Some(("left", &[1]))),
            record(4, 20, 2, Action::Edit, &[2], Some(("b edit", &[2]))),
            record(5, 10, 1, Action::Edit, &[1], Some(("right", &[1]))),
            record(6, 20, 2, Action::Delete, &[4], None),
        ];
        store
            .transaction(|tx| accept_records(tx, &records))
            .unwrap();
        Self { path }
    }
    fn sql(&self) -> Connection {
        let sql = Connection::open(self.path.join("node.db")).unwrap();
        // Deliberately bypass write-time FKs to simulate damaged on-disk state.
        // Production open re-enables FKs and must independently reject it.
        sql.pragma_update(None, "foreign_keys", false).unwrap();
        sql
    }
    fn reopen(&self) -> Result<SqliteStore, Error> {
        SqliteStore::open(self.path.join("node.db"), identity())
    }
    fn replace(&self, record: Record) {
        self.sql()
            .execute(
                "UPDATE accept_log SET bytes=?1,digest=?2 WHERE update_id=?3",
                params![record.bytes(), record.digest(), record.update().update_id.0],
            )
            .unwrap();
    }
    fn counts(&self) -> (i64, i64) {
        self.sql()
            .query_row(
                "SELECT (SELECT COUNT(*) FROM accept_log),(SELECT COUNT(*) FROM content_records)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap()
    }
    fn rejected_without_repair(&self) {
        let counts = self.counts();
        assert!(self.reopen().is_err());
        assert_eq!(self.counts(), counts);
        assert!(self.reopen().is_err());
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
                .starts_with("harmion-startup-")
        );
        std::fs::remove_dir_all(&self.path).unwrap();
    }
}

#[test]
fn interleaved_content_histories_with_gaps_conflicts_and_deletion_reopen() {
    let test = Test::new();
    test.sql()
        .execute_batch(
            "UPDATE accept_log SET seq=seq+100; UPDATE content_records SET seq=seq+100;
        UPDATE accept_log SET seq=(seq-100)*3; UPDATE content_records SET seq=(seq-100)*3;",
        )
        .unwrap();
    let mut reopened = test.reopen().unwrap();
    let graph = reopened.transaction(read_graph).unwrap();
    assert_eq!(graph.content_ids().count(), 2);
    assert_eq!(
        graph.snapshot(ContentId([10; 32])).unwrap().fields[&Field::Body]
            .candidates
            .len(),
        2
    );
    assert_eq!(
        graph.snapshot(ContentId([20; 32])).unwrap().life,
        Life::Deleted
    );
    assert_eq!(test.counts(), (6, 6));
    assert!(test.reopen().is_ok());
}

#[test]
fn indexed_legacy_versions_still_migrate_after_stream_verification() {
    for version in [7, 8, 9] {
        let test = Test::new();
        let sql = test.sql();
        sql.execute_batch("DROP TABLE sync_failures; DROP TABLE sync_observations; DROP TABLE identity_pins; DROP TABLE https_state; DROP TABLE https_challenges; DROP TABLE https_rate;")
            .unwrap();
        if version < 9 {
            sql.execute_batch(
                "DROP TABLE oauth_refresh; DROP TABLE oauth_access; DROP TABLE oauth_families;
                DROP TABLE oauth_codes; DROP TABLE oauth_pending; DROP TABLE oauth_clients;",
            )
            .unwrap();
        }
        if version < 8 {
            sql.execute_batch("DROP TABLE grant_generations; DROP TABLE content_changes;")
                .unwrap();
        }
        sql.pragma_update(None, "user_version", version).unwrap();
        test.reopen().unwrap();
        assert_eq!(
            sql.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            16
        );
        assert_eq!(test.counts(), (6, 6));
    }
}

#[test]
fn missing_orphan_and_wrong_content_mappings_fail_closed() {
    let test = Test::new();
    test.sql()
        .execute("DELETE FROM content_records WHERE seq=3", [])
        .unwrap();
    test.rejected_without_repair();
    let test = Test::new();
    test.sql()
        .execute("INSERT INTO content_records VALUES(?1,999)", [[99u8; 32]])
        .unwrap();
    test.rejected_without_repair();
    let test = Test::new();
    test.sql()
        .execute(
            "UPDATE content_records SET content=?1 WHERE seq=3",
            [[20u8; 32]],
        )
        .unwrap();
    test.rejected_without_repair();
    // Missing mapping and compensating orphan have equal table counts.
    let test = Test::new();
    test.sql()
        .execute_batch("DELETE FROM content_records WHERE seq=3;")
        .unwrap();
    test.sql()
        .execute("INSERT INTO content_records VALUES(?1,999)", [[99u8; 32]])
        .unwrap();
    assert_eq!(test.counts(), (6, 6));
    test.rejected_without_repair();
}

#[test]
fn corrupt_columns_bytes_and_sequences_fail_closed() {
    for column in ["update_id", "digest"] {
        let test = Test::new();
        test.sql()
            .execute(
                &format!("UPDATE accept_log SET {column}=?1 WHERE seq=3"),
                [[99u8; 32]],
            )
            .unwrap();
        test.rejected_without_repair();
    }
    let test = Test::new();
    let mut bytes = record(3, 10, 1, Action::Edit, &[1], Some(("left", &[1])))
        .bytes()
        .to_vec();
    bytes.push(0); // Otherwise valid, but not a canonical complete record.
    test.sql()
        .execute("UPDATE accept_log SET bytes=?1 WHERE seq=3", [bytes])
        .unwrap();
    test.rejected_without_repair();
    let test = Test::new();
    test.sql().execute_batch("UPDATE accept_log SET seq=0 WHERE seq=1; UPDATE content_records SET seq=0 WHERE seq=1;").unwrap();
    test.rejected_without_repair();
    // Move the root after its dependents without changing any record bytes.
    let test = Test::new();
    test.sql().execute_batch("UPDATE accept_log SET seq=100 WHERE seq=1; UPDATE content_records SET seq=100 WHERE seq=1;").unwrap();
    test.rejected_without_repair();
}

#[test]
fn invalid_root_basis_replacement_and_cluster_fail_closed() {
    for invalid in [
        record(3, 10, 2, Action::Edit, &[1], Some(("left", &[1]))), // foreign root
        record(3, 10, 1, Action::Edit, &[2], Some(("left", &[2]))), // foreign basis
        record(3, 10, 1, Action::Edit, &[1], Some(("left", &[2]))), // foreign replacement
        record(3, 10, 99, Action::Edit, &[1], Some(("left", &[1]))), // missing root
        record(3, 10, 1, Action::Edit, &[99], Some(("left", &[99]))), // missing basis
        record(3, 10, 3, Action::Create, &[], Some(("second root", &[]))), // root collision
    ] {
        let test = Test::new();
        test.replace(invalid);
        test.rejected_without_repair();
    }
    let test = Test::new();
    let mut wrong = record(3, 10, 1, Action::Edit, &[1], Some(("left", &[1])))
        .update()
        .clone();
    wrong.cluster_id = ClusterId([99; 32]);
    test.replace(Record::from_update(wrong).unwrap());
    test.rejected_without_repair();
}

#[test]
fn weakened_mapping_schema_cannot_assign_one_sequence_twice() {
    let test = Test::new();
    test.sql()
        .execute_batch(
            "ALTER TABLE content_records RENAME TO previous_records;
        CREATE TABLE content_records(content BLOB,seq INTEGER);
        INSERT INTO content_records SELECT content,seq FROM previous_records;
        INSERT INTO content_records SELECT content,seq FROM previous_records WHERE seq=3;
        DROP TABLE previous_records;",
        )
        .unwrap();
    test.rejected_without_repair();
}

#[test]
fn weakened_log_schema_cannot_reuse_global_update_ids_or_sequences() {
    for duplicate_column in ["update_id", "seq"] {
        let test = Test::new();
        test.sql()
            .execute_batch(
                "ALTER TABLE accept_log RENAME TO previous_log;
            CREATE TABLE accept_log(seq INTEGER,update_id BLOB,bytes BLOB,digest BLOB);
            INSERT INTO accept_log SELECT seq,update_id,bytes,digest FROM previous_log;
            DROP TABLE previous_log;",
            )
            .unwrap();
        test.sql().execute(&format!("UPDATE accept_log SET {duplicate_column}=(SELECT {duplicate_column} FROM accept_log WHERE seq=1) WHERE seq=2"), []).unwrap();
        test.rejected_without_repair();
    }
}
