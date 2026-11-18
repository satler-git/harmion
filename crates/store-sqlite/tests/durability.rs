use harmion_domain::*;
use harmion_node::{
    Error, replication,
    storage::{Identity, Store, accept_records, read_graph},
};
use harmion_protocol::{LogEntry, LogEpoch, Page, Record};
use harmion_store_sqlite::SqliteStore;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "harmion-store-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn store(&self, n: u8) -> SqliteStore {
        SqliteStore::open(self.path(&format!("{n}.db")), identity(n)).unwrap()
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn identity(n: u8) -> Identity {
    Identity {
        cluster: ClusterId([1; 32]),
        node: NodeId([n; 32]),
        epoch: LogEpoch([n; 32]),
    }
}
fn id(n: u8) -> UpdateId {
    UpdateId([n; 32])
}
fn ids(ns: &[u8]) -> BTreeSet<UpdateId> {
    ns.iter().map(|n| id(*n)).collect()
}
fn content() -> ContentId {
    ContentId([1; 32])
}
fn record(n: u8, action: Action, basis: &[u8], text: Option<(&str, &[u8])>) -> Record {
    Record::from_update(Update {
        format_version: 1,
        cluster_id: identity(1).cluster,
        content_id: content(),
        root_id: id(1),
        update_id: id(n),
        author_node_id: identity(n).node,
        edited_at: i64::from(n),
        action,
        basis: ids(basis),
        changes: text
            .map(|(s, parents)| {
                BTreeMap::from([(
                    Field::Body,
                    Change {
                        value: Value::Body(s.into()),
                        replaces: ids(parents),
                    },
                )])
            })
            .unwrap_or_default(),
    })
    .unwrap()
}
fn root() -> Record {
    record(1, Action::Create, &[], Some(("base", &[])))
}
fn save(store: &mut SqliteStore, records: &[Record]) -> usize {
    store.transaction(|tx| accept_records(tx, records)).unwrap()
}
fn count(store: &mut SqliteStore) -> usize {
    store.transaction(|tx| Ok(tx.entries()?.len())).unwrap()
}
fn transfer(a: &mut SqliteStore, b: &mut SqliteStore) {
    let sender = a.identity();
    let high = a.transaction(replication::high).unwrap();
    loop {
        let after = b
            .transaction(|tx| tx.cursor(sender.node, sender.epoch))
            .unwrap();
        if after == high {
            break;
        }
        let page = a
            .transaction(|tx| replication::page(tx, sender.epoch, after, high))
            .unwrap();
        b.transaction(|tx| replication::receive(tx, sender.node, &page))
            .unwrap();
    }
}

#[test]
fn restart_preserves_bytes_identity_and_deduplication() {
    let dir = Directory::new();
    let mut a = dir.store(1);
    assert_eq!(save(&mut a, &[root(), root()]), 1);
    let edit = record(2, Action::Edit, &[1], Some(("edited", &[1])));
    save(&mut a, std::slice::from_ref(&edit));
    drop(a);
    let mut a = dir.store(1);
    assert_eq!(save(&mut a, &[root(), edit.clone()]), 0);
    assert_eq!(
        a.transaction(|tx| Ok(tx.entries()?[1].record.clone()))
            .unwrap(),
        edit
    );
    assert_eq!(
        a.transaction(|tx| Ok(read_graph(tx)?.snapshot(content()).unwrap().content.body))
            .unwrap(),
        "edited"
    );
    let wrong = Identity {
        node: NodeId([99; 32]),
        ..identity(1)
    };
    assert!(matches!(
        SqliteStore::open(dir.path("1.db"), wrong),
        Err(Error::IdentityMismatch)
    ));
    assert_eq!(count(&mut a), 2);
}

#[test]
fn errors_and_panics_roll_back_updates_and_cursor_together() {
    let dir = Directory::new();
    let mut a = dir.store(1);
    let sender = identity(2);
    let result: Result<(), Error> = a.transaction(|tx| {
        accept_records(tx, &[root()])?;
        tx.set_cursor(sender.node, sender.epoch, 10)?;
        Err(Error::Storage("injected failure after writes".into()))
    });
    assert!(result.is_err());
    assert_eq!(count(&mut a), 0);
    assert_eq!(
        a.transaction(|tx| tx.cursor(sender.node, sender.epoch))
            .unwrap(),
        0
    );
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _: Result<(), Error> = a.transaction(|tx| {
            accept_records(tx, &[root()])?;
            panic!("injected callback panic");
        });
    }));
    assert!(panic.is_err());
    assert_eq!(count(&mut a), 0);
    assert_eq!(save(&mut a, &[root()]), 1);
}

