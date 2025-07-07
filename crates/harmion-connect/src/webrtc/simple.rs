#![allow(dead_code)]
// TODO: ICE Stateの監視
// TODO: refactor
// TODO: rewrite && learning WebRTC

use serde::{Deserialize, Serialize};

use std::marker::PhantomData;
use std::pin::Pin;

use crate::Message;

use thiserror::Error;

use std::{cell::LazyCell, sync::Arc};
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

pub const GOOGLE_STUN_LIST: LazyCell<Vec<String>> = LazyCell::new(|| {
    vec![
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
    ]
    .iter()
    .map(|&s| s.into())
    .collect()
});

pub(super) struct Config {
    pub stun: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            stun: GOOGLE_STUN_LIST.to_vec(),
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
    pub trait Sealed {}
}

pub struct Peer<S: PeerConnectingState> {
    pc: Arc<RTCPeerConnection>,
    dc: Arc<RwLock<Option<Arc<RTCDataChannel>>>>,
    state: Arc<RwLock<PeerState>>,
    _state: PhantomData<S>,
}

pub trait PeerConnectingState: private::Sealed {}

pub struct WaitingAnswer;
impl private::Sealed for WaitingAnswer {}
impl PeerConnectingState for WaitingAnswer {}

pub struct WaitingICE;
impl private::Sealed for WaitingICE {}
impl PeerConnectingState for WaitingICE {}

pub struct Connected;
impl private::Sealed for Connected {}
impl PeerConnectingState for Connected {}

#[derive(Debug, Clone)]
pub(super) enum PeerState {
    Connecting,
    Connected,
    Disconnected,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum InnerMessage {
    Message(Message),
    ICE(webrtc::ice_transport::ice_candidate::RTCIceCandidateInit), // 接続確立以降は
}

type OnMessageHdlrFn =
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

        let local = pc.local_description().await.unwrap(); // TODO: Error handling

        Ok((peer, serde_json::to_string(&local)?))
    }

    pub async fn from_offer(
        offer: &str,
        on_message: Arc<Mutex<OnMessageHdlrFn>>,
        config: Config,
        peer_id: &PeerID, // only for logging
    ) -> Result<(Peer<WaitingICE>, String), PeerError> {
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

        let local = pc.local_description().await.unwrap(); // TODO: Error handling

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

        let dc = self.dc.write().await;

        if dc.is_none() {
            return Err(PeerError::PeerNotConnected);
        }

        if (*dc).as_ref().map(|dc| dc.ready_state())
            != Some(webrtc::data_channel::data_channel_state::RTCDataChannelState::Open)
        {
            return Err(PeerError::PeerNotConnected);
        }

        let bytes = &message_bytes.into();

        (*dc).as_ref().map(|dc| dc.send(bytes)).unwrap().await?;

        Ok(())
    }

    pub async fn disconnect(&mut self) -> Result<(), PeerError> {
        self.pc.close().await?;
        Ok(())
    }

    pub async fn get_peer_states(&self) -> PeerState {
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

                match serde_json::to_string(&InnerMessage::ICE(msg)) {
                    Ok(message_json) => {
                        let message_bytes = message_json.into_bytes();

                        if dc.ready_state()
                            != webrtc::data_channel::data_channel_state::RTCDataChannelState::Open
                        {
                            warn!("{}", PeerError::PeerNotConnected);
                        }

                        let bytes = &message_bytes.into();

                        match dc.send(bytes).await {
                            Err(e) => error!("failed to send ice data to the another peer: {e}"),
                            _ => (),
                        }
                    }
                    Err(e) => warn!("failed to convert ice to json string: {e}"),
                }
            }
        });
        pc.on_ice_candidate(Box::new(move |ice| {
            match ice.map(|i| i.to_json()) {
                Some(Ok(ice)) => match tx.send(ice) {
                    Err(e) => warn!("ICE Channel has closed: {e}"),
                    _ => (),
                },
                Some(Err(e)) => warn!("failed to convert ice candidate to json: {e}"),
                _ => warn!("ice is none"),
            }
            Box::pin(async {})
        }));

        let (tx, mut rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            // 他でCloneしていないはず
            let mut handler = on_message.lock().await;

            loop {
                let msg: Option<InnerMessage> = rx.recv().await;

                match msg {
                    Some(msg) => match msg {
                        InnerMessage::ICE(ice) => match pc.add_ice_candidate(ice).await {
                            Err(e) => warn!("Failed to add ice candidate: {e}"),
                            _ => (),
                        },
                        InnerMessage::Message(msg) => handler(msg).await,
                    },
                    _ => warn!("OnMessage Channel has closed"),
                }
            }
        });

        dc.on_message(Box::new(move |msg| {
            let data = msg.data.to_vec();

            match String::from_utf8(data) {
                Ok(text) => match serde_json::from_str::<InnerMessage>(&text) {
                    Ok(msg) => match tx.send(msg) {
                        Err(e) => warn!("OnMessage Channel has closed: {e}"),
                        _ => (),
                    },
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
    pub async fn set_remote_answer(self, answer_sdp: &str) -> Result<Peer<Connected>, PeerError> {
        let answer: webrtc::peer_connection::sdp::session_description::RTCSessionDescription =
            serde_json::from_str(answer_sdp)?;
        self.pc.set_remote_description(answer).await?;

        let (tx, rx) = oneshot::channel();
        {
            let dc = self.dc.clone();
            let dc = dc.read().await;
            let dc = dc.as_ref().take().unwrap(); // こっち常にある

            dc.on_open(Box::new(move || {
                let _ = tx.send(());
                Box::pin(async {})
            }));
        }

        rx.await.map_err(|_| PeerError::Other)?;

        Ok(Peer {
            pc: self.pc,
            dc: self.dc,
            state: self.state,
            _state: PhantomData,
        })
    }
}

impl Peer<WaitingICE> {
    pub async fn wait(self) -> Result<Peer<Connected>, PeerError> {
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
    pub fn new(id: String) -> Self {
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

// #[cfg(test)]
// mod tests {
//     use super::*;
//
//     #[tokio::test]
//     async fn test_simple_conn() {
//         let mut conn1 = SimpleConn::new_with_default();
//         let mut conn2 = SimpleConn::new_with_default();
//
//         let peer_id1 = PeerID::new("peer1".to_string());
//         let peer_id2 = PeerID::new("peer2".to_string());
//
//         let connecter1 = conn1.prepare_connect(peer_id2.clone()).await.unwrap();
//         let connecter2 = conn2.prepare_connect(peer_id1.clone()).await.unwrap();
//
//         let offer = connecter1.create_offer().await.unwrap();
//         let answer = connecter2.create_answer(&offer).await.unwrap();
//         connecter1.set_remote_answer(&answer).await.unwrap();
//
//         conn1.wait_for_connection(&peer_id2).await.unwrap();
//         conn2.wait_for_connection(&peer_id1).await.unwrap();
//
//         let test_message = Message::new("Hello, WebRTC!".to_string());
//         conn1.send(peer_id2.clone(), &test_message).await.unwrap();
//
//         let (from_id, received_message) = conn2.recv().await.expect("Failed to receive message");
//
//         assert_eq!(from_id, peer_id1);
//         assert_eq!(received_message.content, "Hello, WebRTC!");
//     }
// }
