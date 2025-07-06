#![allow(dead_code)]
// TODO: ICE交換
// TODO: ICE Stateの監視
// TODO: gathering_complete_promise
// TODO: on_ice_connection_state_change
// TODO: refactor
// TODO: rewrite && learning WebRTC

use crate::Message;
use tokio::sync::RwLock;

use thiserror::Error;

use dashmap::DashMap;
use std::{cell::LazyCell, sync::Arc};
use tokio::sync::mpsc;

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
}

struct Peer {
    pc: Arc<RTCPeerConnection>,
    dc: Arc<RTCDataChannel>,
    state: Arc<RwLock<PeerState>>,
}

#[derive(Debug, Clone)]
pub(super) enum PeerState {
    Connecting,
    Connected,
    Disconnected,
    Failed,
}

impl Peer {
    // PeerA
    pub(super) async fn prepare_connect(
        &mut self,
        on_message: webrtc::data_channel::OnMessageHdlrFn,
        config: Config,
        peer_id: PeerID, // only for logging
    ) -> Result<(Self, Connecter), PeerError> {
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
                        warn!("Peer Connection has gone to failed exiting");
                        *state_clone.write().await = PeerState::Failed;
                    }
                })
            }));
        }

        let dc = pc.create_data_channel("data", None).await?;

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

        dc.on_message(on_message);

        let peer = Peer {
            pc: pc.clone(),
            dc,
            state: state.clone(),
        };

        Ok((peer, Connecter { pc, state }))
    }

    // TODO: PeerB(受け)
    // Connecterも変えると分かりやすい

    pub async fn send<T: serde::Serialize>(&self, message: T) -> Result<(), PeerError> {
        let message_json = serde_json::to_string(&message)?;
        let message_bytes = message_json.into_bytes();

        if self.dc.ready_state()
            != webrtc::data_channel::data_channel_state::RTCDataChannelState::Open
        {
            return Err(PeerError::PeerNotConnected);
        }

        self.dc.send(&message_bytes.into()).await?;

        Ok(())
    }

    pub async fn disconnect(&mut self) -> Result<(), PeerError> {
        self.pc.close().await?;
        Ok(())
    }

    pub async fn get_peer_states(&self) -> PeerState {
        self.state.read().await.clone()
    }
}

#[derive(Debug)]
pub(super) struct Connecter {
    state: Arc<RwLock<PeerState>>,
    pc: Arc<RTCPeerConnection>,
}

impl Connecter {
    // 1. PeerA
    pub async fn create_offer(&self) -> Result<String, PeerError> {
        let offer = self.pc.create_offer(None).await?;
        self.pc.set_local_description(offer.clone()).await?;

        // SDP を JSON 形式で返す
        Ok(serde_json::to_string(&offer)?)
    }

    // 2. PeerB
    pub async fn create_answer(&self, offer_sdp: &str) -> Result<String, PeerError> {
        let offer: webrtc::peer_connection::sdp::session_description::RTCSessionDescription =
            serde_json::from_str(offer_sdp)?;
        self.pc.set_remote_description(offer).await?;

        let answer = self.pc.create_answer(None).await?;
        self.pc.set_local_description(answer.clone()).await?;

        Ok(serde_json::to_string(&answer)?)
    }

    // 3. PeerA
    pub async fn set_remote_answer(&self, answer_sdp: &str) -> Result<(), PeerError> {
        let answer: webrtc::peer_connection::sdp::session_description::RTCSessionDescription =
            serde_json::from_str(answer_sdp)?;
        self.pc.set_remote_description(answer).await?;
        Ok(())
    }

    // 4. TODO: ICE
    // 最初だけConnecterを通す。それ以降はDataChannelで送る
    // -> どこで?
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct PeerID(String);

impl PeerID {
    pub fn new(id: String) -> Self {
        Self(id)
    }

    pub fn to_string(&self) -> String {
        self.0.clone()
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

pub(super) struct SimpleConn {
    peers: DashMap<PeerID, Peer>,
    config: Config,

    rx: Arc<RwLock<mpsc::UnboundedReceiver<(PeerID, Message)>>>,
    tx: mpsc::UnboundedSender<(PeerID, Message)>,
}

impl SimpleConn {
    fn new(config: Config) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();

        Self {
            peers: DashMap::new(),
            config,

            rx: Arc::new(RwLock::new(rx)),
            tx,
        }
    }

    fn new_with_default() -> Self {
        Self::new(Config::default())
    }
}

// 複数のNodeとコネクションを保持しつつ、1:1の通信をするStruct
impl SimpleConn {}

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
