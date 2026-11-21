//! Fixed-high synchronization independent of HTTP and TLS framing.
use crate::{Error, SharedStore, SystemHost};
use harmion_domain::NodeId;
use harmion_node::{application::Host, sync_status};
use harmion_node::{
    membership::{self, Peer},
    replication,
    storage::{Identity, Store},
};
use harmion_protocol::*;
use std::future::Future;

/// Host transport authenticates the remote against the approved binding before
/// sending data, and verifies the response cluster/node. A response from an
/// unverified connection must never be returned. Implementations bound frames,
/// deadlines and connection lifetime; they do not commit records or cursors.
pub trait PeerTransport: Send + Sync {
    fn exchange(
        &self,
        local: Identity,
        peer: &Peer,
        command: PeerCommand,
    ) -> impl Future<Output = Result<PeerReply, Error>> + Send;
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub uploaded: u64,
    pub downloaded: u64,
    pub local_high: u64,
    pub remote_high: u64,
}

async fn check_remote_cursor(
    transport: &(impl PeerTransport + ?Sized),
    local: Identity,
    peer: &Peer,
    epoch: LogEpoch,
    cursor: u64,
    frozen_high: u64,
) -> Result<(), Error> {
    if cursor <= frozen_high {
        return Ok(());
    }
    // Another inbound transfer can finish beyond this round's frozen high.
    // Distinguish that from a sender rolling its log back without changing epoch.
    match transport.exchange(local, peer, PeerCommand::Status).await? {
        PeerReply::Status {
            epoch: current_epoch,
            high,
        } if current_epoch == epoch && cursor <= high => Ok(()),
        _ => Err(Error::InvalidResponse),
    }
}

#[tracing::instrument(level = "debug", skip_all, fields(peer = %hex(&node.0)))]
pub async fn sync_once<S: Store + Send + 'static>(
    store: &SharedStore<S>,
    transport: &(impl PeerTransport + ?Sized),
    node: NodeId,
) -> Result<SyncReport, Error> {
    // A stalled remote must not hold up unrelated peers. Keep same-peer outgoing
    // rounds serialized; inbound receipt still uses durable transactional cursors.
    let lock = store.round_lock(node)?;
    // Outcome writes run on blocking workers and may outlive a cancelled caller.
    // Keep serialization until those writes actually finish, not just until await
    // is dropped, so an older outcome cannot overwrite a newer same-peer round.
    let round = std::sync::Arc::new(lock.lock_owned().await);
    let result = sync_round(store, transport, node, round.clone()).await;
    if let Err(error) = &result {
        let code = failure_code(error);
        // Diagnostics must not replace the original synchronization failure.
        // Once submitted, this write may commit even if the caller is cancelled.
        if let Err(record_error) = store
            .run(move |s| {
                let _round = round;
                s.transaction(|tx| {
                    sync_status::record_failure(tx, node, SystemHost.now_micros(), code)
                })
            })
            .await
        {
            tracing::debug!(%record_error, "could not persist sync failure");
        }
    }
    result
}

/// Stable, bounded diagnostic codes; never persist transport messages or payloads.
fn failure_code(error: &Error) -> &'static str {
    use harmion_node::Error as N;
    match error {
        Error::Timeout => "timeout",
        Error::Io(_) | Error::Transport(_) => "transport_error",
        Error::InvalidResponse => "invalid_response",
        Error::RemoteRejected { .. } | Error::Node(N::RecordRejected { .. }) => "record_rejected",
        Error::Node(N::PeerRetired) => "peer_retired",
        Error::Node(N::MembershipConflict) => "membership_conflict",
        Error::Node(N::Unauthenticated) => "unauthenticated",
        Error::Node(N::IdentityMismatch) => "identity_mismatch",
        Error::Node(N::Storage(_) | N::CorruptLog) => "storage_error",
        _ => "sync_error",
    }
}

