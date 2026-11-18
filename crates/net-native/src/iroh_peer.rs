//! Authenticated QUIC streams carrying the same Peer messages as HTTPS.
use crate::{Error, SharedStore, sync::PeerTransport, transport};
use harmion_node::{
    membership::{self, Peer, PeerKey},
    storage::{Identity, Store},
};
use harmion_protocol::*;
use iroh::{Endpoint, EndpointAddr, EndpointId};
use std::{sync::Arc, time::Duration};
use tokio::{
    sync::{Semaphore, watch},
    time::timeout,
};

pub const ALPN: &[u8] = b"harmion/peer/1";
const DEADLINE: Duration = Duration::from_secs(30);

/// Explicit relay and discovery policy. Public lookup uses n0 DNS/Pkarr;
/// disabling it requires an out-of-band route hint in the approved Peer.
pub async fn bind(
    secret: [u8; 32],
    relay_urls: Option<Vec<String>>,
    public_lookup: bool,
) -> Result<Endpoint, Error> {
    let mode = match relay_urls {
        None => iroh::RelayMode::Default,
        Some(urls) if urls.is_empty() => iroh::RelayMode::Disabled,
        Some(urls) => iroh::RelayMode::custom(
            urls.into_iter()
                .map(|u| u.parse::<iroh::RelayUrl>().map_err(transport))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    };
    let builder = if public_lookup {
        Endpoint::builder(iroh::endpoint::presets::N0)
    } else {
        iroh::endpoint::Builder::empty()
            .crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
    };
    builder
        .secret_key(iroh::SecretKey::from_bytes(&secret))
        .alpns(vec![ALPN.to_vec()])
        .relay_mode(mode)
        .bind()
        .await
        .map_err(transport)
}

#[derive(Clone)]
pub struct IrohTransport {
    pub endpoint: Endpoint,
}

// Closing on cancellation also bounds connections whose response was never read.
struct Connection(iroh::endpoint::Connection);
impl Drop for Connection {
    fn drop(&mut self) {
        self.0.close(0u32.into(), b"request complete");
    }
}

impl PeerTransport for IrohTransport {
    async fn exchange(
        &self,
        local: Identity,
        peer: &Peer,
        command: PeerCommand,
    ) -> Result<PeerReply, Error> {
        let PeerKey::Iroh(key) = peer.key else {
            return Err(harmion_node::Error::IdentityMismatch.into());
        };
        let id = EndpointId::from_bytes(&key).map_err(transport)?;
        let mut address = EndpointAddr::new(id);
        // Empty hint uses configured address lookup. A relay hint is routing only,
        // never authority to change the approved remote public key.
        if !peer.endpoint.is_empty() {
            address = address.with_relay_url(peer.endpoint.parse().map_err(transport)?);
        }
        timeout(DEADLINE, async {
            let connection = Connection(
                self.endpoint
                    .connect(address, ALPN)
                    .await
                    .map_err(transport)?,
            );
            if connection.0.remote_id() != id {
                return Err(Error::InvalidResponse);
            }
            let (mut send, mut recv) = connection.0.open_bi().await.map_err(transport)?;
            let bytes = encode_peer_request(&PeerRequest {
                cluster: local.cluster,
                node: local.node,
                command,
            })?;
            send.write_all(&bytes).await.map_err(transport)?;
            send.finish().map_err(transport)?;
            let response =
                decode_peer_response(&recv.read_to_end(MAX_PEER_BYTES).await.map_err(transport)?)?;
            if response.cluster != local.cluster || response.node != peer.node {
                return Err(Error::InvalidResponse);
            }
            if let PeerReply::Rejected { update_id, reason } = response.reply {
                return Err(Error::RemoteRejected { update_id, reason });
            }
            Ok(response.reply)
        })
        .await
        .map_err(|_| Error::Timeout)?
    }
}

/// Endpoint must advertise ALPN and be configured by the host for discovery and
/// relay policy. No address or key is learned from unauthenticated request fields.
pub async fn serve<S: Store + Send + 'static>(
    endpoint: Endpoint,
    store: SharedStore<S>,
    mut stop: watch::Receiver<bool>,
) -> Result<(), Error> {
    let slots = Arc::new(Semaphore::new(16));
    let mut tasks = tokio::task::JoinSet::new();
    loop {
        let incoming = tokio::select! {
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() { break; }
                continue;
            },
            Some(_) = tasks.join_next(), if !tasks.is_empty() => continue,
            incoming = endpoint.accept() => match incoming { Some(incoming) => incoming, None => break },
        };
        let Ok(permit) = slots.clone().try_acquire_owned() else {
            incoming.refuse();
            continue;
        };
        let store = store.clone();
        tasks.spawn(async move {
            let _permit = permit;
            let _ = timeout(DEADLINE, async {
                let connection = Connection(incoming.await.map_err(transport)?);
                let key = PeerKey::Iroh(*connection.0.remote_id().as_bytes());
                let (mut send, mut recv) = connection.0.accept_bi().await.map_err(transport)?;
                let request = decode_peer_request(
                    &recv.read_to_end(MAX_PEER_BYTES).await.map_err(transport)?,
                )?;
                if let PeerCommand::MembershipAccept(page) = &request.command {
                    crate::peer::validate_membership_page(page)?;
                }
                let reply = store
                    .run_notifying(
                        matches!(&request.command, PeerCommand::Accept(_)),
                        move |s| membership::handle(s, &key, &request),
                    )
                    .await?;
                send.write_all(&encode_peer_response(&reply)?)
                    .await
                    .map_err(transport)?;
                send.finish().map_err(transport)?;
                // Client closes only after reading the response; do not truncate its ACK.
                connection.0.closed().await;
                Ok::<(), Error>(())
            })
            .await;
        });
    }
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    Ok(())
}
