use std::marker::PhantomData;

use crate::{webrtc::BUFFER_SIZE, Message};

use thiserror::Error;

use std::sync::Arc;
use tokio::{
    select,
    sync::{mpsc, oneshot, RwLock},
};
use tokio_util::sync::CancellationToken;

use tracing::{debug, error, info, warn};

use webrtc::{
    api::{
        interceptor_registry::register_default_interceptors, media_engine::MediaEngine, APIBuilder,
    },
    data_channel::RTCDataChannel,
    ice_transport::{ice_candidate::RTCIceCandidateInit, ice_server::RTCIceServer},
    peer_connection::{
        configuration::RTCConfiguration, sdp::session_description::RTCSessionDescription,
        RTCPeerConnection,
    },
};

pub const GOOGLE_STUN_LIST: [&str; 1] = [
    "stun:stun.l.google.com:19302",
    // "stun:stun1.l.google.com:19302",
    // "stun:stun2.l.google.com:19302",
    // "stun:stun3.l.google.com:19302",
    // "stun:stun4.l.google.com:19302",
];

type SignalConn = Option<(
    mpsc::Sender<RTCIceCandidateInit>,
    mpsc::Receiver<RTCIceCandidateInit>,
)>;

pub(super) struct Config {
    pub stun: Vec<String>,
    pub signal: SignalConn,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            stun: GOOGLE_STUN_LIST.iter().map(|&s| s.into()).collect(),
            signal: None,
        }
    }
}

