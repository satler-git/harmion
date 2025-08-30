pub(super) mod client;
pub(super) mod server;

use crate::PeerIndex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct InitClientToSig {
    known_signals: Vec<SignalInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
struct InitSigToClient {
    known_signals: Vec<SignalInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
enum SignalData {
    Sdp(Box<webrtc::peer_connection::sdp::session_description::RTCSessionDescription>),
    Ice(webrtc::ice_transport::ice_candidate::RTCIceCandidateInit),
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct SignalMessage {
    origin: PeerIndex,
    to: PeerIndex,

    data: SignalData,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(super) struct SignalInfo {
    addr: (String, u16), // ip or domain(w/tailscale?)
    id: PeerIndex,
}

impl SignalInfo {
    fn to_http(&self) -> String {
        format!("http://{}:{}", self.addr.0, self.addr.1)
    }

    fn to_ws(&self) -> String {
        format!("ws://{}:{}", self.addr.0, self.addr.1)
    }
}
