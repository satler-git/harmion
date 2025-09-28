use dashmap::DashMap;
use either::Either::{self, Left, Right};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, RwLock};
use tracing::{error, info, warn};

use ed25519_dalek::SigningKey;
use tokio_util::sync::CancellationToken;

use std::{collections::HashMap, mem};

use crate::{
    webrtc::{
        signal::{
            client::{SignalCError, SignalClient},
            SignalData, SignalInfo, SignalMessage,
        },
        simple::{Connected, Peer, PeerError},
        BUFFER_SIZE,
    },
    Connection, Layer, Message, PeerIndex,
};

// 機能
//
// なるべくblockしないのが目標
//
// dashmap<id(pubkey), peer>
//
// add(&self, peer) -> Result<()>
//  -> 相手と初期の情報を交換する
//  - pubkey
//  - (if sig) siginfo
//
// disconnect(&self, peer) -> Result<()>
//
// send<T>(&self, &peerid, T)
// recv(&self, &peerid) -> Result<Message>
//  -> 必ずpeeridからMessage一つで返す。verifyに失敗しても(Err)
//
// verify
// timestamp(10m(const)) -> verify sign -> replay-cache(10m(const))

type SignalChannels = (
    mpsc::Sender<(PeerIndex, mpsc::Sender<SignalMessage>)>, // 受け取る用
    mpsc::Sender<SignalMessage>,                            // 相手に送る用
);

type SignalRef = Arc<RwLock<Either<SignalClient, (CancellationToken, SignalChannels)>>>;

pub(crate) struct PeerPool {
    id: PeerIndex,
    key: SigningKey,

    map: Arc<DashMap<PeerIndex, (CancellationToken, Peer<Connected>)>>,

    signal: SignalRef,

    center: Option<(mpsc::Sender<Peer<Connected>>, )>,
}

impl PeerPool {
    fn new(id: PeerIndex, key: SigningKey) -> Self {
        PeerPool {
            signal: Arc::new(RwLock::new(Left(SignalClient::new(id, key.clone())))),
            map: Arc::new(DashMap::new()),

            id,
            key,

            center: None,
        }
    }
}

use thiserror::Error;

#[derive(Debug, Error)]
pub(crate) enum PoolError {
    #[error("failed to connect to Signalling service: {0}")]
    Signal(#[from] SignalCError),
    #[error("failed to keep or create connection via WebRTC: {0}")]
    Peer(#[from] PeerError),
    #[error(
        "cannot connect to Signalling service or edit SignalClient when connected to the service"
    )]
    SignalConnected,
    #[error("cannot disconnect when client is not connected to Signalling service")]
    SignalNotConnected,
}

// SignalClient
//
// Sender<(PeerIndex(vs), mpsc::Sender<SignalMessage>)>,
// HashMap<PeerIndex, ..>

type PoolResult<T> = Result<T, PoolError>;

async fn connect_offer(
    offer: webrtc::peer_connection::sdp::session_description::RTCSessionDescription,
    to: PeerIndex,
    origin: PeerIndex,
    signal: SignalChannels,
    token: CancellationToken,
) -> Result<Peer<Connected>, PoolError> {
    let config = crate::webrtc::simple::Config {
        signal: Some({
            let (sender, mut rx) = mpsc::channel(BUFFER_SIZE);
            let (tx, receiver) = mpsc::channel(BUFFER_SIZE);

            let (sig_tx, mut sig_rx) = mpsc::channel(BUFFER_SIZE);
            let _ = signal.0.send((to, sig_tx)).await; // TODO:

            let signal = signal.clone();

            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        msg = sig_rx.recv() => {
                            let Some(msg) = msg else { break };

                            if let SignalData::Ice(ice) = msg.data {
                                let _ = tx.send(ice).await; // TODO:
                            }
                        }
                        msg = rx.recv() => {
                            let Some(ice) = msg else { break };

                            let _ = signal.1.send(SignalMessage { origin, to, data: SignalData::Ice(ice) }).await; // TODO:
                        }
                        _ = token.cancelled() => {
                            break;
                        }
                    }
                }

                token.cancel();
            });

            (sender, receiver)
        }),
        ..Default::default()
    };

    let (peer, answer_sdp) = Peer::<crate::webrtc::simple::WaitingICE>::from_offer(
        offer,
        config,
        &crate::webrtc::simple::PeerAlias::from_key(to.inner()),
    )
    .await?;

    let _ = signal
        .1
        .send(SignalMessage {
            origin,
            to,
            data: SignalData::Sdp(Box::new(answer_sdp)),
        })
        .await; // TODO:

    Ok(peer.wait().await?)
}

