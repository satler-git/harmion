//! Membership is derived from signed records, never from receipt order.
use crate::Error;
use crate::{
    access::{Credential, authenticate},
    membership::{Peer, PeerKey},
    storage::{Store, Transaction},
};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use harmion_domain::{ClusterId, NodeId};
use harmion_protocol::membership::{Action, Certificate, CertificateId, Member, Statement};
use std::collections::{BTreeMap, BTreeSet};

/// Host-owned secret. Debug intentionally omitted. It is not a transport address.
pub struct MembershipSigner(SigningKey);
impl MembershipSigner {
    pub fn from_bytes(seed: &[u8; 32]) -> Self {
        Self(SigningKey::from_bytes(seed))
    }
    pub fn public_key(&self) -> [u8; 32] {
        self.0.verifying_key().to_bytes()
    }
    /// Crate-internal access for the constrained typed signing operations in
    /// [`crate::https_peer`]; never exposed outside the crate.
    pub(crate) fn key(&self) -> &SigningKey {
        &self.0
    }
    pub fn sign(&self, statement: Statement) -> Result<Certificate, Error> {
        if statement.signing_key != self.public_key() {
            return Err(Error::IdentityMismatch);
        }
        let signature = self.0.sign(&statement.signing_bytes()?).to_bytes();
        Ok(Certificate::new(statement, signature)?)
    }
}

