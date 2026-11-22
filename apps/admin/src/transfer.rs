use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use harmion_client::protocol::{MAX_RECORD_BYTES, PeerRegistration, unhex};
use hyper::StatusCode;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    sync::Mutex,
    time::{Duration, Instant},
};

pub const PEER_PREFIX: &str = "harmion-peer:v1:";

pub fn encode_peer(registration: &serde_json::Value) -> Result<String, StatusCode> {
    let bytes = serde_json::to_vec(registration).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(&bytes)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let compressed = encoder
        .finish()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(format!(
        "{PEER_PREFIX}z:{}",
        URL_SAFE_NO_PAD.encode(compressed)
    ))
}

pub fn decode_peer(input: &str) -> Result<PeerRegistration, StatusCode> {
    if input.len() > MAX_RECORD_BYTES {
        tracing::warn!(
            limit_bytes = MAX_RECORD_BYTES,
            input_bytes = input.len(),
            "transfer input too large"
        );
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    if let Some(encoded) = input.trim().strip_prefix(&format!("{PEER_PREFIX}z:")) {
        let compressed = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| StatusCode::BAD_REQUEST)?;
        let mut bytes = Vec::new();
        GzDecoder::new(compressed.as_slice())
            .take(MAX_RECORD_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| StatusCode::BAD_REQUEST)?;
        if bytes.len() > MAX_RECORD_BYTES {
            tracing::warn!(
                limit_bytes = MAX_RECORD_BYTES,
                expanded_bytes = bytes.len(),
                "transfer expansion too large"
            );
            return Err(StatusCode::PAYLOAD_TOO_LARGE);
        }
        return serde_json::from_slice(&bytes).map_err(|_| StatusCode::BAD_REQUEST);
    }
    decode(PEER_PREFIX, input)
}

pub fn decode<T: for<'a> Deserialize<'a>>(prefix: &str, input: &str) -> Result<T, StatusCode> {
    if input.len() > MAX_RECORD_BYTES {
        tracing::warn!(
            limit_bytes = MAX_RECORD_BYTES,
            input_bytes = input.len(),
            "transfer input too large"
        );
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    let encoded = input
        .trim()
        .strip_prefix(prefix)
        .ok_or(StatusCode::BAD_REQUEST)?;
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    serde_json::from_slice(&bytes).map_err(|_| StatusCode::BAD_REQUEST)
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerAction {
    Approve,
    Join,
}

pub struct PendingPeer {
    pub registration: PeerRegistration,
    pub action: PeerAction,
    expires: Instant,
}

#[derive(Default)]
pub struct Confirmations(Mutex<BTreeMap<[u8; 32], PendingPeer>>);

impl Confirmations {
    pub fn preview(
        &self,
        registration: PeerRegistration,
        action: PeerAction,
    ) -> Result<String, StatusCode> {
        let mut tickets = self
            .0
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        tickets.retain(|_, value| value.expires > Instant::now());
        if tickets.len() >= 32 {
            tracing::warn!(
                limit = 32,
                pending = tickets.len(),
                status = 429,
                "peer confirmation limit reached"
            );
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        let key = harmion_client::random_id().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if tickets.contains_key(&key) {
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
        tickets.insert(
            key,
            PendingPeer {
                registration,
                action,
                expires: Instant::now() + Duration::from_secs(120),
            },
        );
        Ok(harmion_client::protocol::hex(&key))
    }
    pub fn take(&self, input: &str) -> Result<PendingPeer, StatusCode> {
        let key = unhex(input).map_err(|_| {
            tracing::warn!(
                reason = "invalid_ticket",
                "peer confirmation ticket rejected"
            );
            StatusCode::BAD_REQUEST
        })?;
        self.0
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .remove(&key)
            .filter(|value| value.expires > Instant::now())
            .ok_or_else(|| {
                tracing::warn!(
                    reason = "unknown_or_expired_ticket",
                    "peer confirmation ticket rejected"
                );
                StatusCode::GONE
            })
    }
    pub fn summary(&self, input: &str) -> Option<String> {
        let key = unhex(input).ok()?;
        let tickets = self.0.lock().ok()?;
        let pending = tickets.get(&key)?;
        if pending.expires <= Instant::now() {
            return None;
        }
        let action = match pending.action {
            PeerAction::Approve => "承認",
            PeerAction::Join => "参加",
        };
        Some(format!(
            "Peerを{action}: Node ID {} / Cluster ID {}",
            pending.registration.node_id, pending.registration.cluster_id
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_expansion_beyond_the_transfer_limit() {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(&vec![b'x'; MAX_RECORD_BYTES + 1])
            .unwrap();
        let encoded = format!(
            "{PEER_PREFIX}z:{}",
            URL_SAFE_NO_PAD.encode(encoder.finish().unwrap())
        );
        assert_eq!(
            decode_peer(&encoded).unwrap_err(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
    }

    #[test]
    fn rejects_truncated_compressed_transfers() {
        assert_eq!(
            decode_peer(&format!("{PEER_PREFIX}z:AA")).unwrap_err(),
            StatusCode::BAD_REQUEST
        );
    }
}
