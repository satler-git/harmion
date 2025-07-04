// TODO: ICE交換
// TODO: refactor
// TODO: rewrite && learning WebRTC

use std::sync::Arc;

use tracing::info_span;
use webrtc::data_channel::RTCDataChannel;
use webrtc::peer_connection::RTCPeerConnection;

use thiserror::Error;

use dashmap::DashMap;

use crate::Message;

use tokio::sync::mpsc;

use std::cell::LazyCell;

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

pub(super) struct SimpleConn {
    peers: DashMap<PeerID, Peer>,
    config: Config,

    rx: mpsc::UnboundedReceiver<(PeerID, Message)>,
    tx: mpsc::UnboundedSender<(PeerID, Message)>,
}

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

impl SimpleConn {
    fn new(config: Config) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();

        Self {
            peers: DashMap::new(),
            config,

            rx,
            tx,
        }
    }

    fn new_with_default() -> Self {
        Self::new(Config::default())
    }
}

#[derive(Debug, Clone)]
pub(super) enum PeerState {
    Connecting,
    Connected,
    Disconnected,
    Failed,
}

struct Peer {
    pc: Arc<RTCPeerConnection>,
    dc: Arc<RTCDataChannel>,
    state: PeerState,
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
    #[error("WebRTC error: {0}")]
    WebRTC(#[from] webrtc::Error),
    #[error("Peer not found: {0}")]
    PeerNotFound(String),
    #[error("Peer not connected: {0}")]
    PeerNotConnected(String),
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("Data channel not available")]
    DataChannelNotAvailable,
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),
}

#[derive(Debug)]
pub(super) struct Connecter {
    peer_id: PeerID,
    pc: Arc<RTCPeerConnection>,
}

impl Connecter {
    pub async fn create_offer(&self) -> Result<String, SimpleError> {
        let offer = self.pc.create_offer(None).await?;
        self.pc.set_local_description(offer.clone()).await?;

        // SDP を JSON 形式で返す
        Ok(serde_json::to_string(&offer)?)
    }

    pub async fn set_remote_answer(&self, answer_sdp: &str) -> Result<(), SimpleError> {
        let answer: webrtc::peer_connection::sdp::session_description::RTCSessionDescription =
            serde_json::from_str(answer_sdp)?;
        self.pc.set_remote_description(answer).await?;
        Ok(())
    }

    pub async fn create_answer(&self, offer_sdp: &str) -> Result<String, SimpleError> {
        let offer: webrtc::peer_connection::sdp::session_description::RTCSessionDescription =
            serde_json::from_str(offer_sdp)?;
        self.pc.set_remote_description(offer).await?;

        let answer = self.pc.create_answer(None).await?;
        self.pc.set_local_description(answer.clone()).await?;

        Ok(serde_json::to_string(&answer)?)
    }

    pub fn peer_id(&self) -> &PeerID {
        &self.peer_id
    }
}

use webrtc::{
    api::{
        interceptor_registry::register_default_interceptors, media_engine::MediaEngine, APIBuilder,
    },
    ice_transport::ice_server::RTCIceServer,
    peer_connection::configuration::RTCConfiguration,
};

// 複数のNodeとコネクションを保持しつつ、1:1の通信をするStruct
impl SimpleConn {
    pub(super) async fn prepare_connect(
        &mut self,
        peer_id: PeerID,
    ) -> Result<Connecter, SimpleError> {
        let mut m = MediaEngine::default();
        m.register_default_codecs()?;

        let registry =
            register_default_interceptors(webrtc::interceptor::registry::Registry::new(), &mut m)?;

        let api = APIBuilder::new()
            .with_media_engine(m)
            .with_interceptor_registry(registry)
            .build();

        let ice_servers = self
            .config
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

        let dc = pc.create_data_channel("data", None).await?;

        {
            let peer_id_clone = peer_id.clone();
            pc.on_peer_connection_state_change(Box::new(move |state| {
                info_span!(
                    "Peer connection state changed",
                    peer_id = peer_id_clone.as_ref(),
                    %state
                );
                Box::pin(async move {})
            }));
        }

        {
            let peer_id_clone = peer_id.clone();

            dc.on_open(Box::new(move || {
                info_span!(
                    "Data channel opened for peer",
                    peer_id = peer_id_clone.as_ref()
                );
                Box::pin(async move {})
            }));
        }

        {
            let message_tx = self.tx.clone();
            let peer_id_clone = peer_id.clone();
            dc.on_message(Box::new(move |msg| {
                let tx = message_tx.clone();
                let peer_id = peer_id_clone.clone();

                Box::pin(async move {
                    if let Ok(message_str) = String::from_utf8(msg.data.to_vec()) {
                        if let Ok(message) = serde_json::from_str::<Message>(&message_str) {
                            let _ = tx.send((peer_id, message));
                        }
                    }
                })
            }));
        }

        let peer = Peer {
            pc: pc.clone(),
            dc,
            state: PeerState::Connecting,
        };

        self.peers.insert(peer_id.clone(), peer);

        Ok(Connecter { peer_id, pc })
    }

