use harmion_domain::{ClusterId, NodeId};
use harmion_node::{
    Error,
    membership_state::{self, MembershipSigner, Status},
};
use harmion_protocol::membership::{Action, Binding, Certificate, Member, Statement};
use std::collections::BTreeSet;

const CLUSTER: ClusterId = ClusterId([1; 32]);
fn member(n: u8) -> Member {
    let key = MembershipSigner::from_bytes(&[n; 32]).public_key();
    Member {
        node: NodeId([n; 32]),
        signing_key: key,
        binding: Binding::Iroh(key),
        endpoint: String::new(),
    }
}
fn signed(n: u8, nonce: u8, action: Action) -> Certificate {
    let signer = MembershipSigner::from_bytes(&[n; 32]);
    signer
        .sign(Statement {
            cluster: CLUSTER,
            issuer: member(n).node,
            signing_key: signer.public_key(),
            nonce: [nonce; 32],
            action,
        })
        .unwrap()
}
fn admit(from: u8, to: u8) -> Certificate {
    signed(from, to, Action::Admit(member(to)))
}
fn retire(from: u8, to: u8, retained: &[&Certificate]) -> Certificate {
    signed(
        from,
        99,
        Action::Retire {
            target: member(to).node,
            retained: retained.iter().map(|c| c.id()).collect(),
        },
    )
}
fn evaluate(records: &[Certificate]) -> membership_state::Membership {
    membership_state::evaluate(CLUSTER, &[member(2)], records).unwrap()
}

#[test]
fn admission_is_rooted_and_signatures_bind_every_field() {
    let ab = admit(2, 3);
    let bc = admit(3, 4);
    assert!(
        !evaluate(std::slice::from_ref(&bc))
            .nodes
            .contains_key(&member(4).node)
    );
    assert!(matches!(
        evaluate(&[bc, ab.clone()]).nodes[&member(4).node],
        Status::Active(_)
    ));
    assert_eq!(
        evaluate(&[admit(3, 4), admit(4, 3)]).confirmed,
        BTreeSet::new()
    );
    let mut forged = ab.statement().clone();
    forged.nonce[0] ^= 1;
    assert!(matches!(
        membership_state::verify(CLUSTER, &Certificate::new(forged, *ab.signature()).unwrap()),
        Err(Error::InvalidSignature)
    ));
    assert!(matches!(
        membership_state::verify(ClusterId([9; 32]), &ab),
        Err(Error::IdentityMismatch)
    ));
    for len in 0..ab.bytes().len() {
        assert!(Certificate::from_bytes(&ab.bytes()[..len]).is_err());
    }
    assert_eq!(Certificate::from_bytes(ab.bytes()).unwrap(), ab);
}

#[test]
fn retirement_preserves_its_cut_and_reapproval_needs_an_active_issuer() {
    let ab = admit(2, 3);
    let bc = admit(3, 4);
    let bd = admit(3, 5);
    let rb = retire(2, 3, &[&bc]);
    let records = vec![ab, bc, bd, rb];
    let expected = evaluate(&records);
    assert_eq!(expected.nodes[&member(3).node], Status::Retired);
    assert!(matches!(expected.nodes[&member(4).node], Status::Active(_)));
    assert!(!matches!(
        expected.nodes.get(&member(5).node),
        Some(Status::Active(_))
    ));
    // Full permutation coverage, including a cut arriving before the retained proof.
    for a in 0..4 {
        for b in 0..4 {
            for c in 0..4 {
                for d in 0..4 {
                    if BTreeSet::from([a, b, c, d]).len() == 4 {
                        assert_eq!(
                            evaluate(&[
                                records[a].clone(),
                                records[b].clone(),
                                records[c].clone(),
                                records[d].clone()
                            ]),
                            expected
                        );
                    }
                }
            }
        }
    }
    let mut recovered = records;
    recovered.push(admit(4, 5));
    assert!(matches!(
        evaluate(&recovered).nodes[&member(5).node],
        Status::Active(_)
    ));
}

#[test]
fn mutual_retirement_is_held_until_an_independent_member_decides() {
    let ab = admit(2, 3);
    let ac = admit(2, 4);
    let rb = retire(2, 3, &[]);
    let ra = retire(3, 2, &[&ab, &ac]);
    let ra_id = ra.id();
    let mut records = vec![ab, ac, rb, ra];
    let held = evaluate(&records);
    assert_eq!(held.nodes[&member(2).node], Status::Conflict);
    assert_eq!(held.nodes[&member(3).node], Status::Conflict);
    assert!(matches!(held.nodes[&member(4).node], Status::Active(_)));
    records.reverse();
    assert_eq!(evaluate(&records), held);
    // C confirms B's retirement, excluding B's contested retirement of A.
    records.push(retire(4, 3, &[]));
    records.push(signed(
        4,
        100,
        Action::Dismiss {
            retirements: [ra_id].into(),
        },
    ));
    let resolved = evaluate(&records);
    assert_eq!(resolved.nodes[&member(3).node], Status::Retired);
    assert!(matches!(resolved.nodes[&member(2).node], Status::Active(_)));
}

