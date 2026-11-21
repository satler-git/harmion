//! Deployment-time provisioning. Host-only: callers must not expose this through
//! a network API, and an unconfigured replica stays fail-closed.
//!
//! The owner supplies the node identity, an already-pinned trust anchor, the
//! signed admission evidence for this node and initial contacts. Reapplying the
//! identical manifest is idempotent; any conflicting identity or root fails.
use crate::{
    Error,
    access::{Credential, authenticate},
    identity::{self, Provenance},
    membership::MAX_CERTIFICATE_BYTES,
    membership_state::{self, MembershipSigner, Status},
    storage::Store,
};
use harmion_domain::NodeId;
use harmion_protocol::membership::{Action, Binding, Certificate, Member, Statement};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug)]
pub struct Manifest<'a> {
    /// This node's member entry. Its signing key must be the provisioned seed.
    pub own: Member,
    /// Owner-pinned self-signed cluster trust anchor.
    pub root: Certificate,
    /// Signed admission chain for this node. Empty means "awaiting first
    /// evidence", which requires at least one initial contact.
    pub evidence: &'a [Certificate],
    /// Explicitly pinned contacts the owner verified out of band.
    pub contacts: &'a [Member],
}

pub fn provision(
    store: &mut impl Store,
    signer: &MembershipSigner,
    manifest: &Manifest<'_>,
) -> Result<(), Error> {
    store.transaction(|tx| {
        let local = tx.identity();
        if manifest.own.node != local.node || manifest.own.signing_key != signer.public_key() {
            return Err(Error::IdentityMismatch);
        }
        validate_member(&manifest.own)?;
        for contact in manifest.contacts {
            if contact.node == local.node {
                return Err(Error::InvalidRequest);
            }
            validate_member(contact)?;
        }
        // The anchor must be a self-issued admission, not an arbitrary record.
        membership_state::verify(local.cluster, &manifest.root)?;
        let Action::Admit(anchor) = &manifest.root.statement().action else {
            return Err(Error::InvalidRequest);
        };
        if anchor.node != manifest.root.statement().issuer
            || anchor.signing_key != manifest.root.statement().signing_key
        {
            return Err(Error::InvalidRequest);
        }
        validate_member(anchor)?;
        for certificate in manifest.evidence {
            membership_state::verify(local.cluster, certificate)?;
            let Action::Admit(member) = &certificate.statement().action else {
                return Err(Error::InvalidRequest);
            };
            validate_member(member)?;
        }
        if manifest.evidence.is_empty() && manifest.contacts.is_empty() {
            return Err(Error::InvalidRequest);
        }

        // A deterministic self-admission is the durable node identity; it is not
        // proof of current authority on its own.
        let identity_certificate = signer.sign(Statement {
            cluster: local.cluster,
            issuer: local.node,
            signing_key: manifest.own.signing_key,
            nonce: [0; 32],
            action: Action::Admit(manifest.own.clone()),
        })?;
        match tx.membership_identity()? {
            Some(existing) if existing != identity_certificate => {
                return Err(Error::IdentityMismatch);
            }
            Some(_) => {}
            None => {
                tx.set_membership_identity(&identity_certificate)?;
                tx.append_membership(&identity_certificate)?;
            }
        }
        match tx.membership_root()? {
            Some(existing) if existing != manifest.root => return Err(Error::IdentityMismatch),
            Some(_) => {}
            None => {
                tx.set_membership_root(&manifest.root)?;
                tx.append_membership(&manifest.root)?;
            }
        }
        membership_state::apply(tx, manifest.evidence)?;
        for contact in manifest.contacts {
            identity::pin(
                tx,
                contact.node,
                contact.signing_key,
                membership_state::as_peer(contact).key,
                Provenance::OwnerRegistration,
            )?;
        }
        let state = membership_state::state(tx)?.ok_or(Error::InvalidRequest)?;
        match state.nodes.get(&local.node) {
            Some(Status::Active(_)) => Ok(()),
            Some(Status::Retired | Status::Conflict) => Err(Error::IdentityMismatch),
            // No admission yet: allowed only to bootstrap evidence exchange with
            // the explicitly pinned contacts.
            _ if manifest.evidence.is_empty() => Ok(()),
            _ => Err(Error::InvalidRequest),
        }
    })
}

