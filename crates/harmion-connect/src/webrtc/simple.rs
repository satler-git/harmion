#![allow(dead_code)] // TODO: remove this
                     // TODO: ICE Stateの監視
                     // TODO: refactor
                     // TODO: rewrite && learning WebRTC

use serde::{Deserialize, Serialize};

use std::marker::PhantomData;
use std::pin::Pin;

use crate::Message;

use thiserror::Error;

use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Mutex, RwLock};

use tracing::{error, info_span, warn};

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
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("Data channel not available")]
    DataChannelNotAvailable,
    #[error("Connection failed")]
    ConnectionFailed,
    #[error("Something were wrong")]
    Other,
}

pub(crate) mod private {
    #![allow(dead_code)]
    pub trait Sealed {}
}

pub(super) struct Peer<S: PeerConnectingState> {
    pc: Arc<RTCPeerConnection>,
    dc: Arc<RwLock<Option<Arc<RTCDataChannel>>>>,
    state: Arc<RwLock<PeerState>>,
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
    Message(Message),
    Ice(webrtc::ice_transport::ice_candidate::RTCIceCandidateInit), // 接続確立以降は
}

pub(super) type OnMessageHdlrFn =
    Box<dyn FnMut(Message) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> + Send + Sync>;

impl<S: PeerConnectingState> Peer<S> {
    // PeerA
    pub(super) async fn new(
        on_message: OnMessageHdlrFn,
        config: Config,
        peer_id: &PeerID, // only for logging
    ) -> Result<(Peer<WaitingAnswer>, String), PeerError> {
        let (pc, state) = Self::new_peer_connection_with_state(config, peer_id).await?;

        let dc = pc.create_data_channel("data", None).await?;

        let on_message = Arc::new(Mutex::new(on_message));
        Self::set_data_channel_callbacks(
            pc.clone(),
            dc.clone(),
            state.clone(),
            on_message,
            peer_id,
        )
        .await;

        let peer = Peer {
            pc: pc.clone(),
            dc: Arc::new(RwLock::new(Some(dc))),
            state: state.clone(),
            _state: PhantomData,
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
        on_message: OnMessageHdlrFn,
        config: Config,
        peer_id: &PeerID, // only for logging
    ) -> Result<(Peer<WaitingICE>, String), PeerError> {
        let on_message = Arc::new(Mutex::new(on_message));

        let (pc, state) = Self::new_peer_connection_with_state(config, peer_id).await?;

        let dc = Arc::new(RwLock::new(None));

        {
            let dc_clone = dc.clone();
            let state_clone = state.clone();
            let peer_id_clone = peer_id.clone();
            let pc_clone = pc.clone();

            pc.on_data_channel(Box::new(move |dc: Arc<RTCDataChannel>| {
                let dc_clone = dc_clone.clone();
                let state_clone = state_clone.clone();
                let on_message = on_message.clone();
                let peer_id = peer_id_clone.clone();
                let pc_clone = pc_clone.clone();

                Box::pin(async move {
                    *dc_clone.write().await = Some(dc.clone());

                    Self::set_data_channel_callbacks(
                        pc_clone,
                        dc,
                        state_clone,
                        on_message,
                        &peer_id,
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
            _state: PhantomData,
        };

        Ok((peer, serde_json::to_string(&local)?))
    }

    pub async fn send(&self, message: String) -> Result<(), PeerError> {
        let message_json = serde_json::to_string(&InnerMessage::Message(Message::new(message)))?;
        let message_bytes = message_json.into_bytes();

        let dc_guard = self.dc.read().await;
        let dc = dc_guard.as_ref().ok_or(PeerError::PeerNotConnected)?;

        if dc.ready_state() != webrtc::data_channel::data_channel_state::RTCDataChannelState::Open {
            return Err(PeerError::PeerNotConnected);
        }

        let bytes = &message_bytes.into();
        (*dc).send(bytes).await?;

        Ok(())
    }

    pub(super) async fn disconnect(&mut self) -> Result<(), PeerError> {
        self.pc.close().await?;
        Ok(())
    }

    pub(super) async fn get_peer_states(&self) -> PeerState {
        self.state.read().await.clone()
    }

    async fn new_peer_connection_with_state(
        config: Config,
        peer_id: &PeerID, // only for logging
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
            let peer_id_clone = peer_id.clone();
            let state_clone = state.clone();

            pc.on_peer_connection_state_change(Box::new(move |state| {
                info_span!(
                    "Peer connection state changed",
                    peer_id = peer_id_clone.as_ref(),
                    %state
                );


                if state == webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState::Failed {
                    warn!("Peer Connection has gone to failed exiting");
                }

                let state_clone = state_clone.clone();

                Box::pin(async move {
                    if state == webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState::Failed {
                        *state_clone.write().await = PeerState::Failed;
                    }
                })
            }));
        }

        Ok((pc, state))
    }

    async fn set_data_channel_callbacks(
        pc: Arc<RTCPeerConnection>,
        dc: Arc<RTCDataChannel>,
        state: Arc<RwLock<PeerState>>,
        on_message: Arc<Mutex<OnMessageHdlrFn>>,
        peer_id: &PeerID,
    ) {
        {
            let peer_id_clone = peer_id.clone();
            let state_clone = state.clone();

            dc.on_open(Box::new(move || {
                info_span!(
                    "Data channel opened for peer",
                    peer_id = peer_id_clone.as_ref()
                );

                let state_clone = state_clone.clone();
                Box::pin(async move {
                    *state_clone.write().await = PeerState::Connected;
                })
            }));
        }
        {
            let peer_id_clone = peer_id.clone();
            let state_clone = state.clone();

            dc.on_close(Box::new(move || {
                info_span!(
                    "Data channel closed for peer",
                    peer_id = peer_id_clone.as_ref()
                );
                let state_clone = state_clone.clone();
                Box::pin(async move {
                    *state_clone.write().await = PeerState::Disconnected;
                })
            }))
        }

        {
            let peer_id_clone = peer_id.clone();

            dc.on_error(Box::new(move |error| {
                error!(
                    "error has occurred in the datachannel {}: {:?}",
                    peer_id_clone.as_ref(),
                    error
                );
                Box::pin(async move {})
            }))
        }

        let (tx, mut rx) = mpsc::unbounded_channel();
        let dc_clone = dc.clone();
        tokio::spawn(async move {
            loop {
                let msg = rx.recv().await;
                let dc = dc_clone.clone();

                if msg.is_none() {
                    warn!("ICE Channel has closed");
                    break;
                }

                let msg = msg.unwrap();

                match serde_json::to_string(&InnerMessage::Ice(msg)) {
                    Ok(message_json) => {
                        let message_bytes = message_json.into_bytes();

                        if dc.ready_state()
                            != webrtc::data_channel::data_channel_state::RTCDataChannelState::Open
                        {
                            warn!("{}", PeerError::PeerNotConnected);
                        }

                        let bytes = &message_bytes.into();

                        if let Err(e) = dc.send(bytes).await {
                            error!("failed to send ice data to the another peer: {e}")
                        }
                    }
                    Err(e) => warn!("failed to convert ice to json string: {e}"),
                }
            }
        });
        pc.on_ice_candidate(Box::new(move |ice| {
            match ice.map(|i| i.to_json()) {
                Some(Ok(ice)) => {
                    if let Err(e) = tx.send(ice) {
                        warn!("ICE Channel has closed: {e}")
                    }
                }
                Some(Err(e)) => warn!("failed to convert ice candidate to json: {e}"),
                _ => warn!("ice is none"),
            }
            Box::pin(async {})
        }));

        let (tx, mut rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            // 他でCloneしていないはず
            let mut handler = on_message.lock().await;

            while let Some(msg) = rx.recv().await {
                match msg {
                    InnerMessage::Ice(ice) => {
                        if let Err(e) = pc.add_ice_candidate(ice).await {
                            warn!("Failed to add ice candidate: {e}")
                        }
                    }
                    InnerMessage::Message(msg) => handler(msg).await,
                }
            }
            warn!("OnMessage Channel has closed");
        });

        dc.on_message(Box::new(move |msg| {
            let data = msg.data.to_vec();

            match String::from_utf8(data) {
                Ok(text) => match serde_json::from_str::<InnerMessage>(&text) {
                    Ok(msg) => {
                        if let Err(e) = tx.send(msg) {
                            warn!("OnMessage Channel has closed: {e}")
                        }
                    }
                    Err(e) => {
                        warn!("JSON deserialize error: {}", e);
                    }
                },
                Err(e) => {
                    warn!("Invalid UTF-8 sequence: {}", e);
                }
            }

            Box::pin(async move {})
        }));
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
        *(self.state.write().await) = PeerState::Connected;

        Ok(Peer {
            pc: self.pc,
            dc: self.dc,
            state: self.state,
            _state: PhantomData,
        })
    }
}

impl Peer<WaitingICE> {
    pub(super) async fn wait(self) -> Result<Peer<Connected>, PeerError> {
        let dc = loop {
            if let Some(dc) = &*self.dc.read().await {
                break dc.clone();
            }
            tokio::task::yield_now().await;
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
            _state: PhantomData,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct PeerID(String);

impl PeerID {
    pub(super) fn new(id: String) -> Self {
        Self(id)
    }
}

impl std::fmt::Display for PeerID {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<str> for PeerID {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[derive(Error, Debug)]
pub(super) enum SimpleError {
    #[error("{0}: {1:?}")]
    Peer(#[source] PeerError, PeerID),
}

// pub(super) struct SimpleConn {
//     peers: DashMap<PeerID, Peer>,
//     config: Config,
//
//     rx: Arc<RwLock<mpsc::UnboundedReceiver<(PeerID, Message)>>>,
//     tx: mpsc::UnboundedSender<(PeerID, Message)>,
// }
//
// impl SimpleConn {
//     fn new(config: Config) -> Self {
//         let (tx, rx) = mpsc::unbounded_channel();
//
//         Self {
//             peers: DashMap::new(),
//             config,
//
//             rx: Arc::new(RwLock::new(rx)),
//             tx,
//         }
//     }
//
//     fn new_with_default() -> Self {
//         Self::new(Config::default())
//     }
// }

// 複数のNodeとコネクションを保持しつつ、1:1の通信をするStruct
// impl SimpleConn

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::oneshot;

    // ダミー on_message ハンドラ。受信したメッセージを oneshot で返す。
    fn make_on_message_handler(mut tx: Option<oneshot::Sender<String>>) -> OnMessageHdlrFn {
        Box::new(move |msg: Message| {
            let tx = tx.take();

            Box::pin(async move {
                let _ = tx.unwrap().send(msg.content.to_owned());
            })
        })
    }

    #[tokio::test]
    #[ignore]
    async fn offer_answer_connects_peers() {
        let (peer_a_wa, offer_sdp) = Peer::<WaitingAnswer>::new(
            Box::new(|_| Box::pin(async {})),
            Config::default(),
            &PeerID::new("A".into()),
        )
        .await
        .expect("failed to create peer A");

        let (peer_b_wi, answer_sdp) = Peer::<WaitingICE>::from_offer(
            &offer_sdp,
            Box::new(|_| Box::pin(async {})),
            Config::default(),
            &PeerID::new("B".into()),
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
        let (peer_a_wa, offer_sdp) = Peer::<WaitingAnswer>::new(
            Box::new(|_| Box::pin(async {})),
            Config::default(),
            &PeerID::new("A".into()),
        )
        .await
        .unwrap();

        let (tx, rx) = oneshot::channel();
        let on_msg_handler = make_on_message_handler(Some(tx));

        let (peer_b_wi, answer_sdp) = Peer::<WaitingICE>::from_offer(
            &offer_sdp,
            on_msg_handler,
            Config::default(),
            &PeerID::new("B".into()),
        )
        .await
        .unwrap();

        let peer_a_c = peer_a_wa.set_remote_answer(&answer_sdp).await.unwrap();
        let _peer_b_c = peer_b_wi.wait().await.unwrap();

        let payload = "hello from A".to_string();
        peer_a_c
            .send(payload.clone())
            .await
            .expect("failed to send");

        let received = tokio::time::timeout(std::time::Duration::from_secs(100), rx)
            .await
            .expect("timeout waiting for message")
            .expect("receiver dropped");
        assert_eq!(received, payload);
    }
}