// TODO: cancel
async fn per_peer_thread(mut peer: Peer<Connected>, id: PeerIndex, tx: mpsc::Sender<Option<(PeerIndex, Message)>>, mut rx: mpsc::Receiver<Message>) -> PoolResult<()> {
    loop {
        tokio::select! {
            msg = peer.recv()  => {
                let _ = tx.send(msg?.map(|msg| (id, msg))).await; // TODO:
            }
            to_send = rx.recv() => {
                let Some(to_send) = to_send else {
                    warn!("(PeerPool per peer thread)To send has been closed");
                    break
                };

                peer.send(&to_send).await?;
            }
        }
    }

    Ok(())
}

async fn center_thread(
    mut peer_rx: mpsc::Receiver<(PeerIndex, Peer<Connected>)>,
    mut channel_rx: mpsc::Receiver<(PeerIndex, mpsc::Sender<Message>)>,
    mut to_send_rx: mpsc::Receiver<(PeerIndex, Message)>,
    mut bd: broadcast::Sender<(PeerIndex, Message)>,
    cancel: CancellationToken,
) -> PoolResult<()>
{
    let mut channels = HashMap::new();
    let (tx, rx) = mpsc::channel(BUFFER_SIZE); // Option<(peerid, <message>)
    let mut senders = HashMap::new();

    // TODO: log use span
    loop {
        tokio::select! {
            peer = peer_rx.recv() => {
                let Some((id, peer)) = peer else {
                    warn!("peer channel has been closed");
                    break
                };

                let tx = tx.clone();
                let (sender_tx, sender_rx) = mpsc::channel(BUFFER_SIZE);

                // span
                tokio::spawn(async move {
                    if let Err(e) = per_peer_thread(peer, id, tx, sender_rx).await {
                        error!("error happend in per peer thread: {e}");
                    }
                });

                senders.insert(id, sender_tx);
            }
            channel = channel_rx.recv() => {
                let Some((id, channel)) = channel else {
                    warn!("channel channel has been closed");
                    break
                };

                channels.entry(id).or_insert_with(|| vec![]).push(channel);
            }
            to_send = to_send_rx.recv() => {
                let Some((id, msg)) = to_send else {
                    warn!("to_send channel has been closed");
                    break
                };
                if let Some(c) = senders.get_mut(&id) {
                    let _ = c.send(msg).await;
                } else {
                    error!("{id:?} is not connected");
                }
            }
            // TODO: rx -> if !contains then broadcast else forall
        }
    }
    Ok(())
}

impl PeerPool {
    pub async fn start(&mut self) -> PoolResult<()>
    {
        if self.center.is_some() {
            return Ok(());
        }

        let (peer_tx, peer_rx) = mpsc::channel(BUFFER_SIZE);

        tokio::spawn(async move {
            if let Err(e) = center_thread(peer_rx).await {
                error!("error happened in PeerPool center thread: {e}")
            }
        });

        self.center = Some((peer_tx, ));

        // start center thread
        // set Sender<Peer>
        //
        // received Peer -> layer<peer> -> start peer thread
        // HashMap<Id, HashSet<Sender<Message>>>

        // create(wait)
        // broadcast::Sender<(Id, Message)> // if haven't received Sender<Message>
        // mpsc::Receiver<(Id, mpsc::Sender<mpsc::Sender<Message>>)>
        //
        // set Sender<(id, Sender<Message>)>
        Ok(())
    }

