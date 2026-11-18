//! Initial owner-approved peer bindings. Key possession is proven
//! by the transport host; every operation also checks this durable membership table.
use crate::{
    Error,
    access::{Credential, authenticate},
    replication,
    storage::{Store, Transaction},
};
use harmion_domain::{ClusterId, NodeId};
use harmion_protocol::{PeerCommand, PeerReply, PeerRequest, PeerResponse};

pub const MAX_CERTIFICATE_BYTES: usize = 64 * 1024;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PeerKey {
    TlsCertificate(Vec<u8>),
    Iroh([u8; 32]),
}
impl PeerKey {
    pub fn tls_certificate(&self) -> Result<&[u8], Error> {
        match self {
            Self::TlsCertificate(bytes) => Ok(bytes),
            _ => Err(Error::IdentityMismatch),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Peer {
    pub node: NodeId,
    pub key: PeerKey,
    pub endpoint: String,
    pub retired: bool,
}

/// The host validates TLS DER / iroh public key and routing syntax before approval.
/// Approval is explicit and cannot be inferred merely from a TLS connection.
pub fn approve(
    store: &mut impl Store,
    now: i64,
    credential: &Credential,
    cluster: ClusterId,
    peer: Peer,
) -> Result<(), Error> {
    store.transaction(|tx| {
        if tx.membership_identity()?.is_some() { return Err(Error::InvalidRequest); }
        if !authenticate(tx, credential, now)?.app.grants.manage_nodes {
            return Err(Error::Forbidden);
        }
        if cluster != tx.identity().cluster || peer.node == tx.identity().node {
            return Err(Error::IdentityMismatch);
        }
        if peer.retired
            || matches!(&peer.key, PeerKey::TlsCertificate(bytes) if bytes.is_empty() || bytes.len() > MAX_CERTIFICATE_BYTES)
            || (matches!(peer.key, PeerKey::TlsCertificate(_)) && peer.endpoint.is_empty())
            || peer.endpoint.len() > 2048
        {
            return Err(Error::InvalidRequest);
        }
        if let Some(previous) = tx.peer(peer.node)? {
            if previous.retired {
                return Err(Error::PeerRetired);
            }
            if previous.key != peer.key {
                return Err(Error::IdentityMismatch);
            }
        }
        if tx
            .peers()?
            .iter()
            .any(|p| p.node != peer.node && p.key == peer.key)
        {
            return Err(Error::IdentityMismatch);
        }
        tx.put_peer(&peer)
    })
}

pub fn retire(
    store: &mut impl Store,
    now: i64,
    credential: &Credential,
    node: NodeId,
) -> Result<(), Error> {
    store.transaction(|tx| {
        if tx.membership_identity()?.is_some() {
            return Err(Error::InvalidRequest);
        }
        if !authenticate(tx, credential, now)?.app.grants.manage_nodes {
            return Err(Error::Forbidden);
        }
        let mut peer = tx.peer(node)?.ok_or(Error::NotFound)?;
        peer.retired = true;
        tx.put_peer(&peer)
    })
}

pub fn active_peer(tx: &mut dyn Transaction, node: NodeId) -> Result<Peer, Error> {
    let peer = tx.peer(node)?.ok_or(Error::Unauthenticated)?;
    if let Some(state) = crate::membership_state::state(tx)? {
        crate::membership_state::require_active(&state, tx.identity().node)?;
        let member = crate::membership_state::require_active(&state, node)?;
        if crate::membership_state::as_peer(member).key != peer.key {
            return Err(Error::IdentityMismatch);
        }
    }
    if peer.retired {
        return Err(Error::PeerRetired);
    }
    Ok(peer)
}

/// Accept a page returned by an authenticated outbound exchange. The host must
/// have verified the response against this approved binding. Recheck membership
/// inside the write transaction so retirement during network I/O rejects it.
pub fn receive_response(
    store: &mut impl Store,
    expected: &Peer,
    page: &harmion_protocol::Page,
) -> Result<u64, Error> {
    store.transaction(|tx| {
        if active_peer(tx, expected.node)? != *expected {
            return Err(Error::IdentityMismatch);
        }
        replication::receive(tx, expected.node, page)
    })
}

struct PeerPrincipal {
    node: NodeId,
}
fn authenticate_peer(
    tx: &mut dyn Transaction,
    key: &PeerKey,
    request: &PeerRequest,
) -> Result<PeerPrincipal, Error> {
    if request.cluster != tx.identity().cluster || request.node == tx.identity().node {
        return Err(Error::IdentityMismatch);
    }
    let peer = if is_membership_command(&request.command) {
        if tx.membership_root()?.is_none() {
            return Err(Error::InvalidRequest);
        }
        // Known keys can exchange signed administrative evidence even while a
        // retirement conflict blocks data. This grants no mutation authority.
        tx.peer(request.node)?.ok_or(Error::Unauthenticated)?
    } else {
        active_peer(tx, request.node)?
    };
    if &peer.key != key {
        return Err(Error::Unauthenticated);
    }
    Ok(PeerPrincipal { node: peer.node })
}

fn is_membership_command(command: &PeerCommand) -> bool {
    matches!(
        command,
        PeerCommand::MembershipStatus
            | PeerCommand::MembershipCursor { .. }
            | PeerCommand::MembershipPage { .. }
            | PeerCommand::MembershipAccept(_)
    )
}

/// Only call with the identity proven by the transport handshake (TLS leaf or
/// iroh remote Endpoint ID), never an identity claimed in a header/body.
/// The ACK is returned after commit.
pub fn handle(
    store: &mut impl Store,
    verified_key: &PeerKey,
    request: &PeerRequest,
) -> Result<PeerResponse, Error> {
    let mut local_identity = None;
    let result = store.transaction(|tx| {
        local_identity = Some(tx.identity());
        let principal = authenticate_peer(tx, verified_key, request)?;
        let identity = tx.identity();
        let reply = match &request.command {
            PeerCommand::MembershipStatus => PeerReply::Status {
                epoch: identity.epoch,
                high: crate::membership_log::high(tx)?,
            },
            PeerCommand::MembershipCursor { epoch } => PeerReply::Cursor {
                epoch: *epoch,
                through: tx.membership_cursor(principal.node, *epoch)?,
            },
            PeerCommand::MembershipPage { epoch, after, high } => {
                PeerReply::MembershipPage(crate::membership_log::page(tx, *epoch, *after, *high)?)
            }
            PeerCommand::MembershipAccept(page) => {
                match crate::membership_log::receive(tx, principal.node, page) {
                    Ok(through) => PeerReply::Ack {
                        epoch: page.epoch,
                        through,
                    },
                    Err(Error::CursorMismatch { current }) => PeerReply::CursorMismatch {
                        epoch: page.epoch,
                        current,
                    },
                    Err(error) => return Err(error),
                }
            }
            PeerCommand::Status => PeerReply::Status {
                epoch: identity.epoch,
                high: replication::high(tx)?,
            },
            PeerCommand::Cursor { epoch } => PeerReply::Cursor {
                epoch: *epoch,
                through: tx.cursor(principal.node, *epoch)?,
            },
            PeerCommand::Page { epoch, after, high } => {
                PeerReply::Page(replication::page(tx, *epoch, *after, *high)?)
            }
            PeerCommand::Accept(page) => match replication::receive(tx, principal.node, page) {
                Ok(through) => PeerReply::Ack {
                    epoch: page.epoch,
                    through,
                },
                Err(Error::CursorMismatch { current }) => PeerReply::CursorMismatch {
                    epoch: page.epoch,
                    current,
                },
                Err(error) => return Err(error),
            },
        };
        Ok(PeerResponse {
            cluster: identity.cluster,
            node: identity.node,
            reply,
        })
    });
    match result {
        Err(Error::RecordRejected { update_id, reason }) => {
            // Convert to a protocol rejection only AFTER the failed transaction rolled back.
            let identity = local_identity.ok_or(Error::CorruptLog)?;
            let reason = match reason {
                harmion_domain::Error::MissingDependency(_) => {
                    harmion_protocol::Rejection::MissingDependency
                }
                harmion_domain::Error::IdMismatch => harmion_protocol::Rejection::IdMismatch,
                _ => harmion_protocol::Rejection::InvalidUpdate,
            };
            Ok(PeerResponse {
                cluster: identity.cluster,
                node: identity.node,
                reply: PeerReply::Rejected { update_id, reason },
            })
        }
        other => other,
    }
}