pub fn verify(cluster: ClusterId, certificate: &Certificate) -> Result<(), Error> {
    if certificate.statement().cluster != cluster {
        return Err(Error::IdentityMismatch);
    }
    VerifyingKey::from_bytes(&certificate.statement().signing_key)
        .map_err(|_| Error::InvalidSignature)?
        .verify_strict(
            certificate.signed_bytes(),
            &Signature::from_bytes(certificate.signature()),
        )
        .map_err(|_| Error::InvalidSignature)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Active(Member),
    Retired,
    /// Includes mutual retirement and admissions dependent on such a conflict.
    Conflict,
    Unconfirmed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Membership {
    pub nodes: BTreeMap<NodeId, Status>,
    pub confirmed: BTreeSet<CertificateId>,
    pub rooted: BTreeSet<CertificateId>,
    pub pending_retirements: BTreeMap<CertificateId, (NodeId, NodeId)>,
    pub dismissed: BTreeSet<CertificateId>,
}

type Principal = (NodeId, [u8; 32]);

/// Find the least rooted admission closure under a fixed set of cuts.
/// Unknown self-signed cycles cannot create an authority.
fn closure(
    roots: &[Member],
    records: &BTreeMap<CertificateId, &Certificate>,
    assumed: &BTreeSet<CertificateId>,
    dismissed: &BTreeSet<CertificateId>,
) -> (BTreeSet<CertificateId>, BTreeSet<Principal>) {
    let mut admitted: BTreeSet<_> = roots.iter().map(|m| (m.node, m.signing_key)).collect();
    let mut valid = BTreeSet::new();
    let cuts: Vec<_> = assumed
        .iter()
        .filter_map(|id| match &records[id].statement().action {
            Action::Retire { target, retained } => Some((*target, retained)),
            _ => None,
        })
        .collect();
    loop {
        let before = valid.len();
        for (id, record) in records {
            if dismissed.contains(id) {
                continue;
            }
            let statement = record.statement();
            if admitted.contains(&(statement.issuer, statement.signing_key))
                && cuts
                    .iter()
                    .all(|(target, retained)| *target != statement.issuer || retained.contains(id))
            {
                valid.insert(*id);
                if let Action::Admit(member) = &statement.action {
                    admitted.insert((member.node, member.signing_key));
                }
            }
        }
        if valid.len() == before {
            return (valid, admitted);
        }
    }
}

/// A rooted but unauthorized retirement stays a barrier until an independent
/// authorized issuer dismisses it. Losing its issuer must never reactivate its
/// target automatically. Dismissals are applied in simultaneous monotone rounds
/// from the most restrictive state, so circular self-authorization cannot start.
pub fn evaluate(
    cluster: ClusterId,
    roots: &[Member],
    certificates: &[Certificate],
) -> Result<Membership, Error> {
    let mut records = BTreeMap::new();
    for certificate in certificates {
        verify(cluster, certificate)?;
        if let Some(previous) = records.insert(certificate.id(), certificate)
            && previous != certificate
        {
            return Err(Error::IdentityMismatch);
        }
    }
    let (historical, _) = closure(roots, &records, &BTreeSet::new(), &BTreeSet::new());
    let retirement_ids: BTreeSet<_> = historical
        .iter()
        .filter(|id| matches!(records[*id].statement().action, Action::Retire { .. }))
        .copied()
        .collect();
    let mut dismissed = BTreeSet::new();
    let mut accepted_dismissals = BTreeSet::new();
    let (mut lower, admitted, barriers) = loop {
        let barriers: BTreeSet<_> = retirement_ids.difference(&dismissed).copied().collect();
        let (valid, admitted) = closure(roots, &records, &barriers, &dismissed);
        let mut next = dismissed.clone();
        for id in &valid {
            let statement = records[id].statement();
            if let Action::Dismiss { retirements } = &statement.action
                && retirements.iter().all(|target| retirement_ids.contains(target) && !valid.contains(target) && matches!(records[target].statement().action, Action::Retire { target: node, .. } if node != statement.issuer)) {
                    next.extend(retirements);
                    accepted_dismissals.insert(*id);
            }
        }
        if next == dismissed {
            break (valid, admitted, barriers);
        }
        dismissed = next;
    };
    lower.retain(|id| {
        !matches!(records[id].statement().action, Action::Dismiss { .. })
            || accepted_dismissals.contains(id)
    });
    let retired = |ids: &BTreeSet<CertificateId>| -> BTreeSet<NodeId> {
        ids.iter()
            .filter_map(|id| match records[id].statement().action {
                Action::Retire { target, .. } => Some(target),
                _ => None,
            })
            .collect()
    };
    let definitely_retired = retired(&lower);
    let possibly_retired = retired(&barriers);
    let mut members: BTreeMap<NodeId, Member> = BTreeMap::new();
    for member in
        roots.iter().chain(
            lower
                .iter()
                .filter_map(|id| match &records[id].statement().action {
                    Action::Admit(member) => Some(member),
                    _ => None,
                }),
        )
    {
        if let Some(previous) = members.get(&member.node) {
            if previous.signing_key != member.signing_key || previous.binding != member.binding {
                return Err(Error::IdentityMismatch);
            }
        } else {
            if members.values().any(|previous| {
                previous.signing_key == member.signing_key || previous.binding == member.binding
            }) {
                return Err(Error::IdentityMismatch);
            }
            members.insert(member.node, member.clone());
        }
    }
    let mut nodes = BTreeMap::new();
    for (node, member) in members {
        let status = if definitely_retired.contains(&node) {
            Status::Retired
        } else if possibly_retired.contains(&node) {
            Status::Conflict
        } else if admitted.contains(&(node, member.signing_key)) {
            Status::Active(member)
        } else {
            Status::Unconfirmed
        };
        nodes.insert(node, status);
    }
    // Targets may be learned before the matching admission proof.
    for node in definitely_retired {
        nodes.insert(node, Status::Retired);
    }
    for node in possibly_retired {
        nodes.entry(node).or_insert(Status::Conflict);
    }
    for id in &historical {
        if let Action::Admit(member) = &records[id].statement().action {
            nodes.entry(member.node).or_insert(Status::Unconfirmed);
        }
    }
    let pending_retirements = barriers
        .difference(&lower)
        .map(|id| {
            let statement = records[id].statement();
            let Action::Retire { target, .. } = statement.action else {
                unreachable!("retirement IDs")
            };
            (*id, (statement.issuer, target))
        })
        .collect();
    Ok(Membership {
        nodes,
        confirmed: lower,
        rooted: historical,
        pending_retirements,
        dismissed,
    })
}

pub fn state(tx: &mut dyn Transaction) -> Result<Option<Membership>, Error> {
    let Some(local) = tx.membership_identity()? else {
        if !tx.membership_entries()?.is_empty()
            || tx.membership_root()?.is_some()
            || !tx.recovery_roots()?.is_empty()
        {
            return Err(Error::CorruptLog);
        }
        return Ok(None);
    };
    verify(tx.identity().cluster, &local)?;
    let Action::Admit(own_member) = &local.statement().action else {
        return Err(Error::CorruptLog);
    };
    if own_member.node != tx.identity().node
        || local.statement().issuer != own_member.node
        || local.statement().signing_key != own_member.signing_key
    {
        return Err(Error::CorruptLog);
    }
    let Some(root) = tx.membership_root()? else {
        for entry in tx.membership_entries()? {
            verify(tx.identity().cluster, &entry.record)?;
        }
        return Ok(Some(Membership {
            nodes: [(own_member.node, Status::Unconfirmed)].into(),
            confirmed: BTreeSet::new(),
            rooted: BTreeSet::new(),
            pending_retirements: BTreeMap::new(),
            dismissed: BTreeSet::new(),
        }));
    };
    verify(tx.identity().cluster, &root)?;
    let Action::Admit(member) = &root.statement().action else {
        return Err(Error::CorruptLog);
    };
    if root.statement().issuer != member.node || root.statement().signing_key != member.signing_key
    {
        return Err(Error::CorruptLog);
    }
    let records: Vec<_> = tx
        .membership_entries()?
        .into_iter()
        .map(|entry| entry.record)
        .collect();
    let mut roots = vec![member.clone()];
    for recovery in tx.recovery_roots()? {
        verify(tx.identity().cluster, &recovery)?;
        if recovery.statement().issuer != own_member.node
            || recovery.statement().signing_key != own_member.signing_key
        {
            return Err(Error::CorruptLog);
        }
        let Action::Admit(recovery_member) = &recovery.statement().action else {
            return Err(Error::CorruptLog);
        };
        roots.push(recovery_member.clone());
    }
    let mut result = evaluate(tx.identity().cluster, &roots, &records)?;
    for record in &records {
        if result.rooted.contains(&record.id())
            && let Action::Admit(candidate) = &record.statement().action
            && candidate.node == own_member.node
            && (candidate.signing_key != own_member.signing_key
                || candidate.binding != own_member.binding)
        {
            return Err(Error::IdentityMismatch);
        }
    }
    result
        .nodes
        .entry(own_member.node)
        .or_insert(Status::Unconfirmed);
    for peer in tx.peers()? {
        result.nodes.entry(peer.node).or_insert(Status::Unconfirmed);
    }
    Ok(Some(result))
}

pub fn require_active(state: &Membership, node: NodeId) -> Result<&Member, Error> {
    match state.nodes.get(&node) {
        Some(Status::Active(member)) => Ok(member),
        Some(Status::Retired) => Err(Error::PeerRetired),
        Some(Status::Conflict) => Err(Error::MembershipConflict),
        _ => Err(Error::Unauthenticated),
    }
}

/// Local and remote must both be Active, and the routing projection must agree
/// with the effective binding. Returns the effective member for identity checks.
pub fn active_member(tx: &mut dyn Transaction, node: NodeId) -> Result<Member, Error> {
    let state = state(tx)?.ok_or(Error::Unauthenticated)?;
    require_active(&state, tx.identity().node)?;
    let member = require_active(&state, node)?.clone();
    let peer = tx.peer(node)?.ok_or(Error::Unauthenticated)?;
    if peer.retired {
        return Err(Error::PeerRetired);
    }
    if as_peer(&member).key != peer.key {
        return Err(Error::IdentityMismatch);
    }
    Ok(member)
}

pub fn as_peer(member: &Member) -> Peer {
    Peer {
        node: member.node,
        key: match &member.binding {
            harmion_protocol::membership::Binding::Https(bytes) => {
                PeerKey::TlsCertificate(bytes.clone())
            }
            harmion_protocol::membership::Binding::Iroh(key) => PeerKey::Iroh(*key),
            harmion_protocol::membership::Binding::SignedHttps(key) => PeerKey::SignedHttps(*key),
        },
        endpoint: member.endpoint.clone(),
        retired: false,
    }
}

/// Pin the exact signing key and binding of every effectively Active remote
/// member in the same transaction that accepts the admission. An existing pin is
/// never overwritten; an explicit owner pin inserted earlier is preserved.
fn pin_active_identities(tx: &mut dyn Transaction, membership: &Membership) -> Result<(), Error> {
    let root = tx.membership_root()?;
    let recovery_roots = tx.recovery_roots()?;
    let root_id = root.as_ref().map(|root| root.id());
    let recovery: BTreeSet<CertificateId> = recovery_roots.iter().map(|root| root.id()).collect();
    let entries = tx.membership_entries()?;
    for (node, status) in &membership.nodes {
        let Status::Active(member) = status else {
            continue;
        };
        if *node == tx.identity().node {
            continue;
        }
        let certificate = entries
            .iter()
            .filter(|entry| membership.confirmed.contains(&entry.record.id()))
            .find_map(|entry| match &entry.record.statement().action {
                Action::Admit(candidate)
                    if candidate.node == member.node
                        && candidate.signing_key == member.signing_key
                        && candidate.binding == member.binding =>
                {
                    Some(entry.record.id())
                }
                _ => None,
            });
        let provenance = match certificate {
            Some(certificate) if Some(certificate) == root_id => {
                crate::identity::Provenance::OwnerRegistration
            }
            Some(certificate) if recovery.contains(&certificate) => {
                crate::identity::Provenance::OwnerRecovery
            }
            Some(certificate) => crate::identity::Provenance::AcceptedAdmission(certificate),
            // A trust anchor can be effective from durable root state alone.
            None if anchor_matches(root.as_ref(), member) => {
                crate::identity::Provenance::OwnerRegistration
            }
            None if recovery_roots
                .iter()
                .any(|root| anchor_matches(Some(root), member)) =>
            {
                crate::identity::Provenance::OwnerRecovery
            }
            None => return Err(Error::CorruptLog),
        };
        crate::identity::pin(
            tx,
            member.node,
            member.signing_key,
            as_peer(member).key,
            provenance,
        )?;
    }
    Ok(())
}

fn anchor_matches(certificate: Option<&Certificate>, member: &Member) -> bool {
    certificate.is_some_and(|certificate| {
        matches!(
            &certificate.statement().action,
            Action::Admit(anchor)
                if anchor.node == member.node
                    && anchor.signing_key == member.signing_key
                    && anchor.binding == member.binding
        )
    })
}

/// The peer table is a routing projection; certificates remain the authority.
pub fn apply(tx: &mut dyn Transaction, records: &[Certificate]) -> Result<Membership, Error> {
    for certificate in records {
        verify(tx.identity().cluster, certificate)?;
        tx.append_membership(certificate)?;
    }
    let result = state(tx)?.ok_or(Error::InvalidRequest)?;
    pin_active_identities(tx, &result)?;
    for status in result.nodes.values() {
        if let Status::Active(member) = status {
            if member.node == tx.identity().node {
                continue;
            }
            let mut peer = as_peer(member);
            if let Some(previous) = tx.peer(peer.node)? {
                if previous.key != peer.key {
                    return Err(Error::IdentityMismatch);
                }
                // Received old certificates must not overwrite a local route update.
                peer.endpoint = previous.endpoint;
            }
            if tx
                .peers()?
                .iter()
                .any(|p| p.node != peer.node && p.key == peer.key)
            {
                return Err(Error::IdentityMismatch);
            }
            tx.put_peer(&peer)?;
        }
    }
    for mut peer in tx.peers()? {
        peer.retired = matches!(result.nodes.get(&peer.node), Some(Status::Retired));
        tx.put_peer(&peer)?;
    }
    Ok(result)
}

pub fn initialize(
    store: &mut impl Store,
    signer: &MembershipSigner,
    member: Member,
    founder: bool,
) -> Result<(), Error> {
    store.transaction(|tx| {
        if member.node != tx.identity().node || member.signing_key != signer.public_key() {
            return Err(Error::IdentityMismatch);
        }
        if let Some(existing) = tx.membership_identity()? {
            let Action::Admit(previous) = &existing.statement().action else {
                return Err(Error::CorruptLog);
            };
            if previous.node != member.node
                || previous.signing_key != member.signing_key
                || previous.binding != member.binding
            {
                return Err(Error::IdentityMismatch);
            }
        } else {
            let root = signer.sign(Statement {
                cluster: tx.identity().cluster,
                issuer: member.node,
                signing_key: signer.public_key(),
                nonce: [0; 32],
                action: Action::Admit(member),
            })?;
            tx.set_membership_identity(&root)?;
            if founder {
                tx.set_membership_root(&root)?;
            }
            tx.append_membership(&root)?;
            // Existing local tombstones must survive migration; no unknown key is
            // automatically promoted to a signed admission.
            for peer in tx.peers()?.into_iter().filter(|p| p.retired) {
                let certificate = signer.sign(Statement {
                    cluster: tx.identity().cluster,
                    issuer: tx.identity().node,
                    signing_key: signer.public_key(),
                    nonce: [0; 32],
                    action: Action::Retire {
                        target: peer.node,
                        retained: BTreeSet::new(),
                    },
                })?;
                tx.append_membership(&certificate)?;
            }
        }
        if founder && tx.membership_root()?.is_none() {
            let root = tx.membership_identity()?.ok_or(Error::CorruptLog)?;
            tx.set_membership_root(&root)?;
        }
        apply(tx, &[])?;
        Ok(())
    })
}

/// Owner verifies the contact registration out of band. The common anchor is
/// adopted once; arbitrary remote certificates can never replace it.
pub fn join(
    store: &mut impl Store,
    now: i64,
    credential: &Credential,
    cluster: ClusterId,
    contact: Member,
    root: Certificate,
    recovery: bool,
) -> Result<(), Error> {
    store.transaction(|tx| {
        if !authenticate(tx, credential, now)?.app.grants.manage_nodes {
            return Err(Error::Forbidden);
        }
        if cluster != tx.identity().cluster || contact.node == tx.identity().node {
            return Err(Error::IdentityMismatch);
        }
        verify(cluster, &root)?;
        let Action::Admit(anchor) = &root.statement().action else {
            return Err(Error::InvalidRequest);
        };
        if root.statement().issuer != anchor.node
            || root.statement().signing_key != anchor.signing_key
        {
            return Err(Error::InvalidRequest);
        }
        let local = tx.membership_identity()?.ok_or(Error::InvalidRequest)?;
        if let Some(existing) = tx.membership_root()? {
            if existing != root || (recovery && !tx.recovery_roots()?.contains(&local)) {
                return Err(Error::InvalidRequest);
            }
            // Retrying a lost ordinary join response must not overwrite trust.
            return Ok(());
        }
        tx.set_membership_root(&root)?;
        tx.append_membership(&root)?;
        // The owner verified this registration out of band; pin both the trust
        // anchor and the explicitly supplied contact identity.
        for verified in [anchor, &contact] {
            crate::identity::pin(
                tx,
                verified.node,
                verified.signing_key,
                as_peer(verified).key,
                crate::identity::Provenance::OwnerRegistration,
            )?;
        }
        if recovery {
            tx.insert_recovery_root(&local)?;
        }
        let peer = as_peer(&contact);
        if let Some(previous) = tx.peer(contact.node)?
            && (previous.key != peer.key || previous.retired)
        {
            return Err(Error::IdentityMismatch);
        }
        tx.put_peer(&peer)?;
        apply(tx, &[])?;
        Ok(())
    })
}

pub fn authorize(
    tx: &mut dyn Transaction,
    credential: &Credential,
    now: i64,
    signer: &MembershipSigner,
) -> Result<Membership, Error> {
    if !authenticate(tx, credential, now)?.app.grants.manage_nodes {
        return Err(Error::Forbidden);
    }
    let current = state(tx)?.ok_or(Error::InvalidRequest)?;
    if require_active(&current, tx.identity().node)?.signing_key != signer.public_key() {
        return Err(Error::IdentityMismatch);
    }
    Ok(current)
}

pub fn inspect(
    store: &mut impl Store,
    credential: &Credential,
    now: i64,
) -> Result<Membership, Error> {
    store.transaction(|tx| {
        if !authenticate(tx, credential, now)?.app.grants.manage_nodes {
            return Err(Error::Forbidden);
        }
        state(tx)?.ok_or(Error::InvalidRequest)
    })
}

/// Explicit local owner recovery, NOT a remotely transferable trust assertion.
/// When no independent authority remains, every affected node must verify the
/// recovery key out of band. Ordinary certificate relay never adds a root.
pub fn trust_recovery_peer(
    store: &mut impl Store,
    now: i64,
    credential: &Credential,
    signer: &MembershipSigner,
    cluster: ClusterId,
    member: Member,
    nonce: [u8; 32],
) -> Result<(), Error> {
    store.transaction(|tx| {
        if !authenticate(tx, credential, now)?.app.grants.manage_nodes {
            return Err(Error::Forbidden);
        }
        if cluster != tx.identity().cluster {
            return Err(Error::IdentityMismatch);
        }
        let current = state(tx)?.ok_or(Error::InvalidRequest)?;
        let own_status = current.nodes.get(&tx.identity().node);
        if tx.membership_root()?.is_none()
            || matches!(own_status, Some(Status::Retired))
            || (current.pending_retirements.is_empty() && !matches!(own_status, Some(Status::Unconfirmed))) {
            return Err(Error::InvalidRequest);
        }
        if member.node == tx.identity().node
            || matches!(current.nodes.get(&member.node), Some(Status::Active(_) | Status::Retired))
            || tx.membership_entries()?.iter().any(|entry| matches!(entry.record.statement().action, Action::Retire { target, .. } if target == member.node))
        {
            return Err(Error::InvalidRequest);
        }
        crate::identity::pin(
            tx,
            member.node,
            member.signing_key,
            as_peer(&member).key,
            crate::identity::Provenance::OwnerRecovery,
        )?;
        let certificate = signer.sign(Statement {
            cluster,
            issuer: tx.identity().node,
            signing_key: signer.public_key(),
            nonce,
            action: Action::Admit(member),
        })?;
        tx.insert_recovery_root(&certificate)?;
        apply(tx, &[certificate])?;
        Ok(())
    })
}

pub fn approve(
    store: &mut impl Store,
    now: i64,
    credential: &Credential,
    signer: &MembershipSigner,
    cluster: ClusterId,
    member: Member,
    nonce: [u8; 32],
) -> Result<(), Error> {
    store.transaction(|tx| {
        let current = authorize(tx, credential, now, signer)?;
        if cluster != tx.identity().cluster || member.node == tx.identity().node {
            return Err(Error::IdentityMismatch);
        }
        if matches!(current.nodes.get(&member.node), Some(Status::Retired)) {
            return Err(Error::PeerRetired);
        }
        let peer = as_peer(&member);
        if let Some(previous) = tx.peer(peer.node)?
            && previous.key != peer.key
        {
            return Err(Error::IdentityMismatch);
        }
        // Explicit owner registration pins possession before the signed admission;
        // `apply` accepts the identical pin and never rewrites it.
        crate::identity::pin(
            tx,
            member.node,
            member.signing_key,
            peer.key.clone(),
            crate::identity::Provenance::OwnerRegistration,
        )?;
        let certificate = signer.sign(Statement {
            cluster,
            issuer: tx.identity().node,
            signing_key: signer.public_key(),
            nonce,
            action: Action::Admit(member),
        })?;
        apply(tx, &[certificate])?;
        // Owner-supplied route change, separate from received historical proofs.
        tx.put_peer(&peer)
    })
}

pub fn retire(
    store: &mut impl Store,
    now: i64,
    credential: &Credential,
    signer: &MembershipSigner,
    node: NodeId,
    nonce: [u8; 32],
) -> Result<(), Error> {
    store.transaction(|tx| {
        let current = authorize(tx, credential, now, signer)?;
        if node == tx.identity().node {
            return Err(Error::InvalidRequest);
        }
        if !current.nodes.contains_key(&node) && tx.peer(node)?.is_none() {
            return Err(Error::NotFound);
        }
        let retained = tx
            .membership_entries()?
            .into_iter()
            .filter(|entry| {
                entry.record.statement().issuer == node
                    && current.confirmed.contains(&entry.record.id())
            })
            .map(|entry| entry.record.id())
            .collect();
        let certificate = signer.sign(Statement {
            cluster: tx.identity().cluster,
            issuer: tx.identity().node,
            signing_key: signer.public_key(),
            nonce,
            action: Action::Retire {
                target: node,
                retained,
            },
        })?;
        apply(tx, &[certificate])?;
        Ok(())
    })
}

pub fn resolve_retirement(
    store: &mut impl Store,
    now: i64,
    credential: &Credential,
    signer: &MembershipSigner,
    id: CertificateId,
    confirm: bool,
    nonce: [u8; 32],
) -> Result<(), Error> {
    store.transaction(|tx| {
        let current = authorize(tx, credential, now, signer)?;
        let (_, target) = *current
            .pending_retirements
            .get(&id)
            .ok_or(Error::InvalidRequest)?;
        let mut certificates = Vec::new();
        if confirm {
            let retained = tx
                .membership_entries()?
                .into_iter()
                .filter(|entry| {
                    entry.record.statement().issuer == target
                        && current.confirmed.contains(&entry.record.id())
                })
                .map(|entry| entry.record.id())
                .collect();
            certificates.push(signer.sign(Statement {
                cluster: tx.identity().cluster,
                issuer: tx.identity().node,
                signing_key: signer.public_key(),
                nonce,
                action: Action::Retire { target, retained },
            })?);
        }
        certificates.push(signer.sign(Statement {
            cluster: tx.identity().cluster,
            issuer: tx.identity().node,
            signing_key: signer.public_key(),
            nonce,
            action: Action::Dismiss {
                retirements: [id].into(),
            },
        })?);
        apply(tx, &certificates)?;
        Ok(())
    })
}