async fn sync_round<S: Store + Send + 'static>(
    store: &SharedStore<S>,
    transport: &(impl PeerTransport + ?Sized),
    node: NodeId,
    round: std::sync::Arc<tokio::sync::OwnedMutexGuard<()>>,
) -> Result<SyncReport, Error> {
    let control = store
        .run(move |s| {
            s.transaction(|tx| {
                if tx.membership_root()?.is_none() {
                    return Ok(None);
                }
                Ok(Some((
                    tx.identity(),
                    tx.peer(node)?.ok_or(harmion_node::Error::Unauthenticated)?,
                )))
            })
        })
        .await?;
    if let Some((identity, peer)) = control {
        crate::membership_sync::exchange(store, transport, identity, &peer).await?;
    }
    let (local, peer, local_high) = store
        .run(move |s| {
            s.transaction(|tx| {
                Ok((
                    tx.identity(),
                    membership::active_peer(tx, node)?,
                    replication::high(tx)?,
                ))
            })
        })
        .await?;
    let PeerReply::Status {
        epoch,
        high: remote_high,
    } = transport
        .exchange(local, &peer, PeerCommand::Status)
        .await?
    else {
        return Err(Error::InvalidResponse);
    };
    let PeerReply::Cursor {
        epoch: echoed,
        through: mut upload,
    } = transport
        .exchange(local, &peer, PeerCommand::Cursor { epoch: local.epoch })
        .await?
    else {
        return Err(Error::InvalidResponse);
    };
    if echoed != local.epoch {
        return Err(Error::InvalidResponse);
    }
    if upload > local_high && upload > store.run(|s| s.transaction(replication::high)).await? {
        return Err(Error::InvalidResponse);
    }
    let mut download = store
        .run(move |s| {
            s.transaction(|tx| {
                membership::active_peer(tx, node)?;
                tx.cursor(node, epoch)
            })
        })
        .await?;
    check_remote_cursor(transport, local, &peer, epoch, download, remote_high).await?;
    let mut report = SyncReport {
        local_high,
        remote_high,
        ..SyncReport::default()
    };
    while upload < local_high || download < remote_high {
        if upload < local_high {
            let page = store
                .run(move |s| {
                    s.transaction(|tx| {
                        membership::active_peer(tx, node)?;
                        replication::page(tx, local.epoch, upload, local_high)
                    })
                })
                .await?;
            let through = page.through;
            let count = page.entries.len() as u64;
            match transport
                .exchange(local, &peer, PeerCommand::Accept(page))
                .await?
            {
                PeerReply::Ack {
                    epoch: received,
                    through: accepted,
                } if received == local.epoch && accepted == through && accepted > upload => {
                    upload = accepted;
                    report.uploaded += count;
                }
                PeerReply::CursorMismatch {
                    epoch: received,
                    current,
                } if received == local.epoch && current > upload => {
                    if current > local_high
                        && current > store.run(|s| s.transaction(replication::high)).await?
                    {
                        return Err(Error::InvalidResponse);
                    }
                    upload = current
                }
                _ => return Err(Error::InvalidResponse),
            }
        }
        if download < remote_high {
            store
                .run(move |s| s.transaction(|tx| membership::active_peer(tx, node).map(|_| ())))
                .await?;
            let PeerReply::Page(page) = transport
                .exchange(
                    local,
                    &peer,
                    PeerCommand::Page {
                        epoch,
                        after: download,
                        high: remote_high,
                    },
                )
                .await?
            else {
                return Err(Error::InvalidResponse);
            };
            if page.epoch != epoch
                || page.after != download
                || page.high != remote_high
                || page.through <= download
            {
                return Err(Error::InvalidResponse);
            }
            let count = page.entries.len() as u64;
            let expected = peer.clone();
            let received = store
                .run_notifying(true, move |s| {
                    membership::receive_response(s, &expected, &page)
                })
                .await;
            match received {
                Ok(through) => {
                    download = through;
                    report.downloaded += count;
                }
                Err(Error::Node(harmion_node::Error::CursorMismatch { current }))
                    if current > download =>
                {
                    check_remote_cursor(transport, local, &peer, epoch, current, remote_high)
                        .await?;
                    download = current;
                }
                Err(error) => return Err(error),
            }
        }
    }
    let observation = sync_status::Observation {
        local_epoch: local.epoch,
        acknowledged_through: upload,
        remote_epoch: epoch,
        remote_high,
        received_through: download,
        succeeded_at: SystemHost.now_micros(),
    };
    store
        .run(move |s| {
            let _round = round;
            s.transaction(|tx| {
                if membership::active_peer(tx, node)? != peer {
                    return Err(harmion_node::Error::IdentityMismatch);
                }
                sync_status::record_success(tx, node, &observation)
            })
        })
        .await?;
    tracing::debug!(
        uploaded = report.uploaded,
        downloaded = report.downloaded,
        local_high = report.local_high,
        remote_high = report.remote_high,
        "sync completed"
    );
    Ok(report)
}

#[cfg(test)]
mod cancellation_tests;
