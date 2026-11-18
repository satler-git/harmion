//! Exchange signed control records before attempting data synchronization.
use crate::{Error, SharedStore, sync::PeerTransport};
use harmion_node::{
    membership::Peer,
    membership_log,
    storage::{Identity, Store},
};
use harmion_protocol::{PeerCommand, PeerReply};

pub async fn exchange<S: Store + Send + 'static>(
    store: &SharedStore<S>,
    transport: &(impl PeerTransport + ?Sized),
    local: Identity,
    peer: &Peer,
) -> Result<(), Error> {
    let local_high = store.run(|s| s.transaction(membership_log::high)).await?;
    let PeerReply::Status {
        epoch,
        high: remote_high,
    } = transport
        .exchange(local, peer, PeerCommand::MembershipStatus)
        .await?
    else {
        return Err(Error::InvalidResponse);
    };
    let PeerReply::Cursor {
        epoch: echoed,
        through: mut upload,
    } = transport
        .exchange(
            local,
            peer,
            PeerCommand::MembershipCursor { epoch: local.epoch },
        )
        .await?
    else {
        return Err(Error::InvalidResponse);
    };
    if echoed != local.epoch || upload > store.run(|s| s.transaction(membership_log::high)).await? {
        return Err(Error::InvalidResponse);
    }
    let node = peer.node;
    let mut download = store
        .run(move |s| s.transaction(|tx| tx.membership_cursor(node, epoch)))
        .await?;
    if download > remote_high {
        match transport
            .exchange(local, peer, PeerCommand::MembershipStatus)
            .await?
        {
            PeerReply::Status {
                epoch: current,
                high,
            } if current == epoch && download <= high => (),
            _ => return Err(Error::InvalidResponse),
        }
    }
    while upload < local_high || download < remote_high {
        if upload < local_high {
            let page = store
                .run(move |s| {
                    s.transaction(|tx| membership_log::page(tx, local.epoch, upload, local_high))
                })
                .await?;
            let through = page.through;
            match transport
                .exchange(local, peer, PeerCommand::MembershipAccept(page))
                .await?
            {
                PeerReply::Ack {
                    epoch,
                    through: ack,
                } if epoch == local.epoch && ack == through && ack > upload => upload = ack,
                PeerReply::CursorMismatch { epoch, current }
                    if epoch == local.epoch && current > upload =>
                {
                    if current > store.run(|s| s.transaction(membership_log::high)).await? {
                        return Err(Error::InvalidResponse);
                    }
                    upload = current;
                }
                _ => return Err(Error::InvalidResponse),
            }
        }
        if download < remote_high {
            let PeerReply::MembershipPage(page) = transport
                .exchange(
                    local,
                    peer,
                    PeerCommand::MembershipPage {
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
            crate::peer::validate_membership_page(&page)?;
            let expected = peer.clone();
            match store
                .run(move |s| {
                    s.transaction(|tx| {
                        let current = tx.peer(node)?.ok_or(harmion_node::Error::Unauthenticated)?;
                        if current.key != expected.key {
                            return Err(harmion_node::Error::IdentityMismatch);
                        }
                        membership_log::receive(tx, node, &page)
                    })
                })
                .await
            {
                Ok(through) => download = through,
                Err(Error::Node(harmion_node::Error::CursorMismatch { current }))
                    if current > download =>
                {
                    match transport
                        .exchange(local, peer, PeerCommand::MembershipStatus)
                        .await?
                    {
                        PeerReply::Status {
                            epoch: current_epoch,
                            high,
                        } if current_epoch == epoch && current <= high => download = current,
                        _ => return Err(Error::InvalidResponse),
                    }
                }
                Err(error) => return Err(error),
            }
        }
    }
    Ok(())
}