    pub async fn connect_to_sig(&self, to: Option<SignalInfo>) -> PoolResult<()> {
        let token = CancellationToken::new();

        let (msg_tx, msg_rx) = mpsc::channel(BUFFER_SIZE);
        let (peer_tx, peer_rx) = mpsc::channel(BUFFER_SIZE);
        let (offer_sdp, mut offer_sdp_rx) = mpsc::channel(BUFFER_SIZE);

        let sig_chan = (peer_tx, msg_tx);

        let sig = {
            let mut sig = self.signal.write().await;

            if sig.is_right() {
                return Err(PoolError::SignalConnected);
            }

            mem::replace(&mut *sig, Right((token.clone(), sig_chan.clone())))
                .left()
                .unwrap()
        };

        let signal_ref = self.signal.clone();

        async fn signal_thread(
            mut sig: SignalClient,
            to: Option<SignalInfo>,
            token: CancellationToken,
            signal_ref: SignalRef,
            mut peer_rx: mpsc::Receiver<(PeerIndex, mpsc::Sender<SignalMessage>)>,
            mut msg_rx: mpsc::Receiver<SignalMessage>,
            offer_sdp: mpsc::Sender<SignalMessage>,
        ) -> Result<(), SignalCError> {
            sig.connect(to).await?;

            let mut peers = HashMap::new();

            loop {
                tokio::select! {
                    msg = peer_rx.recv() => {
                        let Some((id, peer)) = msg else { break };

                        peers.insert(id, peer);
                    }
                    msg = msg_rx.recv() => {
                        let Some(msg) = msg else { break };

                        if let Err(e) = sig.send(msg).await {
                            error!("failed to send SignalMessage via Signalling service {e}");
                        }
                    }
                    msg = sig.recv() => {
                        let Some(msg) = msg? else { break };

                        if let Some(peer) = peers.get_mut(&msg.content.origin) {
                            let _ = peer.send(msg.content).await;
                        } else {
                            info!("received message from disconnected peer: {:?}", msg.content.origin);
                            if msg.content.data.is_sdp() {
                                let _ = offer_sdp.send(msg.content).await;
                            }
                        }
                    }
                    _ = token.cancelled() => {
                        break;
                    }
                }
            }

            *signal_ref.write().await = Left(sig);
            token.cancel();

            Ok(())
        }

        tokio::spawn({
            let token = token.clone();
            async move {
                if let Err(e) =
                    signal_thread(sig, to, token, signal_ref, peer_rx, msg_rx, offer_sdp).await
                {
                    error!("Error on SignalClient, disconnected: {e}");
                }
            }
        });

        tokio::spawn({
            let map = self.map.clone();

            async move {
                loop {
                    tokio::select! {
                        msg = offer_sdp_rx.recv() => {
                            let Some(msg) = msg else { break };

                            if let SignalData::Sdp(sdp) = msg.data {
                                let token = token.child_token();

                                let peer = connect_offer(*sdp, msg.origin ,msg.to, sig_chan.clone(), token.clone()).await.unwrap(); // TODO:
                                map.insert(msg.origin, (token, peer));
                            }

                        }
                        _ = token.cancelled() => { break }
                    }
                }

                token.cancel();
            }
        });

        Ok(())
    }

    pub async fn disconnect_from_sig(&self) -> PoolResult<()> {
        let token = {
            let mut sig = self.signal.write().await;

            if let Right((token, _)) = &mut *sig {
                token.clone()
            } else {
                return Err(PoolError::SignalNotConnected);
            }
        };

        token.cancel();

        Ok(())
    }

    pub async fn add_signal(&mut self, signal: SignalInfo) -> PoolResult<bool> {
        let mut sig = self.signal.write().await;

        if let Left(sig) = &mut *sig {
            Ok(sig.insert_signal(signal))
        } else {
            Err(PoolError::SignalConnected)
        }
    }

    pub fn add(&self, peer: Peer<Connected>, alias: Option<&str>) -> PoolResult<PeerIndex> {
        todo!()
    }

    pub fn connection(&self, peer: PeerIndex) -> impl Connection<Vec<u8>, Vec<u8>> {
        // TODO:
        mpsc::channel(BUFFER_SIZE)
    }

    pub fn connect_by_key(&self, peer: PeerIndex) -> PoolResult<()> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn signal_thread_type() -> Result<(), Box<dyn std::error::Error>> {
        let mut rng = ed25519_dalek::ed25519::signature::rand_core::OsRng;
        let key = ed25519_dalek::SigningKey::generate(&mut rng);

        let pool = super::PeerPool::new((&key).into(), key);
        let _ = pool.connect_to_sig(None).await;

        Ok(())
    }
}
