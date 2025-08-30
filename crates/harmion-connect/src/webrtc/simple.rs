use serde::{Deserialize, Serialize};

use std::marker::PhantomData;

use crate::{webrtc::BUFFER_SIZE, Message};

use thiserror::Error;

use std::sync::Arc;
use tokio::{
    select,
    sync::{mpsc, oneshot, RwLock},
};
use tokio_util::sync::CancellationToken;

use tracing::{error, info, warn};

use webrtc::{
    api::{
        interceptor_registry::register_default_interceptors, media_engine::MediaEngine, APIBuilder,
    },
    data_channel::RTCDataChannel,
    ice_transport::ice_server::RTCIceServer,
    peer_connection::{configuration::RTCConfiguration, RTCPeerConnection},
};

pub const GOOGLE_STUN_LIST: [&str; 10] = [
    "stun:stun.1.google.com:19302",
    "stun:stun.1.google.com:5349",
    "stun:stun1.1.google.com:3478",
    "stun:stun1.1.google.com:5349",
    "stun:stun2.1.google.com:19302",
    "stun:stun2.1.google.com:5349",
    "stun:stun3.1.google.com:3478",
    "stun:stun3.1.google.com:5349",
    "stun:stun4.1.google.com:19302",
    "stun:stun4.1.google.com:5349",
];

