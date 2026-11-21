//! Durable authentication identity pins, separate from current membership grants.
//!
//! A pin records which signing key and transport binding a Node ID was verified
//! with. Pins are kept after retirement or conflict: they establish possession for
//! signed evidence transport, not current permission to serve data. They are never
//! rewritten in place and a key is never reused for another Node ID.
use crate::Error;
use crate::membership::PeerKey;
use crate::storage::Transaction;
use harmion_domain::NodeId;
use harmion_protocol::membership::CertificateId;

/// How a pin was established. Only explicit owner actions and effectively
/// authorized admissions may create one; raw historical claims never do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provenance {
    /// Explicit local owner registration (approve/join/trusted contact).
    OwnerRegistration,
    /// Explicit local owner recovery authority.
    OwnerRecovery,
    /// Admission that was effective in the transaction where it was accepted.
    AcceptedAdmission(CertificateId),
    /// Imported from a pre-pin schema's validated peer/membership state.
    LegacyImport,
}

impl Provenance {
    pub fn kind(self) -> u8 {
        match self {
            Self::OwnerRegistration => 0,
            Self::OwnerRecovery => 1,
            Self::AcceptedAdmission(_) => 2,
            Self::LegacyImport => 3,
        }
    }
    pub fn certificate(self) -> Option<CertificateId> {
        match self {
            Self::AcceptedAdmission(id) => Some(id),
            _ => None,
        }
    }
    pub fn from_parts(kind: u8, id: Option<CertificateId>) -> Result<Self, Error> {
        match (kind, id) {
            (0, None) => Ok(Self::OwnerRegistration),
            (1, None) => Ok(Self::OwnerRecovery),
            (2, Some(id)) => Ok(Self::AcceptedAdmission(id)),
            (3, None) => Ok(Self::LegacyImport),
            _ => Err(Error::CorruptLog),
        }
    }
}

/// Durable, immutable authentication identity. Not a membership grant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnownIdentity {
    pub node: NodeId,
    pub signing_key: [u8; 32],
    /// Transport binding key. For `PeerKey::SignedHttps` this repeats
    /// `signing_key` for explicit typed binding identity.
    pub key: PeerKey,
    pub provenance: Provenance,
}

impl KnownIdentity {
    /// The exact membership binding this pin proves possession of.
    pub fn member_matches(&self, member: &harmion_protocol::membership::Member) -> bool {
        member.node == self.node
            && member.signing_key == self.signing_key
            && crate::membership_state::as_peer(member).key == self.key
    }
}

/// Insert or accept an existing identical pin. Never overwrite and never reuse a
/// signing key or binding key for another Node ID.
pub fn pin(
    tx: &mut dyn Transaction,
    node: NodeId,
    signing_key: [u8; 32],
    key: PeerKey,
    provenance: Provenance,
) -> Result<(), Error> {
    if let PeerKey::Iroh(bytes) | PeerKey::SignedHttps(bytes) = &key
        && bytes.len() != 32
    {
        return Err(Error::InvalidRequest);
    }
    if matches!(key, PeerKey::TlsCertificate(ref bytes) if bytes.is_empty()) {
        return Err(Error::InvalidRequest);
    }
    // The explicit signed-HTTPS binding repeats the membership signing key; it
    // is never a second credential for another key.
    if let PeerKey::SignedHttps(bound) = &key
        && *bound != signing_key
    {
        return Err(Error::InvalidRequest);
    }
    if let Some(existing) = tx.known_peer_identity(node)? {
        if existing.signing_key == signing_key && existing.key == key {
            return Ok(());
        }
        return Err(Error::IdentityMismatch);
    }
    for existing in tx.known_peer_identities()? {
        if existing.node != node && (existing.signing_key == signing_key || existing.key == key) {
            return Err(Error::IdentityMismatch);
        }
    }
    tx.put_known_identity(&KnownIdentity {
        node,
        signing_key,
        key,
        provenance,
    })
}