    pub async fn send<T: serde::Serialize>(
        &mut self,
        id: PeerID,
        message: T,
    ) -> Result<(), SimpleError> {
        let peer = self
            .peers
            .get(&id)
            .ok_or_else(|| SimpleError::PeerNotFound(id.as_ref().to_string()))?;

        let message_json = serde_json::to_string(&message)?;
        let message_bytes = message_json.into_bytes();

        if peer.dc.ready_state()
            != webrtc::data_channel::data_channel_state::RTCDataChannelState::Open
        {
            return Err(SimpleError::PeerNotConnected(id.as_ref().to_string()));
        }

        peer.dc.send(&message_bytes.into()).await?;

        Ok(())
    }

    pub async fn recv(&mut self) -> Result<(PeerID, Message), SimpleError> {
        self.rx
            .recv()
            .await
            .ok_or_else(|| SimpleError::ConnectionFailed("Message channel closed".to_string()))
    }

    pub async fn disconnect(&mut self, peer_id: &PeerID) -> Result<(), SimpleError> {
        if let Some(peer) = self.peers.remove(peer_id) {
            peer.1.pc.close().await?;
        }
        Ok(())
    }

    pub async fn get_peer_states(&self, peer_id: &PeerID) -> Result<PeerState, SimpleError> {
        self.peers
            .get(&peer_id)
            .ok_or_else(|| SimpleError::PeerNotFound(peer_id.to_string()))
            .map(|state| state.value().state.clone())
    }

    pub async fn wait_for_connection(&self, peer_id: &PeerID) -> Result<(), SimpleError> {
        let mut attempts = 0;
        const MAX_ATTEMPTS: u32 = 50; // * 100ms

        loop {
            if let Some(peer) = self.peers.get(peer_id) {
                if peer.dc.ready_state()
                    == webrtc::data_channel::data_channel_state::RTCDataChannelState::Open
                {
                    return Ok(());
                }
            }

            attempts += 1;
            if attempts >= MAX_ATTEMPTS {
                return Err(SimpleError::ConnectionFailed(format!(
                    "Connection timeout for peer: {}",
                    peer_id.as_ref()
                )));
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_simple_conn() {
        let mut conn1 = SimpleConn::new_with_default();
        let mut conn2 = SimpleConn::new_with_default();

        let peer_id1 = PeerID::new("peer1".to_string());
        let peer_id2 = PeerID::new("peer2".to_string());

        let connecter1 = conn1.prepare_connect(peer_id2.clone()).await.unwrap();
        let connecter2 = conn2.prepare_connect(peer_id1.clone()).await.unwrap();

        let offer = connecter1.create_offer().await.unwrap();
        let answer = connecter2.create_answer(&offer).await.unwrap();
        connecter1.set_remote_answer(&answer).await.unwrap();

        conn1.wait_for_connection(&peer_id2).await.unwrap();
        conn2.wait_for_connection(&peer_id1).await.unwrap();

        let test_message = Message::new("Hello, WebRTC!".to_string());
        conn1.send(peer_id2.clone(), &test_message).await.unwrap();

        if let Ok((from_id, received_message)) = conn2.recv().await {
            assert_eq!(from_id, peer_id1);
            assert_eq!(received_message.content, "Hello, WebRTC!");
        }
    }
}
