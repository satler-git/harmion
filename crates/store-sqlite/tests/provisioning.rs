//! Host-only deployment provisioning: idempotency, conflict refusal and the
//! owner-supplied root/admission/pin inputs.
use harmion_domain::{ClusterId, NodeId};
use harmion_node::{
    Error,
    identity::Provenance,
    membership_state::{self, MembershipSigner, Status},
    provisioning::{self, Manifest},
    storage::{Identity, Store},
};
use harmion_protocol::{
    LogEpoch,
    membership::{Action, Binding, Certificate, Member, Statement},
};
use harmion_store_sqlite::SqliteStore;

const CLUSTER: ClusterId = ClusterId([1; 32]);
fn signer(n: u8) -> MembershipSigner {
    MembershipSigner::from_bytes(&[n; 32])
}
fn node(n: u8) -> NodeId {
    NodeId([n; 32])
}
fn member(n: u8) -> Member {
    let key = signer(n).public_key();
    Member {
        node: node(n),
        signing_key: key,
        binding: Binding::SignedHttps(key),
        endpoint: format!("https://node{n}.example"),
    }
}
fn identity(n: u8) -> Identity {
    Identity {
        cluster: CLUSTER,
        node: node(n),
        epoch: LogEpoch([n; 32]),
    }
}
fn admit(issuer: u8, target: Member) -> Certificate {
    signer(issuer)
        .sign(Statement {
            cluster: CLUSTER,
            issuer: node(issuer),
            signing_key: signer(issuer).public_key(),
            nonce: [target.node.0[0]; 32],
            action: Action::Admit(target),
        })
        .unwrap()
}
fn root() -> Certificate {
    admit(2, member(2))
}