#[test]
fn page_failure_rolls_back_valid_prefix_and_database_write_failure() {
    let dir = Directory::new();
    let mut receiver = dir.store(2);
    let mut p = Page {
        epoch: identity(1).epoch,
        after: 0,
        through: 3,
        high: 3,
        entries: vec![
            LogEntry {
                seq: 1,
                record: root(),
            },
            LogEntry {
                seq: 3,
                record: record(2, Action::Edit, &[99], Some(("bad", &[1]))),
            },
        ],
    };
    assert!(
        receiver
            .transaction(|tx| replication::receive(tx, identity(1).node, &p))
            .is_err()
    );
    assert_eq!(count(&mut receiver), 0);
    p.entries[1].record = record(2, Action::Edit, &[1], Some(("good", &[1])));
    // A database failure after appending valid records must still undo the page.
    let db = rusqlite::Connection::open(dir.path("2.db")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_cursor BEFORE INSERT ON peer_cursors BEGIN SELECT RAISE(ABORT, 'injected disk failure'); END;").unwrap();
    assert!(
        receiver
            .transaction(|tx| replication::receive(tx, identity(1).node, &p))
            .is_err()
    );
    assert_eq!(count(&mut receiver), 0);
    assert_eq!(
        receiver
            .transaction(|tx| tx.cursor(identity(1).node, identity(1).epoch))
            .unwrap(),
        0
    );
    db.execute_batch("DROP TRIGGER fail_cursor").unwrap();
    assert_eq!(
        receiver
            .transaction(|tx| replication::receive(tx, identity(1).node, &p))
            .unwrap(),
        3
    );
    assert_eq!(count(&mut receiver), 2);
}

#[test]
fn lost_ack_epoch_rescan_and_sequence_gaps_do_not_duplicate() {
    let dir = Directory::new();
    let mut receiver = dir.store(2);
    let mut p = Page {
        epoch: identity(1).epoch,
        after: 0,
        through: 9,
        high: 9,
        entries: vec![
            LogEntry {
                seq: 2,
                record: root(),
            },
            LogEntry {
                seq: 9,
                record: record(2, Action::Edit, &[1], Some(("good", &[1]))),
            },
        ],
    };
    receiver
        .transaction(|tx| replication::receive(tx, identity(1).node, &p))
        .unwrap();
    drop(receiver);
    let mut receiver = dir.store(2);
    assert!(matches!(
        receiver.transaction(|tx| replication::receive(tx, identity(1).node, &p)),
        Err(Error::CursorMismatch { current: 9 })
    ));
    p.epoch = LogEpoch([99; 32]);
    receiver
        .transaction(|tx| replication::receive(tx, identity(1).node, &p))
        .unwrap();
    assert_eq!(count(&mut receiver), 2);
    // Cursor keys include sender as well as epoch.
    assert_eq!(
        receiver
            .transaction(|tx| tx.cursor(identity(3).node, p.epoch))
            .unwrap(),
        0
    );
}

#[test]
fn bidirectional_sqlite_sync_converges_after_partition_and_late_resolution() {
    let dir = Directory::new();
    let mut a = dir.store(1);
    let mut b = dir.store(2);
    save(&mut a, &[root()]);
    transfer(&mut a, &mut b);
    save(&mut a, &[record(2, Action::Edit, &[1], Some(("A", &[1])))]);
    save(&mut b, &[record(3, Action::Edit, &[1], Some(("B", &[1])))]);
    transfer(&mut b, &mut a);
    // A resolves observed A+B while B edits its old branch during partition.
    save(
        &mut a,
        &[record(
            4,
            Action::Edit,
            &[2, 3],
            Some(("resolved", &[2, 3])),
        )],
    );
    save(
        &mut b,
        &[record(5, Action::Edit, &[3], Some(("late B", &[3])))],
    );
    drop(b);
    let mut b = dir.store(2);
    for _ in 0..3 {
        transfer(&mut a, &mut b);
        transfer(&mut b, &mut a);
    }
    assert_eq!(count(&mut a), 5);
    assert_eq!(count(&mut b), 5);
    let sa = a
        .transaction(|tx| Ok(read_graph(tx)?.snapshot(content()).unwrap()))
        .unwrap();
    let sb = b
        .transaction(|tx| Ok(read_graph(tx)?.snapshot(content()).unwrap()))
        .unwrap();
    assert_eq!(sa, sb);
    assert_eq!(sa.fields[&Field::Body].candidates.len(), 2);
    assert_eq!(sa.basis, ids(&[4, 5]));
    save(&mut a, &[record(6, Action::Delete, &[4, 5], None)]);
    save(
        &mut b,
        &[record(
            7,
            Action::Edit,
            &[4, 5],
            Some(("survives delete", &[5])),
        )],
    );
    transfer(&mut a, &mut b);
    transfer(&mut b, &mut a);
    let sa = a
        .transaction(|tx| Ok(read_graph(tx)?.snapshot(content()).unwrap()))
        .unwrap();
    let sb = b
        .transaction(|tx| Ok(read_graph(tx)?.snapshot(content()).unwrap()))
        .unwrap();
    assert_eq!(sa, sb);
    assert_eq!(sa.life, Life::Conflict);
    assert_eq!(count(&mut a), 7);
    assert_eq!(count(&mut b), 7);
}

#[test]
fn identity_collision_and_corruption_do_not_get_silently_repaired() {
    let dir = Directory::new();
    let mut a = dir.store(1);
    save(&mut a, &[root()]);
    let bad = record(1, Action::Create, &[], Some(("different", &[])));
    assert!(matches!(
        a.transaction(|tx| accept_records(tx, &[bad])),
        Err(Error::RecordRejected {
            reason: harmion_domain::Error::IdMismatch,
            ..
        })
    ));
    assert_eq!(count(&mut a), 1);
    drop(a);
    let db = rusqlite::Connection::open(dir.path("1.db")).unwrap();
    db.execute("UPDATE accept_log SET digest=?1", [[0u8; 32]])
        .unwrap();
    assert!(matches!(
        SqliteStore::open(dir.path("1.db"), identity(1)),
        Err(Error::CorruptLog)
    ));
}

#[test]
fn invalid_page_boundaries_do_not_advance_progress() {
    let dir = Directory::new();
    let mut b = dir.store(2);
    let valid = Page {
        epoch: identity(1).epoch,
        after: 0,
        through: 1,
        high: 1,
        entries: vec![LogEntry {
            seq: 1,
            record: root(),
        }],
    };
    let mut cases = Vec::new();
    let mut p = valid.clone();
    p.through = 2;
    cases.push(p);
    let mut p = valid.clone();
    p.entries[0].seq = 0;
    cases.push(p);
    let mut p = valid.clone();
    p.entries.push(p.entries[0].clone());
    cases.push(p);
    let mut p = valid.clone();
    p.entries.clear();
    p.high = 2;
    cases.push(p);
    for p in cases {
        assert!(matches!(
            b.transaction(|tx| replication::receive(tx, identity(1).node, &p)),
            Err(Error::InvalidPage)
        ));
        assert_eq!(count(&mut b), 0);
        assert_eq!(
            b.transaction(|tx| tx.cursor(identity(1).node, identity(1).epoch))
                .unwrap(),
            0
        );
    }
}

#[test]
fn fixed_high_multiple_pages_and_concurrent_connections_use_durable_cursor() {
    let dir = Directory::new();
    let mut a = dir.store(1);
    let mut b = dir.store(2);
    let mut other_connection = dir.store(2);
    let mut records = vec![root()];
    for n in 2..=130 {
        records.push(record(n, Action::Edit, &[n - 1], Some(("body", &[n - 1]))));
    }
    save(&mut a, &records);
    let high = a.transaction(replication::high).unwrap();
    let first = a
        .transaction(|tx| replication::page(tx, identity(1).epoch, 0, high))
        .unwrap();
    assert_eq!(first.entries.len(), harmion_protocol::MAX_PAGE_ENTRIES);
    assert!(first.through < high);
    b.transaction(|tx| replication::receive(tx, identity(1).node, &first))
        .unwrap();
    assert!(matches!(
        other_connection.transaction(|tx| replication::receive(tx, identity(1).node, &first)),
        Err(Error::CursorMismatch { current: 128 })
    ));
    // A new update must not extend the already fixed round.
    save(
        &mut a,
        &[record(
            131,
            Action::Edit,
            &[130],
            Some(("next round", &[130])),
        )],
    );
    let last = a
        .transaction(|tx| replication::page(tx, identity(1).epoch, first.through, high))
        .unwrap();
    assert_eq!(last.entries.len(), 2);
    assert_eq!(last.through, high);
    other_connection
        .transaction(|tx| replication::receive(tx, identity(1).node, &last))
        .unwrap();
    assert_eq!(count(&mut b), 130);
    transfer(&mut a, &mut b);
    assert_eq!(count(&mut b), 131);
}
