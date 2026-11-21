//! Durable known-signing-identity pins: provenance, retirement retention and
//! resistance to cut-excluded admissions from a retired issuer.
use harmion_domain::{ClusterId, NodeId};
use harmion_node::{
    Error,
    access::Credential,
    application::{self, Host},
    identity::{KnownIdentity, Provenance},
    membership::{self, Peer, PeerKey},
    membership_log,
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
fn request(node: u8, command: PeerCommand) -> PeerRequest {
    PeerRequest {
        cluster: CLUSTER,
        node: NodeId([node; 32]),
        command,
    }
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
            "harmion-pins-{}-{}",
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
    fn pin(&mut self, node: u8) -> Option<KnownIdentity> {
        self.db
            .transaction(|tx| tx.known_peer_identity(NodeId([node; 32])))
            .unwrap()
    }
    fn reopen(&mut self) {
        self.db = SqliteStore::open(self.path.join("db"), self.identity).unwrap();
    }
}
impl Drop for Test {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[test]
fn pins_follow_owner_registration_and_received_admissions_only() {
    let mut t = Test::new();
    let owner = t.pin(3).expect("owner registration pin");
    assert_eq!(owner.signing_key, signer(3).public_key());
    assert_eq!(owner.key, PeerKey::Iroh(signer(3).public_key()));
    assert_eq!(owner.provenance, Provenance::OwnerRegistration);
    assert!(
        t.pin(2).is_none(),
        "a local node is not a remote trust root"
    );

    // An admission that is effective where it is accepted creates its own pin.
    let admit = certificate(3, Action::Admit(member(4)));
    t.db.transaction(|tx| membership_state::apply(tx, std::slice::from_ref(&admit)))
        .unwrap();
    let received = t.pin(4).expect("accepted admission pin");
    assert_eq!(received.signing_key, signer(4).public_key());
    assert_eq!(
        received.provenance,
        Provenance::AcceptedAdmission(admit.id())
    );
    t.reopen();
    assert_eq!(
        t.pin(4).expect("pins survive re-open").provenance,
        Provenance::AcceptedAdmission(admit.id())
    );
}

#[test]
fn pins_are_immutable_and_never_reuse_a_key_for_another_node() {
    let mut t = Test::new();
    // Same node, different key is never a rewrite.
    let replacement = Member {
        signing_key: signer(44).public_key(),
        binding: Binding::Iroh(signer(44).public_key()),
        ..member(3)
    };
    assert!(matches!(
        membership_state::approve(
            &mut t.db,
            0,
            &t.admin,
            &signer(2),
            CLUSTER,
            replacement,
            [2; 32]
        ),
        Err(Error::IdentityMismatch)
    ));
    // A different node may not claim an existing signing key or binding key.
    let reused = Member {
        node: NodeId([5; 32]),
        ..member(3)
    };
    assert!(matches!(
        membership_state::approve(&mut t.db, 0, &t.admin, &signer(2), CLUSTER, reused, [3; 32]),
        Err(Error::IdentityMismatch)
    ));
    assert_eq!(t.pin(3).unwrap().signing_key, signer(3).public_key());
    let count =
        t.db.transaction(|tx| tx.known_peer_identities())
            .unwrap()
            .len();
    assert_eq!(count, 1);
}

#[test]
fn signed_https_evidence_uses_pins_and_data_requires_active_members() {
    let mut t = Test::new();
    let key = signer(3).public_key();
    // Evidence is allowed for a pinned identity.
    let status = membership::handle(
        &mut t.db,
        &PeerKey::SignedHttps(key),
        &request(3, PeerCommand::MembershipStatus),
    )
    .unwrap();
    assert!(matches!(
        status.reply,
        harmion_protocol::PeerReply::Status { .. }
    ));
    // Data requires both peers Active and the pin to match the effective binding.
    membership::handle(
        &mut t.db,
        &PeerKey::SignedHttps(key),
        &request(3, PeerCommand::Status),
    )
    .unwrap();

    // Retire node 3: the pin stays, evidence still works, data stops.
    membership_state::retire(&mut t.db, 0, &t.admin, &signer(2), member(3).node, [4; 32]).unwrap();
    assert_eq!(t.pin(3).unwrap().signing_key, key);
    membership::handle(
        &mut t.db,
        &PeerKey::SignedHttps(key),
        &request(3, PeerCommand::MembershipStatus),
    )
    .unwrap();
    assert!(matches!(
        membership::handle(
            &mut t.db,
            &PeerKey::SignedHttps(key),
            &request(3, PeerCommand::Status)
        ),
        Err(Error::PeerRetired)
    ));
    // An unproven key never authenticates, even for the same node.
    assert!(matches!(
        membership::handle(
            &mut t.db,
            &PeerKey::SignedHttps(signer(44).public_key()),
            &request(3, PeerCommand::MembershipStatus)
        ),
        Err(Error::Unauthenticated)
    ));
}

#[test]
fn retired_issuer_cannot_poison_a_pinned_control_plane_identity() {
    let mut t = Test::new();
    // Root 2 admits issuer 3 and victim 4, then retires issuer 3 with no retained
    // certificates, cutting everything issuer 3 previously issued.
    let victim = certificate(2, Action::Admit(member(4)));
    t.db.transaction(|tx| membership_state::apply(tx, std::slice::from_ref(&victim)))
        .unwrap();
    let retire = certificate(
        2,
        Action::Retire {
            target: member(3).node,
            retained: Default::default(),
        },
    );
    t.db.transaction(|tx| membership_state::apply(tx, &[retire]))
        .unwrap();
    let pinned = t.pin(4).unwrap();

    // Even a historically rooted issuer can only make a cut-excluded claim.
    let poison = certificate(
        3,
        Action::Admit(Member {
            node: member(4).node,
            signing_key: signer(44).public_key(),
            binding: Binding::Iroh(signer(44).public_key()),
            endpoint: String::new(),
        }),
    );
    t.db.transaction(|tx| membership_state::apply(tx, std::slice::from_ref(&poison)))
        .unwrap();

    // The victim keeps its effective identity and its pin.
    assert_eq!(t.pin(4), Some(pinned.clone()));
    t.db.transaction(|tx| {
        let state = membership_state::state(tx)?.unwrap();
        assert_eq!(state.nodes[&member(4).node], Status::Active(member(4)));
        assert!(state.rooted.contains(&poison.id()), "evidence is retained");
        Ok(())
    })
    .unwrap();

    // Poisoned key cannot authenticate; the pinned victim key still can.
    assert!(matches!(
        membership::handle(
            &mut t.db,
            &PeerKey::SignedHttps(signer(44).public_key()),
            &request(4, PeerCommand::MembershipStatus)
        ),
        Err(Error::Unauthenticated)
    ));
    membership::handle(
        &mut t.db,
        &PeerKey::SignedHttps(pinned.signing_key),
        &request(4, PeerCommand::MembershipStatus),
    )
    .unwrap();
}

#[test]
fn old_database_migration_seeds_only_validated_identities() {
    let path = std::env::temp_dir().join(format!(
        "harmion-pins-legacy-{}-{}",
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
    // A transport registration made before membership existed has no validated
    // signing key, so it must not be imported into the pin registry.
    membership::approve(
        &mut db,
        0,
        &admin,
        CLUSTER,
        Peer {
            node: NodeId([5; 32]),
            key: PeerKey::Iroh([55; 32]),
            endpoint: String::new(),
            retired: false,
        },
    )
    .unwrap();
    membership_state::initialize(&mut db, &signer(2), member(2), true).unwrap();
    membership_state::approve(&mut db, 0, &admin, &signer(2), CLUSTER, member(3), [1; 32]).unwrap();
    // Simulate the schema immediately before the pin registry.
    let sql = rusqlite::Connection::open(path.join("db")).unwrap();
    sql.execute_batch("DROP TABLE identity_pins; DROP TABLE https_state; DROP TABLE https_challenges; DROP TABLE https_rate; PRAGMA user_version=10;")
        .unwrap();
    drop(sql);
    drop(db);

    let mut db = SqliteStore::open(path.join("db"), identity).unwrap();
    let pins = db.transaction(|tx| tx.known_peer_identities()).unwrap();
    assert_eq!(
        pins.len(),
        1,
        "only the effectively Active member is seeded"
    );
    assert_eq!(pins[0].node, member(3).node);
    assert_eq!(pins[0].signing_key, signer(3).public_key());
    assert!(matches!(
        pins[0].provenance,
        Provenance::AcceptedAdmission(_)
    ));
    // Unknown legacy peers stay unknown for signed-HTTPS authentication.
    assert!(matches!(
        membership::handle(
            &mut db,
            &PeerKey::SignedHttps([55; 32]),
            &request(5, PeerCommand::MembershipStatus)
        ),
        Err(Error::Unauthenticated)
    ));
    membership::handle(
        &mut db,
        &PeerKey::SignedHttps(signer(3).public_key()),
        &request(3, PeerCommand::MembershipStatus),
    )
    .unwrap();
    // Reopening a migrated database does not reseed or rewrite pins.
    drop(db);
    let mut db = SqliteStore::open(path.join("db"), identity).unwrap();
    assert_eq!(
        db.transaction(|tx| tx.known_peer_identities())
            .unwrap()
            .len(),
        1
    );
    let _ = std::fs::remove_dir_all(&path);
}

#[test]
fn received_membership_pages_pin_effective_admissions_atomically() {
    let mut t = Test::new();
    let admit = certificate(3, Action::Admit(member(4)));
    let page = Page {
        epoch: LogEpoch([3; 32]),
        after: 0,
        through: 1,
        high: 1,
        entries: vec![LogEntry {
            seq: 1,
            record: admit.clone(),
        }],
    };
    t.db.transaction(|tx| membership_log::receive(tx, member(3).node, &page))
        .unwrap();
    assert_eq!(
        t.pin(4).unwrap().provenance,
        Provenance::AcceptedAdmission(admit.id())
    );
    // A conflicting page that fails validation leaves no partial pin.
    let mut bad = admit.statement().clone();
    let Action::Admit(poisoned) = &mut bad.action else {
        unreachable!()
    };
    poisoned.binding = Binding::Iroh([88; 32]);
    let bad = Certificate::new(bad, [9; 64]).unwrap();
    let page = Page {
        epoch: LogEpoch([3; 32]),
        after: 1,
        through: 2,
        high: 2,
        entries: vec![LogEntry {
            seq: 2,
            record: bad,
        }],
    };
    assert!(
        t.db.transaction(|tx| membership_log::receive(tx, member(3).node, &page))
            .is_err()
    );
    assert_eq!(
        t.pin(4).unwrap().provenance,
        Provenance::AcceptedAdmission(admit.id())
    );
}