#[test]
fn an_unseen_retirement_by_a_retired_key_cannot_reactivate_that_key() {
    let ab = admit(2, 3);
    let rb = retire(2, 3, &[]);
    let forged_old_retirement = retire(3, 2, &[&ab]);
    let before = evaluate(&[ab.clone(), rb.clone()]);
    assert_eq!(before.nodes[&member(3).node], Status::Retired);
    let after = evaluate(&[ab, rb, forged_old_retirement]);
    assert_eq!(after.nodes[&member(3).node], Status::Conflict);
    assert!(!matches!(after.nodes[&member(3).node], Status::Active(_)));
}

#[test]
fn multiple_retirements_do_not_expand_the_retained_set() {
    let ab = admit(2, 3);
    let ac = admit(2, 4);
    let bd = admit(3, 5);
    let first = retire(2, 3, &[]);
    let second = retire(4, 3, &[&bd]);
    let state = evaluate(&[ab, ac, bd, first, second]);
    assert!(!matches!(
        state.nodes.get(&member(5).node),
        Some(Status::Active(_))
    ));
}

#[test]
fn losing_a_retirement_issuer_holds_the_target_until_explicit_decision() {
    let ac = admit(2, 3);
    let ab = admit(2, 4);
    let cb = retire(3, 4, &[]);
    let rc = retire(2, 3, &[]);
    let mut records = vec![ac, ab, cb.clone()];
    assert_eq!(evaluate(&records).nodes[&member(4).node], Status::Retired);
    records.push(rc);
    let pending = evaluate(&records);
    assert_eq!(pending.nodes[&member(4).node], Status::Conflict);
    assert!(pending.pending_retirements.contains_key(&cb.id()));
    records.reverse();
    assert_eq!(evaluate(&records), pending);
    // The held subject cannot dismiss its own evidence or create a new authority.
    records.push(signed(
        4,
        101,
        Action::Dismiss {
            retirements: [cb.id()].into(),
        },
    ));
    records.push(admit(4, 5));
    assert_eq!(evaluate(&records).nodes[&member(4).node], Status::Conflict);
    assert!(!matches!(
        evaluate(&records).nodes.get(&member(5).node),
        Some(Status::Active(_))
    ));
    records.push(signed(
        2,
        102,
        Action::Dismiss {
            retirements: [cb.id()].into(),
        },
    ));
    assert!(matches!(
        evaluate(&records).nodes[&member(4).node],
        Status::Active(_)
    ));
}

#[test]
fn signed_proofs_cannot_replace_a_node_key_or_alias_another_identity() {
    let ab = admit(2, 3);
    let mut replacement = member(3);
    replacement.signing_key = member(4).signing_key;
    assert!(matches!(
        membership_state::evaluate(
            CLUSTER,
            &[member(2)],
            &[ab.clone(), signed(2, 111, Action::Admit(replacement))]
        ),
        Err(Error::IdentityMismatch)
    ));
    let mut alias = member(4);
    alias.binding = member(3).binding;
    assert!(matches!(
        membership_state::evaluate(
            CLUSTER,
            &[member(2)],
            &[ab, signed(2, 112, Action::Admit(alias))]
        ),
        Err(Error::IdentityMismatch)
    ));
}

fn signed_https_member(n: u8) -> Member {
    let key = MembershipSigner::from_bytes(&[n; 32]).public_key();
    Member {
        node: NodeId([n; 32]),
        signing_key: key,
        binding: Binding::SignedHttps(key),
        endpoint: "https://replica.example".to_owned(),
    }
}

#[test]
fn signed_https_binding_key_must_equal_signing_key_and_is_unique() {
    // The typed binding repeats the membership key; a different key is not a
    // second credential and cannot be encoded at all.
    let mut wrong = signed_https_member(4);
    wrong.binding = Binding::SignedHttps([77; 32]);
    assert!(
        MembershipSigner::from_bytes(&[3; 32])
            .sign(Statement {
                cluster: CLUSTER,
                issuer: member(3).node,
                signing_key: MembershipSigner::from_bytes(&[3; 32]).public_key(),
                nonce: [1; 32],
                action: Action::Admit(wrong),
            })
            .is_err()
    );

    // Two Node IDs may not share the same signed-HTTPS binding key.
    let shared = MembershipSigner::from_bytes(&[4; 32]).public_key();
    let first = Member {
        node: NodeId([4; 32]),
        signing_key: shared,
        binding: Binding::SignedHttps(shared),
        endpoint: String::new(),
    };
    let second = Member {
        node: NodeId([5; 32]),
        signing_key: shared,
        binding: Binding::SignedHttps(shared),
        endpoint: String::new(),
    };
    let admit_first = signed(3, 1, Action::Admit(first));
    let admit_second = signed(3, 2, Action::Admit(second));
    assert!(matches!(
        membership_state::evaluate(
            CLUSTER,
            &[member(2)],
            &[admit(2, 3), admit_first, admit_second]
        ),
        Err(Error::IdentityMismatch)
    ));
}
