use harmion_domain::{ClusterId, NodeId};
use harmion_node::storage::Identity;
use harmion_protocol::LogEpoch;
use harmion_store_sqlite::{SqliteStore, snapshot_database};
use rusqlite::Connection;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn identity() -> Identity {
    Identity {
        cluster: ClusterId([1; 32]),
        node: NodeId([2; 32]),
        epoch: LogEpoch([3; 32]),
    }
}

#[test]
fn online_snapshot_is_a_standalone_consistent_database() {
    let number = NEXT.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "harmion-snapshot-test-{}-{number}",
        std::process::id()
    ));
    std::fs::create_dir(&root).unwrap();
    let source = root.join("node.db");
    let destination = root.join("snapshot.db");
    let _store = SqliteStore::open(&source, identity()).unwrap();

    snapshot_database(&source, &destination).unwrap();

    let copy = Connection::open(&destination).unwrap();
    let check: String = copy
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .unwrap();
    assert_eq!(check, "ok");
    let tables: i64 = copy
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(tables > 0);
    std::fs::remove_dir_all(root).unwrap();
}
