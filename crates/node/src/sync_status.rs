//! Historical completed-round evidence, restricted to full administrators.
use crate::{
    Error,
    access::{Credential, authenticate},
    storage::{Store, Transaction},
};
use harmion_domain::{NodeId, UpdateId};
use harmion_protocol::{
    LogEpoch, hex,
    sync_status::{PeerSyncStatus, SyncFailure, SyncStatus},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    pub local_epoch: LogEpoch,
    pub acknowledged_through: u64,
    pub remote_epoch: LogEpoch,
    pub remote_high: u64,
    pub received_through: u64,
    pub succeeded_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub attempted_at: i64,
    pub code: String,
}

/// Call only after the fixed-high round completes and the authenticated network
/// caller validates its ACK and cursor. Partial failed rounds do not count.
pub fn record_success(
    tx: &mut dyn Transaction,
    node: NodeId,
    observation: &Observation,
) -> Result<(), Error> {
    crate::membership::active_peer(tx, node)?;
    if observation.local_epoch != tx.identity().epoch
        || observation.acknowledged_through > tx.accepted_high()?
        || observation.received_through < observation.remote_high
        || tx.cursor(node, observation.remote_epoch)? < observation.received_through
    {
        return Err(Error::InvalidPage);
    }
    tx.put_sync_observation(node, observation)?;
    tx.put_sync_failure(node, None)
}

/// `code` must be a stable non-sensitive classification, never formatted error
/// text, credentials, keys, or endpoints. Retired known peers may fail a round.
pub fn record_failure(
    tx: &mut dyn Transaction,
    node: NodeId,
    now: i64,
    code: &str,
) -> Result<(), Error> {
    tx.peer(node)?.ok_or(Error::NotFound)?;
    if code.is_empty()
        || code.len() > 64
        || !code.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
    {
        return Err(Error::InvalidRequest);
    }
    tx.put_sync_failure(
        node,
        Some(&Failure {
            attempted_at: now,
            code: code.to_owned(),
        }),
    )
}

/// Authenticate and sample all evidence in one transaction. This deliberately
/// requires full administrator grants, not merely manage-nodes or read-all.
pub fn get(
    store: &mut impl Store,
    credential: &Credential,
    now: i64,
    update: Option<UpdateId>,
) -> Result<SyncStatus, Error> {
    store.transaction(|tx| {
        if !authenticate(tx, credential, now)?
            .app
            .grants
            .is_administrator()
        {
            return Err(Error::Forbidden);
        }
        let identity = tx.identity();
        let local_high = tx.accepted_high()?;
        let update_sequence = update
            .map(|id| tx.accepted_sequence(id)?.ok_or(Error::NotFound))
            .transpose()?;
        let mut peers = Vec::new();
        let mut known_peers = tx.peers()?;
        known_peers.sort_by_key(|peer| peer.node);
        for peer in known_peers {
            let observation = tx.sync_observation(peer.node)?;
            let failure = tx.sync_failure(peer.node)?;
            let applicable = observation.as_ref().filter(|o| {
                o.local_epoch == identity.epoch && o.acknowledged_through <= local_high
            });
            let acknowledged = applicable.map_or(0, |o| o.acknowledged_through);
            peers.push(PeerSyncStatus {
                node: hex(&peer.node.0),
                retired: peer.retired,
                last_success_at: observation.as_ref().map(|o| o.succeeded_at.to_string()),
                last_failure: failure.map(|f| SyncFailure {
                    attempted_at: f.attempted_at.to_string(),
                    code: f.code,
                }),
                local_epoch: observation.as_ref().map(|o| hex(&o.local_epoch.0)),
                acknowledged_through: observation
                    .as_ref()
                    .map(|o| o.acknowledged_through.to_string()),
                remote_epoch: observation.as_ref().map(|o| hex(&o.remote_epoch.0)),
                remote_high: observation.as_ref().map(|o| o.remote_high.to_string()),
                received_through: observation.as_ref().map(|o| o.received_through.to_string()),
                unconfirmed_local: tx.accepted_count_after(acknowledged)?.to_string(),
                update_confirmed: applicable.and_then(|o| {
                    update_sequence.map(|sequence| sequence <= o.acknowledged_through)
                }),
            });
        }
        Ok(SyncStatus {
            node: hex(&identity.node.0),
            local_epoch: hex(&identity.epoch.0),
            local_high: local_high.to_string(),
            update: update.map(|id| hex(&id.0)),
            peers,
        })
    })
}
