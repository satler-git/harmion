use std::sync::Arc;

use webrtc::data_channel::RTCDataChannel;
use webrtc::peer_connection::RTCPeerConnection;

use thiserror::Error;

use dashmap::DashMap;

const STUN_LIST: [&str; 10] = [
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

pub(super) struct SimpleConn {
    peers: DashMap<PeerID, Peer>,
}

struct Config {
    pub stun: Vec<String>,
}

impl SimpleConn {
    fn new(config: Config) -> Self {
        todo!()
    }
}

struct Peer {
    pc: Arc<RTCPeerConnection>,
    dc: Arc<RTCDataChannel>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct PeerID(String);

#[derive(Error, Debug)]
pub(super) enum SimpleError {}

pub(super) struct Message;

impl SimpleConn {
    pub(super) async fn send<T: serde::Serialize>(
        &mut self,
        id: PeerID,
        message: T,
    ) -> Result<(), SimpleError> {
        todo!()
    }

    pub(super) async fn recv(&mut self) -> Result<Message, SimpleError> {
        todo!()
    }
}