#[test]
fn provision_is_idempotent_and_conflicts_fail() {
    let path = std::env::temp_dir().join(format!("harmion-provision-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    let mut store = SqliteStore::open(path.join("db"), identity(3)).unwrap();
    let evidence = [admit(2, member(3))];
    let manifest = Manifest {
        own: member(3),
        root: root(),
        evidence: &evidence,
        contacts: &[member(2)],
    };
    provisioning::provision(&mut store, &signer(3), &manifest).unwrap();
    store
        .transaction(|tx| {
            let state = membership_state::state(tx)?.unwrap();
            assert!(matches!(state.nodes[&node(3)], Status::Active(_)));
            assert!(matches!(state.nodes[&node(2)], Status::Active(_)));
            assert_eq!(tx.membership_root()?.unwrap(), root());
            let local = tx.membership_identity()?.unwrap();
            let Action::Admit(local_member) = &local.statement().action else {
                panic!("identity")
            };
            assert_eq!(local_member.node, node(3));
            assert_eq!(local.statement().issuer, node(3));
            let pin = tx.known_peer_identity(node(2))?.unwrap();
            assert_eq!(pin.signing_key, signer(2).public_key());
            assert_eq!(pin.provenance, Provenance::OwnerRegistration);
            assert!(tx.known_peer_identity(node(3))?.is_none());
            Ok(())
        })
        .unwrap();

    // Reapplying the identical manifest is idempotent.
    provisioning::provision(&mut store, &signer(3), &manifest).unwrap();
    // A different root or identity is an owner-visible failure, never a rewrite.
    let other_root = admit(5, member(5));
    assert!(matches!(
        provisioning::provision(
            &mut store,
            &signer(3),
            &Manifest {
                root: other_root,
                ..manifest.clone()
            }
        ),
        Err(Error::IdentityMismatch)
    ));
    let mut changed = member(3);
    changed.binding = Binding::Iroh(signer(3).public_key());
    assert!(matches!(
        provisioning::provision(
            &mut store,
            &signer(3),
            &Manifest {
                own: changed,
                ..manifest.clone()
            }
        ),
        Err(Error::IdentityMismatch)
    ));
    // A wrong seed cannot provision this node.
    assert!(matches!(
        provisioning::provision(&mut store, &signer(7), &manifest),
        Err(Error::IdentityMismatch)
    ));
    let _ = std::fs::remove_dir_all(&path);
}

#[test]
fn provisioning_rejects_invalid_inputs() {
    let path = std::env::temp_dir().join(format!("harmion-provision-bad-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    let mut store = SqliteStore::open(path.join("db"), identity(3)).unwrap();

    // Unauthorized admission: signed by a node that is not rooted.
    let unauthorized = [admit(4, member(3))];
    assert!(matches!(
        provisioning::provision(
            &mut store,
            &signer(3),
            &Manifest {
                own: member(3),
                root: root(),
                evidence: &unauthorized,
                contacts: &[member(2)],
            }
        ),
        Err(Error::InvalidRequest)
    ));
    // Tampered signature fails closed.
    let mut bad = admit(2, member(3));
    let mut signature = *bad.signature();
    signature[0] ^= 1;
    bad = Certificate::new(bad.statement().clone(), signature).unwrap();
    assert!(matches!(
        provisioning::provision(
            &mut store,
            &signer(3),
            &Manifest {
                own: member(3),
                root: root(),
                evidence: &[bad],
                contacts: &[member(2)],
            }
        ),
        Err(Error::InvalidSignature)
    ));
    // Non-canonical signed-HTTPS origin is rejected before storage.
    let mut bad_origin = member(2);
    bad_origin.endpoint = "https://Node2.example".to_owned();
    assert!(matches!(
        provisioning::provision(
            &mut store,
            &signer(3),
            &Manifest {
                own: member(3),
                root: root(),
                evidence: &[],
                contacts: &[bad_origin],
            }
        ),
        Err(Error::Codec(_))
    ));
    // Nothing usable can be provisioned from neither evidence nor contacts.
    assert!(matches!(
        provisioning::provision(
            &mut store,
            &signer(3),
            &Manifest {
                own: member(3),
                root: root(),
                evidence: &[],
                contacts: &[],
            }
        ),
        Err(Error::InvalidRequest)
    ));
    // Bootstrapping before admission is allowed only with pinned contacts.
    let evidence: [Certificate; 0] = [];
    provisioning::provision(
        &mut store,
        &signer(3),
        &Manifest {
            own: member(3),
            root: root(),
            evidence: &evidence,
            contacts: &[member(2)],
        },
    )
    .unwrap();
    store
        .transaction(|tx| {
            let state = membership_state::state(tx)?.unwrap();
            assert!(matches!(state.nodes[&node(3)], Status::Unconfirmed));
            Ok(())
        })
        .unwrap();
    let _ = std::fs::remove_dir_all(&path);
}

struct Entropy(u64);
impl harmion_node::application::Host for Entropy {
    fn random_bytes(&mut self) -> Result<[u8; 32], Error> {
        self.0 += 1;
        Ok([self.0 as u8; 32])
    }
    fn now_micros(&self) -> i64 {
        0
    }
}

#[test]
fn export_manifest_builds_the_root_and_admission_chain() {
    let path = std::env::temp_dir().join(format!("harmion-export-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    let mut store = SqliteStore::open(path.join("db"), identity(2)).unwrap();
    let mut host = Entropy(10);
    let (_, admin) =
        harmion_node::application::bootstrap_admin(&mut store, &mut host, i64::MAX).unwrap();
    membership_state::initialize(&mut store, &signer(2), member(2), true).unwrap();
    membership_state::approve(
        &mut store,
        0,
        &admin,
        &signer(2),
        CLUSTER,
        member(3),
        [7; 32],
    )
    .unwrap();

    let export = provisioning::export_manifest(&mut store, &admin, 0, CLUSTER, member(3)).unwrap();
    assert_eq!(export.own, member(3));
    // The founder's root is its deterministic self-admission (nonce zero), not
    // the helper's synthetic admit certificate.
    assert_eq!(export.root.statement().issuer, node(2));
    assert!(matches!(
        &export.root.statement().action,
        Action::Admit(anchor) if anchor == &member(2)
    ));
    assert_eq!(export.evidence.len(), 1);
    let evidence = &export.evidence[0];
    assert_eq!(evidence.statement().issuer, node(2));
    assert!(matches!(
        &evidence.statement().action,
        Action::Admit(admitted) if admitted == &member(3)
    ));
    assert!(export.contacts.contains(&member(2)));
    assert!(!export.contacts.contains(&member(3)));

    // Another host can be provisioned from exactly this exported material.
    let mut target = SqliteStore::open(path.join("target"), identity(3)).unwrap();
    provisioning::provision(
        &mut target,
        &signer(3),
        &Manifest {
            own: export.own.clone(),
            root: export.root.clone(),
            evidence: &export.evidence,
            contacts: &export.contacts,
        },
    )
    .unwrap();
    target
        .transaction(|tx| {
            let state = membership_state::state(tx)?.unwrap();
            assert!(matches!(state.nodes[&node(3)], Status::Active(_)));
            assert!(matches!(state.nodes[&node(2)], Status::Active(_)));
            Ok(())
        })
        .unwrap();

    // Unapproved, mismatched and unauthenticated requests never export an anchor.
    assert!(matches!(
        provisioning::export_manifest(&mut store, &admin, 0, CLUSTER, member(4)),
        Err(Error::InvalidRequest)
    ));
    let mut changed = member(3);
    changed.endpoint = "https://other.example".to_owned();
    assert!(matches!(
        provisioning::export_manifest(&mut store, &admin, 0, CLUSTER, changed),
        Err(Error::IdentityMismatch)
    ));
    let stranger = harmion_node::access::Credential::from_bytes([0; 32]);
    assert!(provisioning::export_manifest(&mut store, &stranger, 0, CLUSTER, member(3)).is_err());
    assert!(matches!(
        provisioning::export_manifest(&mut store, &admin, 0, ClusterId([9; 32]), member(3)),
        Err(Error::InvalidRequest)
    ));

    let _ = std::fs::remove_dir_all(&path);
}