/// Owner-facing inputs needed to provision another host with this node's cluster.
///
/// This is the inverse of [`provision`]: the owner approves the target on this
/// node, then hands the exported anchor, admission chain and contacts to the
/// target host. It contains only public membership material.
#[derive(Clone, Debug)]
pub struct ExportManifest {
    pub own: Member,
    pub root: Certificate,
    pub evidence: Vec<Certificate>,
    pub contacts: Vec<Member>,
}

/// Build the provisioning manifest for an already-approved member.
///
/// The target must be active on this node; exporting an unapproved
/// registration would publish an anchor the target cannot use. The admission
/// chain is emitted root-first because `provision` applies the anchor before
/// the evidence.
pub fn export_manifest(
    store: &mut impl Store,
    credential: &Credential,
    now: i64,
    cluster: harmion_domain::ClusterId,
    requested: Member,
) -> Result<ExportManifest, Error> {
    store.transaction(|tx| {
        if !authenticate(tx, credential, now)?.app.grants.manage_nodes {
            return Err(Error::Forbidden);
        }
        let local = tx.identity();
        if cluster != local.cluster || requested.node == local.node {
            return Err(Error::InvalidRequest);
        }
        validate_member(&requested)?;
        let state = membership_state::state(tx)?.ok_or(Error::InvalidRequest)?;
        let active = match state.nodes.get(&requested.node) {
            Some(Status::Active(member)) => member.clone(),
            _ => return Err(Error::InvalidRequest),
        };
        // The owner must have approved exactly this registration. A different
        // endpoint or key is a conflict, not a silent rewrite.
        if active != requested {
            return Err(Error::IdentityMismatch);
        }
        let root = tx.membership_root()?.ok_or(Error::InvalidRequest)?;
        let Action::Admit(anchor) = root.statement().action.clone() else {
            return Err(Error::CorruptLog);
        };
        let mut admits: BTreeMap<NodeId, Certificate> = BTreeMap::new();
        for entry in tx.membership_entries()? {
            if let Action::Admit(member) = &entry.record.statement().action {
                admits.insert(member.node, entry.record);
            }
        }
        let mut chain = Vec::new();
        let mut seen = BTreeSet::new();
        let mut current = active.node;
        loop {
            if !seen.insert(current) {
                return Err(Error::CorruptLog);
            }
            let certificate = admits.get(&current).ok_or(Error::InvalidRequest)?;
            chain.push(certificate.clone());
            let issuer = certificate.statement().issuer;
            if issuer == anchor.node {
                break;
            }
            current = issuer;
        }
        chain.reverse();
        if chain
            .first()
            .map(|certificate| certificate.statement().issuer)
            != Some(anchor.node)
        {
            return Err(Error::CorruptLog);
        }
        let contacts = state
            .nodes
            .iter()
            .filter_map(|(node, status)| match status {
                Status::Active(member) if *node != active.node => Some(member.clone()),
                _ => None,
            })
            .collect();
        Ok(ExportManifest {
            own: active,
            root,
            evidence: chain,
            contacts,
        })
    })
}

fn validate_member(member: &Member) -> Result<(), Error> {
    identity::validate_signing_key(&member.signing_key)?;
    match &member.binding {
        Binding::SignedHttps(key) => {
            if *key != member.signing_key {
                return Err(Error::IdentityMismatch);
            }
            harmion_protocol::https_peer::validate_origin(&member.endpoint)?;
        }
        Binding::Https(bytes) => {
            if bytes.is_empty() || bytes.len() > MAX_CERTIFICATE_BYTES {
                return Err(Error::InvalidRequest);
            }
        }
        Binding::Iroh(_) => {}
    }
    Ok(())
}