/// Fail closed when an active remote member has no pin or a pin that disagrees
/// with its effective identity. Called after membership tables are verified.
pub fn verify_active_identities(
    tx: &mut dyn Transaction,
    state: &crate::membership_state::Membership,
) -> Result<(), Error> {
    use crate::membership_state::Status;
    for (node, status) in &state.nodes {
        let Status::Active(member) = status else {
            continue;
        };
        if *node == tx.identity().node {
            continue;
        }
        let pin = tx.known_peer_identity(*node)?.ok_or(Error::CorruptLog)?;
        if !pin.member_matches(member) {
            return Err(Error::CorruptLog);
        }
    }
    Ok(())
}

/// Seed pins for a database that predates the registry. Only identities whose
/// exact signing key and binding are validated from effective membership state
/// are imported; historical claims are never replayed into new pins.
pub fn migrate(
    tx: &mut dyn Transaction,
    state: Option<&crate::membership_state::Membership>,
) -> Result<(), Error> {
    let local = tx.identity().node;
    let root = tx.membership_root()?;
    let recovery_roots = tx.recovery_roots()?;
    let root_id = root.as_ref().map(|root| root.id());
    let recovery: Vec<CertificateId> = recovery_roots.iter().map(|root| root.id()).collect();
    if let Some(state) = state {
        let admits: Vec<(NodeId, [u8; 32], crate::membership::PeerKey, CertificateId)> = tx
            .membership_entries()?
            .into_iter()
            .filter(|entry| state.confirmed.contains(&entry.record.id()))
            .filter_map(|entry| match &entry.record.statement().action {
                harmion_protocol::membership::Action::Admit(member) => Some((
                    member.node,
                    member.signing_key,
                    crate::membership_state::as_peer(member).key,
                    entry.record.id(),
                )),
                _ => None,
            })
            .collect();
        for (node, status) in &state.nodes {
            let crate::membership_state::Status::Active(member) = status else {
                continue;
            };
            if *node == local {
                continue;
            }
            let matched = admits
                .iter()
                .find(|(candidate, key, binding, _)| {
                    candidate == node && *key == member.signing_key && *binding == entry_key(member)
                })
                .map(|(_, _, _, id)| *id);
            let provenance = match matched {
                Some(certificate) if Some(certificate) == root_id => Provenance::OwnerRegistration,
                Some(certificate) if recovery.contains(&certificate) => Provenance::OwnerRecovery,
                Some(certificate) => Provenance::AcceptedAdmission(certificate),
                // A trust anchor is effective even when its self-signed
                // certificate is not duplicated in the membership log.
                None if root_matches(root.as_ref(), member) => Provenance::OwnerRegistration,
                None if recovery_roots
                    .iter()
                    .any(|root| root_matches(Some(root), member)) =>
                {
                    Provenance::OwnerRecovery
                }
                None => return Err(Error::CorruptLog),
            };
            pin(tx, *node, member.signing_key, entry_key(member), provenance)?;
        }
    }
    // A peer row without effective membership state has no validated signing
    // key, so it cannot be imported. Its native transport keeps working and v2
    // treats it as unknown until explicit owner verification.
    Ok(())
}

fn entry_key(member: &harmion_protocol::membership::Member) -> crate::membership::PeerKey {
    crate::membership_state::as_peer(member).key
}

fn root_matches(
    certificate: Option<&harmion_protocol::membership::Certificate>,
    member: &harmion_protocol::membership::Member,
) -> bool {
    certificate.is_some_and(|certificate| {
        matches!(
            &certificate.statement().action,
            harmion_protocol::membership::Action::Admit(anchor)
                if anchor.node == member.node
                    && anchor.signing_key == member.signing_key
                    && anchor.binding == member.binding
        )
    })
}

/// Reject an unusable Ed25519 point before it can be stored as a member key.
pub fn validate_signing_key(key: &[u8; 32]) -> Result<(), Error> {
    ed25519_dalek::VerifyingKey::from_bytes(key)
        .map(|_| ())
        .map_err(|_| Error::InvalidSignature)
}
