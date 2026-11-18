use harmion_domain::{ClusterId, NodeId};
use harmion_node::{
    Error,
    access::Credential,
    application::{self, Host},
    membership, membership_log,
    membership_state::{self, MembershipSigner, Status},
    storage::{Identity, Store},
};
use harmion_protocol::{
    LogEntry, LogEpoch, Page, PeerCommand, PeerRequest,
    membership::{Action, Binding, Certificate, Member, Statement},
};
use harmion_store_sqlite::SqliteStore;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

struct Entropy(u8);
impl Host for Entropy {
    fn random_bytes(&mut self) -> Result<[u8; 32], Error> {
        self.0 += 1;
        Ok([self.0; 32])
    }
    fn now_micros(&self) -> i64 {
        0
    }
}
const CLUSTER: ClusterId = ClusterId([1; 32]);
fn signer(n: u8) -> MembershipSigner {
    MembershipSigner::from_bytes(&[n; 32])
}
fn member(n: u8) -> Member {
    let key = signer(n).public_key();
    Member {
        node: NodeId([n; 32]),
        signing_key: key,
        binding: Binding::Iroh(key),
        endpoint: String::new(),
    }
}
fn certificate(from: u8, action: Action) -> Certificate {
    signer(from)
        .sign(Statement {
            cluster: CLUSTER,
            issuer: NodeId([from; 32]),
            signing_key: signer(from).public_key(),
            nonce: [0; 32],
            action,
        })
        .unwrap()
}
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Test {
    path: PathBuf,
    identity: Identity,
    db: SqliteStore,
    admin: Credential,
}
impl Test {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "harmion-membership-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        let identity = Identity {
            cluster: CLUSTER,
            node: member(2).node,
            epoch: LogEpoch([2; 32]),
        };
        let mut db = SqliteStore::open(path.join("db"), identity).unwrap();
        let (_, admin) = application::bootstrap_admin(&mut db, &mut Entropy(10), i64::MAX).unwrap();
        membership_state::initialize(&mut db, &signer(2), member(2), true).unwrap();
        membership_state::approve(&mut db, 0, &admin, &signer(2), CLUSTER, member(3), [1; 32])
            .unwrap();
        Self {
            path,
            identity,
            db,
            admin,
        }
    }
}
impl Drop for Test {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[test]
fn invalid_signature_rolls_back_normal_prefix_projection_and_cursor() {
    let mut t = Test::new();
    let good = certificate(3, Action::Admit(member(4)));
    let bad = certificate(3, Action::Admit(member(5)));
    let mut signature = *bad.signature();
    signature[0] ^= 1;
    let bad = Certificate::new(bad.statement().clone(), signature).unwrap();
    let page = Page {
        epoch: LogEpoch([3; 32]),
        after: 0,
        through: 2,
        high: 2,
        entries: vec![
            LogEntry {
                seq: 1,
                record: good.clone(),
            },
            LogEntry {
                seq: 2,
                record: bad,
            },
        ],
    };
    assert!(matches!(
        t.db.transaction(|tx| membership_log::receive(tx, member(3).node, &page)),
        Err(Error::InvalidSignature)
    ));
    t.db.transaction(|tx| {
        assert_eq!(tx.membership_entries()?.len(), 2);
        assert_eq!(tx.membership_cursor(member(3).node, page.epoch)?, 0);
        assert!(tx.peer(member(4).node)?.is_none());
        Ok(())
    })
    .unwrap();
    let page = Page {
        through: 1,
        high: 1,
        entries: vec![LogEntry {
            seq: 1,
            record: good.clone(),
        }],
        ..page
    };
    t.db.transaction(|tx| membership_log::receive(tx, member(3).node, &page))
        .unwrap();
    assert!(matches!(
        t.db.transaction(|tx| membership_log::receive(tx, member(3).node, &page)),
        Err(Error::CursorMismatch { current: 1 })
    ));
    let duplicate = Page {
        after: 1,
        through: 2,
        high: 2,
        entries: vec![LogEntry {
            seq: 2,
            record: good,
        }],
        ..page
    };
    t.db.transaction(|tx| membership_log::receive(tx, member(3).node, &duplicate))
        .unwrap();
    let mut reopened = SqliteStore::open(t.path.join("db"), t.identity).unwrap();
    reopened
        .transaction(|tx| {
            assert_eq!(tx.membership_entries()?.len(), 3);
            assert_eq!(tx.membership_cursor(member(3).node, page.epoch)?, 2);
            assert_eq!(tx.cursor(member(3).node, page.epoch)?, 0);
            membership::active_peer(tx, member(4).node)?;
            Ok(())
        })
        .unwrap();
}

#[test]
fn cutoff_revokes_a_previously_seen_but_unretained_admission() {
    let mut t = Test::new();
    let bc = certificate(3, Action::Admit(member(4)));
    t.db.transaction(|tx| membership_state::apply(tx, &[bc]))
        .unwrap();
    let rb = certificate(
        2,
        Action::Retire {
            target: member(3).node,
            retained: Default::default(),
        },
    );
    t.db.transaction(|tx| membership_state::apply(tx, &[rb]))
        .unwrap();
    t.db.transaction(|tx| {
        assert!(matches!(
            membership::active_peer(tx, member(3).node),
            Err(Error::PeerRetired)
        ));
        assert!(matches!(
            membership::active_peer(tx, member(4).node),
            Err(Error::Unauthenticated)
        ));
        assert_eq!(
            membership_state::state(tx)?.unwrap().nodes[&member(4).node],
            Status::Unconfirmed
        );
        Ok(())
    })
    .unwrap();
    membership_state::approve(
        &mut t.db,
        0,
        &t.admin,
        &signer(2),
        CLUSTER,
        member(4),
        [2; 32],
    )
    .unwrap();
    t.db.transaction(|tx| membership::active_peer(tx, member(4).node))
        .unwrap();
    assert!(matches!(
        membership_state::approve(
            &mut t.db,
            0,
            &t.admin,
            &signer(2),
            CLUSTER,
            member(3),
            [3; 32]
        ),
        Err(Error::PeerRetired)
    ));
}

#[test]
fn self_retirement_blocks_data_and_management_but_not_evidence_exchange() {
    let mut t = Test::new();
    let known = t.db.transaction(|tx| tx.membership_entries()).unwrap();
    let retirement = certificate(
        3,
        Action::Retire {
            target: member(2).node,
            retained: known.iter().map(|e| e.record.id()).collect(),
        },
    );
    t.db.transaction(|tx| membership_state::apply(tx, &[retirement]))
        .unwrap();
    assert!(matches!(
        membership_state::approve(
            &mut t.db,
            0,
            &t.admin,
            &signer(2),
            CLUSTER,
            member(4),
            [4; 32]
        ),
        Err(Error::PeerRetired)
    ));
    let peer = membership_state::as_peer(&member(3));
    let mut request = PeerRequest {
        cluster: CLUSTER,
        node: peer.node,
        command: PeerCommand::Status,
    };
    assert!(matches!(
        membership::handle(&mut t.db, &peer.key, &request),
        Err(Error::PeerRetired)
    ));
    request.command = PeerCommand::MembershipStatus;
    membership::handle(&mut t.db, &peer.key, &request).unwrap();
}

#[test]
fn corrupted_membership_signature_is_rejected_on_reopen() {
    let t = Test::new();
    let db = rusqlite::Connection::open(t.path.join("db")).unwrap();
    let bytes: Vec<u8> = db
        .query_row("SELECT bytes FROM membership_log WHERE seq=2", [], |r| {
            r.get(0)
        })
        .unwrap();
    let original = Certificate::from_bytes(&bytes).unwrap();
    let mut signature = *original.signature();
    signature[0] ^= 1;
    let corrupted = Certificate::new(original.statement().clone(), signature).unwrap();
    db.execute(
        "UPDATE membership_log SET id=?1,bytes=?2 WHERE seq=2",
        rusqlite::params![corrupted.id(), corrupted.bytes()],
    )
    .unwrap();
    assert!(matches!(
        SqliteStore::open(t.path.join("db"), t.identity),
        Err(Error::InvalidSignature)
    ));
}

#[test]
fn a_joined_node_cannot_use_its_local_identity_as_cluster_authority() {
    let mut a = Test::new();
    let identity = Identity {
        cluster: CLUSTER,
        node: member(4).node,
        epoch: LogEpoch([4; 32]),
    };
    let mut c = SqliteStore::open(":memory:", identity).unwrap();
    let (_, admin) = application::bootstrap_admin(&mut c, &mut Entropy(20), i64::MAX).unwrap();
    membership_state::initialize(&mut c, &signer(4), member(4), false).unwrap();
    assert!(matches!(
        membership_state::approve(&mut c, 0, &admin, &signer(4), CLUSTER, member(5), [8; 32]),
        Err(Error::Unauthenticated)
    ));
    let root =
        a.db.transaction(|tx| tx.membership_root())
            .unwrap()
            .unwrap();
    membership_state::join(&mut c, 0, &admin, CLUSTER, member(2), root.clone(), false).unwrap();
    membership_state::approve(
        &mut a.db,
        0,
        &a.admin,
        &signer(2),
        CLUSTER,
        member(4),
        [2; 32],
    )
    .unwrap();
    let records = a.db.transaction(|tx| tx.membership_entries()).unwrap();
    let ab = records
        .iter()
        .find(|e| matches!(&e.record.statement().action,Action::Admit(m) if m.node==member(3).node))
        .unwrap()
        .record
        .clone();
    c.transaction(|tx| {
        membership_state::apply(
            tx,
            &records.iter().map(|e| e.record.clone()).collect::<Vec<_>>(),
        )
    })
    .unwrap();
    assert!(matches!(
        membership_state::inspect(&mut c, &admin, 0).unwrap().nodes[&member(4).node],
        Status::Active(_)
    ));
    // B's cut knows A's approval of B but not A's approval of C.
    let retire_a = certificate(
        3,
        Action::Retire {
            target: member(2).node,
            retained: [ab.id()].into(),
        },
    );
    c.transaction(|tx| membership_state::apply(tx, &[retire_a]))
        .unwrap();
    assert_eq!(
        membership_state::inspect(&mut c, &admin, 0).unwrap().nodes[&member(4).node],
        Status::Unconfirmed
    );
    assert!(matches!(
        membership_state::approve(&mut c, 0, &admin, &signer(4), CLUSTER, member(5), [9; 32]),
        Err(Error::Unauthenticated)
    ));
    // Restart initialization cannot turn a joined node into a new founder.
    membership_state::initialize(&mut c, &signer(4), member(4), true).unwrap();
    assert_eq!(
        c.transaction(|tx| tx.membership_root()).unwrap().unwrap(),
        root
    );
    assert_eq!(
        membership_state::inspect(&mut c, &admin, 0).unwrap().nodes[&member(4).node],
        Status::Unconfirmed
    );
}