#[derive(Error, Debug)]
pub(super) enum PeerError {
    #[error("WebRTC error: {0}")]
    WebRTC(#[from] webrtc::Error),
    #[error("Peer not connected")]
    PeerNotConnected,
    #[error("Serialization error(Message): {0}")]
    MessageSerialize(#[from] rmp_serde::encode::Error),
    #[error("Data channel not available")]
    DataChannelNotAvailable,
    #[error("Connection failed")]
    ConnectionFailed,
    #[error("Something were wrong")]
    Other,
}

pub(crate) mod private {
    pub trait Sealed {}
}

pub(super) struct Peer<S: PeerConnectingState> {
    pc: Arc<RTCPeerConnection>,
    dc: Arc<RwLock<Option<Arc<RTCDataChannel>>>>,
    state: Arc<RwLock<PeerState>>,

    recv: mpsc::Receiver<Message>,

    cancel: CancellationToken,

    ready: Option<mpsc::Receiver<()>>, // from_offerから作ったときに、DataChannelが出来たことの通知用
    _state: PhantomData<S>,
}

pub(super) trait PeerConnectingState: private::Sealed {}

pub(super) struct WaitingAnswer;
impl private::Sealed for WaitingAnswer {}
impl PeerConnectingState for WaitingAnswer {}

pub(super) struct WaitingICE;
impl private::Sealed for WaitingICE {}
impl PeerConnectingState for WaitingICE {}

pub(super) struct Connected;
impl private::Sealed for Connected {}
impl PeerConnectingState for Connected {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PeerState {
    Connecting,
    Connected,
    Disconnected,
    Failed,
}

impl<S: PeerConnectingState> Peer<S> {
    // PeerA
    pub(super) async fn new(
        mut config: Config,
        peer_id: &PeerAlias, // only for logging
    ) -> Result<(Peer<WaitingAnswer>, RTCSessionDescription), PeerError> {
        let non_trickle = config.signal.is_none();
        let cancel = CancellationToken::new();

        let (pc, state) =
            Self::new_peer_connection_with_state(&mut config, peer_id, cancel.clone()).await?;

        let dc = pc.create_data_channel("data", None).await?;

        let (tx, recv) = mpsc::channel(BUFFER_SIZE);

        Self::set_data_channel_callbacks(
            pc.clone(),
            dc.clone(),
            state.clone(),
            peer_id,
            tx,
            cancel.clone(),
            non_trickle,
        );

        let peer = Peer {
            pc: pc.clone(),
            dc: Arc::new(RwLock::new(Some(dc))),
            state: state.clone(),
            ready: None,
            _state: PhantomData,
            recv,
            cancel,
        };

        let offer = pc.create_offer(None).await?;

        pc.set_local_description(offer).await?;

        if non_trickle {
            let mut gather_complete = pc.gathering_complete_promise().await;

            info!("Waiting for ICE gatherring complete");

            let _ = gather_complete.recv().await;
        }

        let local = pc.local_description().await.ok_or(PeerError::Other)?; // Or a more specific error

        Ok((peer, local))
    }

    pub(super) async fn from_offer(
        offer: RTCSessionDescription,
        mut config: Config,
        peer_id: &PeerAlias, // only for logging
    ) -> Result<(Peer<WaitingICE>, RTCSessionDescription), PeerError> {
        let cancel = CancellationToken::new();
        let non_trickle = config.signal.is_none();

        let (pc, state) =
            Self::new_peer_connection_with_state(&mut config, peer_id, cancel.clone()).await?;

        let dc = Arc::new(RwLock::new(None));

        let (tx, ready) = mpsc::channel(1);
        let (message_tx, recv) = mpsc::channel(BUFFER_SIZE);

        {
            let dc_clone = dc.clone();
            let state_clone = state.clone();
            let peer_id_clone = peer_id.clone();
            let pc_clone = pc.clone();
            let cancel_clone = cancel.clone();

            pc.on_data_channel(Box::new(move |dc: Arc<RTCDataChannel>| {
                let dc_clone = dc_clone.clone();
                let state_clone = state_clone.clone();
                let peer_id = peer_id_clone.clone();
                let pc_clone = pc_clone.clone();
                let tx = tx.clone();
                let message_tx = message_tx.clone();
                let cancel = cancel_clone.clone();

                Box::pin(async move {
                    let _ = tx.send(()).await;
                    *dc_clone.write().await = Some(dc.clone());

                    Self::set_data_channel_callbacks(
                        pc_clone,
                        dc.clone(),
                        state_clone,
                        &peer_id,
                        message_tx,
                        cancel,
                        non_trickle,
                    );
                })
            }))
        }

        pc.set_remote_description(offer).await?;

        let answer = pc.create_answer(None).await?;

        pc.set_local_description(answer).await?;

        if non_trickle {
            let mut gather_complete = pc.gathering_complete_promise().await;

            info!("Waiting for ICE gatherring complete");

            let _ = gather_complete.recv().await;
        }

        let local = pc.local_description().await.ok_or(PeerError::Other)?; // Or a more specific error

        let peer = Peer {
            pc: pc.clone(),
            dc,
            state,
            ready: Some(ready),
            _state: PhantomData,
            recv,
            cancel,
        };

        Ok((peer, local))
    }

    pub(super) async fn get_peer_states(&self) -> PeerState {
        self.state.read().await.clone()
    }

    async fn new_peer_connection_with_state(
        config: &mut Config,
        peer_id: &PeerAlias, // only for logging
        cancel: CancellationToken,
    ) -> Result<(Arc<RTCPeerConnection>, Arc<RwLock<PeerState>>), PeerError> {
        let mut m = MediaEngine::default();
        m.register_default_codecs()?;

        let registry =
            register_default_interceptors(webrtc::interceptor::registry::Registry::new(), &mut m)?;

        let api = APIBuilder::new()
            .with_media_engine(m)
            .with_interceptor_registry(registry)
            .build();

        let ice_servers = config
            .stun
            .iter()
            .map(|url| RTCIceServer {
                urls: vec![url.clone()],
                ..Default::default()
            })
            .collect();

        let rtc_config = RTCConfiguration {
            ice_servers,
            ..Default::default()
        };

        let pc = Arc::new(api.new_peer_connection(rtc_config).await?);

        let state = Arc::new(RwLock::new(PeerState::Connecting));

        {
            let peer_id = peer_id.clone();
            let state_clone = state.clone();

            pc.on_peer_connection_state_change(Box::new(move |state| {
                info!(
                    "{peer_id}: Peer connection state changed: {state:?}",
                );


                if state == webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState::Failed {
                    warn!("{peer_id}: Peer Connection has gone to failed exiting");
                }

                let state_clone = state_clone.clone();

                Box::pin(async move {
                    if state == webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState::Failed {
                        *state_clone.write().await = PeerState::Failed;
                    }
                })
            }));
        }

        {
            let peer_id = peer_id.clone();

            pc.on_ice_gathering_state_change(Box::new(move |new_state| {
                info!("{peer_id}: ice gathering state was changed: {new_state:?}",);

                Box::pin(async move {})
            }))
        }

        {
            let peer_id = peer_id.clone();

            pc.on_ice_connection_state_change(Box::new(move |new_state| {
                info!("{peer_id}: ice connection state was changed: {new_state:?}",);

                Box::pin(async move {})
            }))
        }

        if let Some((tx, mut rx)) = config.signal.take() {
            info!("running as trickle");
            {
                let cancel_c = cancel.clone();
                let peer_id = peer_id.clone();
                let pc = pc.clone();

                tokio::spawn(async move {
                    loop {
                        debug!("Into ice receiver loop");

                        select! {
                            _ = cancel_c.cancelled() => {
                                break;
                            }
                            msg = rx.recv() => {
                                let Some(msg) = msg else { break };

                                info!("Adding Ice Candidate");
                                debug!("Candidate: {msg:?}");

                                if let Err(e) = pc.add_ice_candidate(msg).await {
                                    error!("{peer_id}: Failed to add ice candidate: {e}")
                                }
                            }
                        }
                    }

                    info!("existing trickle receiver thread");

                    cancel_c.cancel();
                });
            }

            {
                let peer_id = peer_id.clone();

                pc.on_ice_candidate(Box::new(move |ice| {
                    let tx = tx.clone();
                    let peer_id = peer_id.clone();

                    info!("on_ice_candidate trickle handler was called");

                    Box::pin(async move {
                        match ice.map(|i| i.to_json()) {
                            Some(Ok(ice)) => {
                                debug!("Sending ice: {ice:?}");
                                if let Err(e) = tx.send(ice).await {
                                    warn!("{peer_id}: ICE Channel has closed: {e}")
                                } else {
                                    debug!("Succeed to send");
                                }
                            }
                            Some(Err(e)) => {
                                error!("{peer_id}: failed to convert ice candidate to json: {e}")
                            }
                            _ => warn!("{peer_id}: ice is none"),
                        }
                    })
                }));
            }
        }

        Ok((pc, state))
    }

    fn set_data_channel_callbacks(
        pc: Arc<RTCPeerConnection>,
        dc: Arc<RTCDataChannel>,
        state: Arc<RwLock<PeerState>>,
        peer_id: &PeerAlias,
        message_tx: mpsc::Sender<Message>,
        cancel: CancellationToken,
        non_trickle: bool,
    ) {
        // TODO: peer_idはspan
        {
            let peer_id = peer_id.clone();
            let state_clone = state.clone();

            dc.on_open(Box::new(move || {
                info!("{peer_id}: Data channel opened for peer");

                let state_clone = state_clone.clone();
                Box::pin(async move {
                    *state_clone.write().await = PeerState::Connected;
                })
            }));
        }
        {
            let peer_id = peer_id.clone();
            let state_clone = state.clone();
            let cancel = cancel.clone();

            dc.on_close(Box::new(move || {
                info!("{peer_id}: Data channel closed for peer");

                cancel.cancel();

                let state_clone = state_clone.clone();
                Box::pin(async move {
                    *state_clone.write().await = PeerState::Disconnected;
                })
            }))
        }

        {
            let peer_id = peer_id.clone();

            dc.on_error(Box::new(move |error| {
                error!("{peer_id}: error has occurred in the datachannel: {error:?}",);
                Box::pin(async move {})
            }))
        }

        if non_trickle {
            {
                pc.on_ice_candidate(Box::new(move |ice| {
                    info!("on_ice_candidate handler was called(non-trickle)");
                    debug!("ice candidate: {ice:?}");

                    Box::pin(async move {})
                }));
            }
        }

        {
            let peer_id = peer_id.clone();
            dc.on_message(Box::new(move |msg| {
                let data = msg.data.to_vec();
                let message_tx = message_tx.clone();
                let peer_id = peer_id.clone();

                Box::pin(async move {
                    match rmp_serde::from_slice::<Message>(&data) {
                        Ok(msg) => {
                            if let Err(e) = message_tx.send(msg).await {
                                warn!("{peer_id}: OnMessage Channel has closed: {e}")
                            }
                        }
                        Err(e) => {
                            warn!("{peer_id}: MessagePack deserialize error: {}", e);
                        }
                    }
                })
            }));
        }
    }
}

impl Peer<Connected> {
    pub async fn send(&self, message: &Message) -> Result<(), PeerError> {
        let message = rmp_serde::to_vec(message)?;

        let dc_guard = self.dc.read().await;
        let dc = dc_guard
            .as_ref()
            .cloned()
            .ok_or(PeerError::PeerNotConnected)?;

        if dc.ready_state() != webrtc::data_channel::data_channel_state::RTCDataChannelState::Open {
            return Err(PeerError::PeerNotConnected);
        }

        dc.send(&message.into()).await?;

        Ok(())
    }

    pub fn receiver(&mut self) -> &mut mpsc::Receiver<Message> {
        &mut self.recv
    }

    pub(super) async fn disconnect(mut self) -> Result<(), PeerError> {
        self.pc.close().await?;
        self.cancel.cancel();
        self.recv.close();
        Ok(())
    }
}

impl crate::Connection<Message, Message> for Peer<Connected> {
    type Error = PeerError;

    async fn send(&mut self, value: Message) -> Result<(), Self::Error> {
        Self::send(self, &value).await
    }

    async fn recv(&mut self) -> Result<Option<Message>, Self::Error> {
        Ok(self.receiver().recv().await)
    }
}

impl Peer<WaitingAnswer> {
    pub(super) async fn set_remote_answer(
        self,
        answer_sdp: RTCSessionDescription,
    ) -> Result<Peer<Connected>, PeerError> {
        self.pc.set_remote_description(answer_sdp).await?;

        let (tx, rx) = oneshot::channel();

        {
            let dc = self.dc.clone();
            let dc = dc.read().await;
            let dc = dc.as_ref().unwrap(); // こっち常にある

            if dc.ready_state()
                == webrtc::data_channel::data_channel_state::RTCDataChannelState::Open
            {
                let _ = tx.send(());
            } else {
                dc.on_open(Box::new(move || {
                    let _ = tx.send(());
                    Box::pin(async {})
                }));
            }
        }

        rx.await.map_err(|_| PeerError::Other)?;
        *self.state.write().await = PeerState::Connected;

        Ok(Peer {
            pc: self.pc,
            dc: self.dc,
            state: self.state,
            ready: self.ready,
            recv: self.recv,
            cancel: self.cancel,
            _state: PhantomData,
        })
    }
}

impl Peer<WaitingICE> {
    pub(super) async fn wait(self) -> Result<Peer<Connected>, PeerError> {
        let dc = {
            let _ = self.ready.unwrap().recv().await;

            if let Some(dc) = &*self.dc.read().await {
                dc.clone()
            } else {
                return Err(PeerError::DataChannelNotAvailable);
            }
        };

        let (tx, rx) = oneshot::channel();
        {
            if dc.ready_state()
                == webrtc::data_channel::data_channel_state::RTCDataChannelState::Open
            {
                let _ = tx.send(());
            } else {
                dc.on_open(Box::new(move || {
                    let _ = tx.send(());
                    Box::pin(async {})
                }));
            }
        }

        rx.await.map_err(|_| PeerError::Other)?;

        *(self.state.write().await) = PeerState::Connected;

        Ok(Peer {
            pc: self.pc,
            dc: self.dc,
            state: self.state,
            ready: None,
            recv: self.recv,
            cancel: self.cancel,
            _state: PhantomData,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct PeerAlias(String);

impl PeerAlias {
    pub(super) fn new(id: String) -> Self {
        Self(id)
    }

    pub(super) fn from_key(key: ed25519_dalek::VerifyingKey) -> Self {
        use sha2::Digest;

        let digest = sha2::Sha256::digest(key);
        let s = bs58::encode(&digest[..6]).into_string();

        Self::new(s.chars().take(8).collect())
    }
}

impl std::fmt::Display for PeerAlias {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<str> for PeerAlias {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore]
    async fn offer_answer_connects_peers() {
        let (peer_a_wa, offer_sdp) =
            Peer::<WaitingAnswer>::new(Config::default(), &PeerAlias::new("A".into()))
                .await
                .expect("failed to create peer A");

        let (peer_b_wi, answer_sdp) = Peer::<WaitingICE>::from_offer(
            offer_sdp,
            Config::default(),
            &PeerAlias::new("B".into()),
        )
        .await
        .expect("failed to create peer B");

        let peer_a_c = peer_a_wa.set_remote_answer(answer_sdp).await.unwrap();
        let peer_b_c = peer_b_wi.wait().await.unwrap();

        assert_eq!(peer_a_c.get_peer_states().await, PeerState::Connected);
        assert_eq!(peer_b_c.get_peer_states().await, PeerState::Connected);
    }

    #[tokio::test]
    #[ignore]
    async fn send_and_receive_message() {
        let (peer_a_wa, offer_sdp) =
            Peer::<WaitingAnswer>::new(Config::default(), &PeerAlias::new("A".into()))
                .await
                .unwrap();

        let (peer_b_wi, answer_sdp) = Peer::<WaitingICE>::from_offer(
            offer_sdp,
            Config::default(),
            &PeerAlias::new("B".into()),
        )
        .await
        .unwrap();

        let peer_a_c = peer_a_wa.set_remote_answer(answer_sdp).await.unwrap();
        let mut peer_b_c = peer_b_wi.wait().await.unwrap();

        let payload = "hello from A".to_string();

        let mut rng = ed25519_dalek::ed25519::signature::rand_core::OsRng;

        peer_a_c
            .send(&Message::new(
                payload.clone().into(),
                &ed25519_dalek::SigningKey::generate(&mut rng),
            ))
            .await
            .expect("failed to send");

        let received =
            tokio::time::timeout(std::time::Duration::from_secs(100), peer_b_c.recv.recv())
                .await
                .expect("timeout waiting for message")
                .expect("receiver dropped");

        assert!(received.verify());

        assert_eq!(String::from_utf8(received.content).unwrap(), payload);
    }

    #[tokio::test]
    #[ignore]
    async fn send_and_receive_message_trickle() {
        crate::tests::init_log();

        use super::BUFFER_SIZE;
        let (tx, rx) = mpsc::channel(BUFFER_SIZE);
        let (tx2, rx2) = mpsc::channel(BUFFER_SIZE);

        let (peer_a_wa, offer_sdp) = Peer::<WaitingAnswer>::new(
            Config {
                signal: Some((tx, rx2)),
                ..Default::default()
            },
            &PeerAlias::new("A".into()),
        )
        .await
        .unwrap();

        let (peer_b_wi, answer_sdp) = Peer::<WaitingICE>::from_offer(
            offer_sdp,
            Config {
                signal: Some((tx2, rx)),
                ..Default::default()
            },
            &PeerAlias::new("B".into()),
        )
        .await
        .unwrap();

        let peer_a_c = peer_a_wa.set_remote_answer(answer_sdp).await.unwrap();
        let mut peer_b_c = peer_b_wi.wait().await.unwrap();

        let payload = "hello from A".to_string();

        let mut rng = ed25519_dalek::ed25519::signature::rand_core::OsRng;

        peer_a_c
            .send(&Message::new(
                payload.clone().into(),
                &ed25519_dalek::SigningKey::generate(&mut rng),
            ))
            .await
            .expect("failed to send");

        let received =
            tokio::time::timeout(std::time::Duration::from_secs(100), peer_b_c.recv.recv())
                .await
                .expect("timeout waiting for message")
                .expect("receiver dropped");

        assert!(received.verify());

        assert_eq!(String::from_utf8(received.content).unwrap(), payload);
    }
}