pub(super) struct Config {
    pub stun: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            stun: GOOGLE_STUN_LIST.iter().map(|&s| s.into()).collect(),
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
    #[error("SDP JSON serialization/deserialization error: {0}")]
    SDPJson(#[from] serde_json::Error),
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

#[derive(Debug, Clone, Serialize, Deserialize)]
enum InnerMessage {
    Message(Box<Message>),
    Ice(webrtc::ice_transport::ice_candidate::RTCIceCandidateInit), // 接続確立以降は
}

impl<S: PeerConnectingState> Peer<S> {
    // PeerA
    pub(super) async fn new(
        config: Config,
        peer_id: &PeerAlias, // only for logging
    ) -> Result<(Peer<WaitingAnswer>, String), PeerError> {
        let (pc, state) = Self::new_peer_connection_with_state(config, peer_id).await?;

        let dc = pc.create_data_channel("data", None).await?;

        let cancel = CancellationToken::new();

        let (tx, recv) = mpsc::channel(BUFFER_SIZE);

        Self::set_data_channel_callbacks(
            pc.clone(),
            dc.clone(),
            state.clone(),
            peer_id,
            tx,
            cancel.clone(),
        )
        .await;

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

        let mut gather_complete = pc.gathering_complete_promise().await; // TODO: trickle ICE

        pc.set_local_description(offer.clone()).await?;

        let _ = gather_complete.recv().await;

        let local = pc.local_description().await.ok_or(PeerError::Other)?; // Or a more specific error

        Ok((peer, serde_json::to_string(&local)?))
    }

    pub(super) async fn from_offer(
        offer: &str,
        config: Config,
        peer_id: &PeerAlias, // only for logging
    ) -> Result<(Peer<WaitingICE>, String), PeerError> {
        let (pc, state) = Self::new_peer_connection_with_state(config, peer_id).await?;

        let dc = Arc::new(RwLock::new(None));

        let (tx, ready) = mpsc::channel(1);
        let (message_tx, recv) = mpsc::channel(BUFFER_SIZE);

        let cancel = CancellationToken::new();

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
                        dc,
                        state_clone,
                        &peer_id,
                        message_tx,
                        cancel,
                    )
                    .await;
                })
            }))
        }

        let offer: webrtc::peer_connection::sdp::session_description::RTCSessionDescription =
            serde_json::from_str(offer)?;

        pc.set_remote_description(offer).await?;

        let answer = pc.create_answer(None).await?;

        let mut gather_complete = pc.gathering_complete_promise().await; // TODO: trickle ICE

        pc.set_local_description(answer.clone()).await?;

        let local = pc.local_description().await.ok_or(PeerError::Other)?; // Or a more specific error

        let _ = gather_complete.recv().await;

        let peer = Peer {
            pc: pc.clone(),
            dc,
            state,
            ready: Some(ready),
            _state: PhantomData,
            recv,
            cancel,
        };

        Ok((peer, serde_json::to_string(&local)?))
    }

    pub(super) async fn get_peer_states(&self) -> PeerState {
        self.state.read().await.clone()
    }

    async fn new_peer_connection_with_state(
        config: Config,
        peer_id: &PeerAlias, // only for logging
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

        let config = RTCConfiguration {
            ice_servers,
            ..Default::default()
        };

        let pc = Arc::new(api.new_peer_connection(config).await?);

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

        Ok((pc, state))
    }

    async fn set_data_channel_callbacks(
        pc: Arc<RTCPeerConnection>,
        dc: Arc<RTCDataChannel>,
        state: Arc<RwLock<PeerState>>,
        peer_id: &PeerAlias,
        message_tx: mpsc::Sender<Message>,
        cancel: CancellationToken,
    ) {
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

        let (tx, mut rx) = mpsc::channel(BUFFER_SIZE);

        {
            let cancel_c = cancel.clone();
            let peer_id = peer_id.clone();
            let dc_clone = dc.clone();

            tokio::spawn(async move {
                loop {
                    select! {
                        _ = cancel_c.cancelled() => {
                            break;
                        }
                        Some(msg) = rx.recv() => {
                            let dc = dc_clone.clone();

                            match rmp_serde::to_vec(&InnerMessage::Ice(msg)) {
                                // TODO: via Signal?
                                Ok(data) => {
                                    if dc.ready_state()
                                        != webrtc::data_channel::data_channel_state::RTCDataChannelState::Open
                                    {
                                        warn!("{peer_id}: {}", PeerError::PeerNotConnected);
                                    }

                                    if let Err(e) = dc.send(&data.into()).await {
                                        error!("{peer_id}: failed to send ice data to the another peer: {e}")
                                    }
                                }
                                Err(e) => warn!("{peer_id}: failed to convert ice to MessagePack: {e}"),
                            }
                        }
                    }
                }

                cancel_c.cancel();
            });
        }

        {
            let peer_id = peer_id.clone();
            pc.on_ice_candidate(Box::new(move |ice| {
                let tx = tx.clone();
                let peer_id = peer_id.clone();

                Box::pin(async move {
                    match ice.map(|i| i.to_json()) {
                        Some(Ok(ice)) => {
                            if let Err(e) = tx.send(ice).await {
                                warn!("{peer_id}: ICE Channel has closed: {e}")
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

        let (tx, mut rx) = mpsc::channel(BUFFER_SIZE);

        {
            let cancel_c = cancel.clone();
            let peer_id = peer_id.clone();

            tokio::spawn(async move {
                // 他でCloneしていないはず
                loop {
                    select! {
                        _ = cancel_c.cancelled() => {
                            break;
                        }
                        Some(msg) = rx.recv() => {
                            match msg {
                                InnerMessage::Ice(ice) => {
                                    if let Err(e) = pc.add_ice_candidate(ice).await {
                                        error!("{peer_id}: Failed to add ice candidate: {e}")
                                    }
                                }
                                InnerMessage::Message(msg) => {
                                    if let Err(e) = message_tx.send(*msg).await {
                                        error!("{peer_id}: failed to send message to message_tx: {e}");
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }

                cancel_c.cancel();
            });
        }

        {
            let peer_id = peer_id.clone();
            dc.on_message(Box::new(move |msg| {
                let data = msg.data.to_vec();
                let tx = tx.clone();
                let peer_id = peer_id.clone();

                Box::pin(async move {
                    match rmp_serde::from_slice::<InnerMessage>(&data) {
                        Ok(msg) => {
                            if let Err(e) = tx.send(msg).await {
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
    pub async fn send(&self, message: Message) -> Result<(), PeerError> {
        let message = rmp_serde::to_vec(&InnerMessage::Message(Box::new(message)))?;

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
        Self::send(self, value).await
    }

    async fn recv(&mut self) -> Result<Option<Message>, Self::Error> {
        Ok(self.receiver().recv().await)
    }
}

impl Peer<WaitingAnswer> {
    pub(super) async fn set_remote_answer(
        self,
        answer_sdp: &str,
    ) -> Result<Peer<Connected>, PeerError> {
        let answer: webrtc::peer_connection::sdp::session_description::RTCSessionDescription =
            serde_json::from_str(answer_sdp)?;
        self.pc.set_remote_description(answer).await?;

        let (tx, rx) = oneshot::channel();
        {
            let dc = self.dc.clone();
            let dc = dc.read().await;
            let dc = dc.as_ref().unwrap(); // こっち常にある

            dc.on_open(Box::new(move || {
                let _ = tx.send(());
                Box::pin(async {})
            }));
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
            dc.on_open(Box::new(move || {
                let _ = tx.send(());
                Box::pin(async {})
            }));
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
            &offer_sdp,
            Config::default(),
            &PeerAlias::new("B".into()),
        )
        .await
        .expect("failed to create peer B");

        let peer_a_c = peer_a_wa.set_remote_answer(&answer_sdp).await.unwrap();
        let peer_b_c = peer_b_wi.wait().await.unwrap();

        assert_eq!(peer_a_c.get_peer_states().await, PeerState::Connected);
        assert_eq!(peer_b_c.get_peer_states().await, PeerState::Connected);
    }

    #[tokio::test]
    #[ignore]
    async fn send_and_receive_message() {
        // Offer/Answer のセットアップ（省略可：先のテストを呼び出しても OK）
        let (peer_a_wa, offer_sdp) =
            Peer::<WaitingAnswer>::new(Config::default(), &PeerAlias::new("A".into()))
                .await
                .unwrap();

        let (peer_b_wi, answer_sdp) = Peer::<WaitingICE>::from_offer(
            &offer_sdp,
            Config::default(),
            &PeerAlias::new("B".into()),
        )
        .await
        .unwrap();

        let peer_a_c = peer_a_wa.set_remote_answer(&answer_sdp).await.unwrap();
        let mut peer_b_c = peer_b_wi.wait().await.unwrap();

        let payload = "hello from A".to_string();

        let mut rng = ed25519_dalek::ed25519::signature::rand_core::OsRng;

        peer_a_c
            .send(Message::new(
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

#[cfg(test)]
mod more_unit_tests_for_simple {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use ed25519_dalek::{SigningKey, VerifyingKey};

    // Helper to build a deterministic signing key from a fixed 32-byte seed.
    fn signing_key_from_seed(seed: [u8; 32]) -> SigningKey {
        SigningKey::from_bytes(&seed)
    }

    // Helper that mirrors the PeerAlias::from_key behavior to compute the expected alias.
    fn compute_expected_alias_from_key(vk: &VerifyingKey) -> String {
        use sha2::Digest;
        let digest = sha2::Sha256::digest(vk);
        let s = bs58::encode(&digest[..6]).into_string();
        s.chars().take(8).collect::<String>()
    }

    #[test]
    fn peer_alias_new_keeps_string_and_display_matches() {
        let alias = PeerAlias::new("ABCDEFGH".to_string());
        assert_eq!(alias.as_ref(), "ABCDEFGH");
        assert_eq!(format!("{}", alias), "ABCDEFGH");
        // Clone/Eq
        let alias2 = alias.clone();
        assert_eq!(alias, alias2);
    }

    #[test]
    fn peer_alias_from_key_has_length_8_and_valid_base58_chars() {
        // Fixed deterministic key
        let sk = signing_key_from_seed([0x11; 32]);
        let vk = sk.verifying_key();
        let alias = PeerAlias::from_key(vk);

        // Length should be exactly 8 as implementation takes first 8 chars.
        assert_eq!(alias.as_ref().len(), 8, "alias must be 8 chars long");

        // Base58 alphabet (Bitcoin), the alias is a prefix so characters must belong to this set.
        let b58_chars = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
        assert!(alias
            .as_ref()
            .chars()
            .all(|c| b58_chars.contains(c)),
            "alias contains only base58 characters");
    }

    #[test]
    fn peer_alias_from_key_is_deterministic_and_matches_internal_logic() {
        let sk = signing_key_from_seed([0xAB; 32]);
        let vk = sk.verifying_key();
        let alias1 = PeerAlias::from_key(vk);
        let alias2 = PeerAlias::from_key(vk);
        assert_eq!(alias1, alias2, "same key must produce identical alias");

        // Validate against independently computed expected value
        let expected = compute_expected_alias_from_key(&vk);
        assert_eq!(alias1.as_ref(), expected, "alias must match expected computation");
    }

    #[test]
    fn peer_alias_from_different_keys_produces_different_aliases_in_general() {
        // While collisions are theoretically possible, with SHA-256 and different inputs this should differ.
        let sk1 = signing_key_from_seed([0x01; 32]);
        let sk2 = signing_key_from_seed([0x02; 32]);
        let a1 = PeerAlias::from_key(sk1.verifying_key());
        let a2 = PeerAlias::from_key(sk2.verifying_key());

        // Avoid overly brittle assertion by checking not equal; if equal, print for diagnostics.
        assert_ne!(a1, a2, "different keys should yield different aliases (got {} and {})", a1, a2);
    }

    #[test]
    fn peer_alias_hash_and_eq_work_in_collections() {
        let mut set = HashSet::new();
        set.insert(PeerAlias::new("AAAAAAA1".into()));
        set.insert(PeerAlias::new("AAAAAAA1".into())); // duplicate
        set.insert(PeerAlias::new("BBBBBBB2".into()));
        assert_eq!(set.len(), 2, "Hash + Eq should ensure uniqueness");

        let mut map = HashMap::new();
        map.insert(PeerAlias::new("CCCCCCC3".into()), 10usize);
        map.insert(PeerAlias::new("CCCCCCC3".into()), 20usize); // overwrite
        assert_eq!(map.get(&PeerAlias::new("CCCCCCC3".into())), Some(&20usize));
    }

    #[test]
    fn peer_state_equality_and_variants() {
        assert_ne!(PeerState::Connecting, PeerState::Connected);
        assert_ne!(PeerState::Connected, PeerState::Disconnected);
        assert_ne!(PeerState::Disconnected, PeerState::Failed);
        assert_eq!(PeerState::Failed, PeerState::Failed);
    }

    #[test]
    fn peer_alias_display_consistency_with_as_ref() {
        let sk = signing_key_from_seed([0x55; 32]);
        let alias = PeerAlias::from_key(sk.verifying_key());
        assert_eq!(alias.as_ref(), format!("{}", alias));
    }
}
