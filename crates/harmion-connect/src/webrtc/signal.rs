use ed25519_dalek::SigningKey;

use dashmap::DashMap;
use futures_util::{Sink, SinkExt as _, Stream, StreamExt as _};
use tokio::{
    sync::{broadcast, mpsc, RwLock},
    time::{Duration, Instant},
};
use tokio_tungstenite::tungstenite::Message as WSMessage;
use tokio_util::sync::CancellationToken;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tracing::error;
use warp::{reply::Reply, Filter};

use thiserror::Error;

#[derive(Debug, Error)]
pub(super) enum SignalError {
    #[error("the Signalling Server is already running")]
    ServerRunning,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to connect via ws: {0}")]
    Ws(#[from] tokio_tungstenite::tungstenite::Error),
}

type SigResult<T> = Result<T, SignalError>;

// Alias for the complex sig-peers mapping type
type SigPeers = Arc<RwLock<(HashMap<SignalInfo, HashSet<PeerIndex>>, HashMap<PeerIndex, SignalInfo>)>>;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(super) struct SignalInfo {
    addr: (String, u16),
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

#[derive(Debug)]
pub(super) struct Signal {
    port: u16,
    info: Option<Arc<ServerInfo>>,
    id: PeerIndex,
    key: SigningKey,
}

#[derive(Debug)]
struct ServerInfo {
    shutdown_token: CancellationToken,
    signal_info: SignalInfo,
    sigs_peers: SigPeers,
    received_connection_state_changes: mpsc::Sender<MessageT<PeerSignalStateChange>>,
    connection_state_changes_rx: broadcast::Receiver<Arc<PeerSignalStateChange>>,
    connection_state_changes_tx: broadcast::Sender<Arc<PeerSignalStateChange>>,
    signal_conns: DashMap<SignalInfo, (CancellationToken, mpsc::Sender<SignalMessage>)>,
    peer_conns: DashMap<PeerIndex, (CancellationToken, mpsc::Sender<SignalMessage>)>,
    new_sig_tx: mpsc::Sender<SignalInfo>,
}

use serde::{Deserialize, Serialize};

// ... (rest of the code remains unchanged through the implementation of Signal, route, heartbeat, client, init_client, signal, WSMessageT, etc.)

async fn init_signal<
    T: Sized + Stream<Item = Result<M, E>> + Sink<M, Error = E>,
    M: WSMessageT,
    E: std::error::Error,
>(
    _key: SigningKey,
    info: Arc<ServerInfo>,
    conn: T,
) -> Result<(), Box<dyn std::error::Error>> {
    let (_ws_tx, _ws_rx) = conn.split();
    let _our_connection_state_changes_rx = info.connection_state_changes_tx.subscribe();
    let _received_connection_state_changes_tx = info.received_connection_state_changes.clone();

    // TODO: handshake, register, loop exchanging PeerSignalStateChange and SignalMessage
    Ok(())
}

use crate::{Message, MessageT, PeerIndex};

// ... (SignalClient implementation unchanged)

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn heartbeat() -> Result<(), Box<dyn std::error::Error>> {
        let mut rng = ed25519_dalek::ed25519::signature::rand_core::OsRng;
        let key = ed25519_dalek::SigningKey::generate(&mut rng);
        let mut sig = super::Signal::new(0, (&key).into(), key);
        println!("starting the server");
        sig.run("test").await?;
        let port = sig.info().unwrap().addr.1;
        let url = format!("http://0.0.0.0:{port}/heartbeat");
        let client = reqwest::Client::new();
        println!("first request");
        let response = client.get(&url).send().await?;
        let body = response.text().await?;
        dbg!(body);
        println!("shutting down the server");
        sig.shutdown();
        println!("second request");
        let response = client.get(&url).send().await;
        assert!(response.is_err());
        Ok(())
    }

    fn gen_id() -> PeerIndex {
        let mut rng = ed25519_dalek::ed25519::signature::rand_core::OsRng;
        (&ed25519_dalek::SigningKey::generate(&mut rng)).into()
    }

    fn gen_sig() -> SignalInfo {
        SignalInfo {
            addr: ("A".into(), 65535),
            id: gen_id(),
        }
    }
}

#[cfg(test)]
mod tests_signal_peers {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;
    use tokio::sync::RwLock;

    async fn empty_sig_peers() -> SigPeers {
        Arc::new(RwLock::new((HashMap::new(), HashMap::new())))
    }

    async fn snapshot(
        sig_peers: &SigPeers,
    ) -> (HashMap<SignalInfo, HashSet<PeerIndex>>, HashMap<PeerIndex, SignalInfo>) {
        let guard = sig_peers.read().await;
        (guard.0.clone(), guard.1.clone())
    }

    fn make_two_signals() -> (SignalInfo, SignalInfo) {
        let s1 = SignalInfo {
            addr: ("127.0.0.1".into(), 9000),
            id: PeerIndex::from(1u64),
        };
        let s2 = SignalInfo {
            addr: ("127.0.0.1".into(), 9001),
            id: PeerIndex::from(2u64),
        };
        (s1, s2)
    }

    async fn insert_mapping(
        sig_peers: &SigPeers,
        peer: PeerIndex,
        sig: SignalInfo,
    ) {
        let mut guard = sig_peers.write().await;
        let (sigs, peers) = &mut *guard;
        peers.insert(peer, sig.clone());
        sigs.entry(sig).or_insert_with(HashSet::new).insert(peer);
    }

    #[tokio::test]
    async fn connect_adds_new_peer_and_signal_set() {
        let sig_peers = empty_sig_peers().await;
        let (sig_a, _) = make_two_signals();
        let p1: PeerIndex = 1.into();

        handle_peer_signal_state_changes(
            PeerSignalStateChange::Connect(p1, sig_a.clone()),
            &sig_peers,
        )
        .await;

        let (sigs, peers) = snapshot(&sig_peers).await;
        assert_eq!(peers.get(&p1), Some(&sig_a));
        let set = sigs.get(&sig_a).expect("signal set should exist");
        assert!(set.contains(&p1));
    }

    // ... (other state-change tests unchanged)

    #[tokio::test]
    async fn heartbeat_formatting_zero_seconds_when_on_in_future() {
        let on = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let filter = heartbeat(on.into());
        let (body,) = warp::test::request().method("GET").filter(&filter).await.expect("filter failed");
        assert_eq!(body, "0s");
    }

    #[tokio::test]
    async fn heartbeat_includes_hours_minutes_seconds_components() {
        let on = std::time::Instant::now() - std::time::Duration::from_secs(3600 + 60 + 1);
        let filter = heartbeat(on.into());
        let (body,) = warp::test::request().method("GET").filter(&filter).await.expect("filter failed");
        assert!(body.contains("1h"), "expected hours component, got: {body}");
        assert!(body.contains("1m"), "expected minutes component, got: {body}");
        assert!(body.contains("1s"), "expected seconds component, got: {body}");
    }
}