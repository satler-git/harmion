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
use warp::reply::Reply;

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

// Type alias to shorten complex sigs_peers types
type SigPeers = Arc<RwLock<(HashMap<SignalInfo, HashSet<PeerIndex>>, HashMap<PeerIndex, SignalInfo>)>>;

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

#[derive(Debug)]
pub(super) struct Signal {
    port: u16, // sig.port /= info.port の場合あり
    info: Option<Arc<ServerInfo>>,
    id: PeerIndex,

    key: SigningKey,
}

#[derive(Debug)]
struct ServerInfo {
    shutdown_token: CancellationToken,
    signal_info: SignalInfo,

    sigs_peers: SigPeers,

    // ((sdp | ice) | state change) のconnectionから受けたstate changeの方を流す。
    // 他signalとの通信を管理するスレッドにcloneして渡す
    // (sdp | ice) の場合、peer_connsに流す
    //
    // これの対のReceiverの情報でsigs/peersを更新する
    received_connection_state_changes: mpsc::Sender<MessageT<PeerSignalStateChange>>,
    // state change
    // 同様、他Signalにここから受けとったのを渡す。Senderはws/clientのとき
    connection_state_changes_rx: broadcast::Receiver<Arc<PeerSignalStateChange>>,
    // Clientのところでcloneする
    connection_state_changes_tx: broadcast::Sender<Arc<PeerSignalStateChange>>,

    signal_conns: DashMap<SignalInfo, (CancellationToken, mpsc::Sender<SignalMessage>)>, // Messageにくるんでtoをpeer_connsから参照しておくりつける
    peer_conns: DashMap<PeerIndex, (CancellationToken, mpsc::Sender<SignalMessage>)>, // 自分のときに

    new_sig_tx: mpsc::Sender<SignalInfo>,
}

use serde::{Deserialize, Serialize};

// 内部的にはsignal <-> signalの通信はこの型を経由する
// client <-> signalは直接SignalMessageのやりとり
#[derive(Debug, Serialize, Deserialize)]
enum SignalToSignal {
    Signal(SignalMessage),
    State(PeerSignalStateChange),
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct SignalMessage {
    origin: PeerIndex,
    to: PeerIndex,

    data: SignalData,
}

#[derive(Debug, Serialize, Deserialize)]
enum PeerSignalStateChange {
    Connect(PeerIndex, SignalInfo),
    DisConnect(PeerIndex, SignalInfo),

    All(Vec<PeerIndex>, SignalInfo),
}

#[derive(Debug, Serialize, Deserialize)]
enum SignalData {
    Sdp(Box<webrtc::peer_connection::sdp::session_description::RTCSessionDescription>),
    Ice(webrtc::ice_transport::ice_candidate::RTCIceCandidateInit),
}

async fn handle_peer_signal_state_changes(
    msg: PeerSignalStateChange,
    sig_peers: &SigPeers,
) {
    match msg {
        PeerSignalStateChange::Connect(peer, sig) => {
            let mut lock = sig_peers.write().await;
            let (sigs, peers) = &mut *lock;

            if let Some(old_sig) = peers.insert(peer, sig.clone()) {
                let mut last = false;

                sigs.entry(old_sig.clone()).and_modify(|set| {
                    set.remove(&peer);

                    last = set.is_empty();
                });

                if last {
                    sigs.remove(&old_sig);
                }
            }

            sigs.entry(sig).or_insert_with(HashSet::new).insert(peer);
        }

        PeerSignalStateChange::DisConnect(peer, sig) => {
            let mut lock = sig_peers.write().await;
            let (sigs, peers) = &mut *lock;

            if let Some(existing) = peers.get(&peer) {
                if *existing == sig {
                    peers.remove(&peer);
                }
            }

            let mut last = false;

            sigs.entry(sig.clone()).and_modify(|set| {
                set.remove(&peer);

                last = set.is_empty();
            });

            if last {
                sigs.remove(&sig);
            }
        }

        PeerSignalStateChange::All(new_peers, sig) => {
            let mut lock = sig_peers.write().await;
            let (sigs, peers) = &mut *lock;

            let old = sigs.entry(sig.clone()).or_insert_with(HashSet::new).clone();

            let new_peers: HashSet<_, std::hash::RandomState> = HashSet::from_iter(new_peers);

            // old ∧ ￢new
            // disconnect
            for peer in old.iter() {
                if !new_peers.contains(peer) {
                    if let Some(existing) = peers.get(peer) {
                        if *existing == sig {
                            peers.remove(peer);
                        }
                    }
                }
            }

            // ￢old ∧ new
            // connect
            for peer in new_peers.iter() {
                if !old.contains(peer) {
                    if let Some(old_sig) = peers.insert(*peer, sig.clone()) {
                        let mut last = false;

                        sigs.entry(old_sig.clone()).and_modify(|set| {
                            set.remove(peer);

                            last = set.is_empty();
                        });

                        if last {
                            sigs.remove(&old_sig);
                        }
                    }
                }
            }

            sigs.insert(sig, new_peers);
        }
    }
}

impl Signal {
    pub(super) fn new(port: u16, id: PeerIndex, key: SigningKey) -> Self {
        Self {
            port,
            info: None,
            id,
            key,
        }
    }

    pub(super) async fn run(&mut self, host: &str) -> SigResult<()> {
        if self.info.is_some() {
            return Err(SignalError::ServerRunning);
        }

        let listener =
            tokio::net::TcpListener::bind::<std::net::SocketAddr>(([0, 0, 0, 0], self.port).into())
                .await?;

        let port = listener.local_addr()?.port();

        let token = CancellationToken::new();

        let (tx, mut rx) = mpsc::channel::<MessageT<PeerSignalStateChange>>(super::BUFFER_SIZE);

        let sigs_peers = {
            let sig_to_peer = HashMap::new();
            let peer_to_sig = HashMap::new();

            Arc::new(RwLock::new((sig_to_peer, peer_to_sig)))
        };

        {
            let token = token.clone();

            let sigs_peers = sigs_peers.clone();

            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        msg = rx.recv() => {
                            let Some(msg) = msg else { break };

                            handle_peer_signal_state_changes(msg.content, &sigs_peers).await;
                        }
                        _ = token.cancelled() => {
                            break;
                        }
                    }
                }

                token.cancel();
            });
        }

        // これはclient/serverのhandle threadでどうこうするから今はいい
        let (connection_state_changes_tx, connection_state_changes_rx) =
            broadcast::channel(super::BUFFER_SIZE);

        let (new_sig_tx, mut new_sig_rx) = mpsc::channel(super::BUFFER_SIZE);

        let server_info = Arc::new(ServerInfo {
            shutdown_token: token.clone(),
            signal_info: SignalInfo {
                addr: (host.into(), port),
                id: self.id,
            },

            received_connection_state_changes: tx,

            sigs_peers,

            signal_conns: DashMap::new(),
            peer_conns: DashMap::new(),

            connection_state_changes_rx,
            connection_state_changes_tx,

            new_sig_tx,
        });

        {
            let token = token.clone();

            let server_info = server_info.clone();
            let key = self.key.clone();

            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        sig = new_sig_rx.recv() => {
                            let Some(sig) = sig else { break };

                            if let Err(e) = Self::add_sig(server_info.clone(), key.clone(), sig).await {
                                error!("{e}");
                            }
                        }
                        _ = token.cancelled() => {
                            break;
                        }
                    }
                }

                token.cancel();
            });
        }

        self.info = Some(server_info.clone());

        let srv = warp::serve(route(self.key.clone(), server_info))
            .incoming(listener)
            .graceful(async move {
                token.cancelled().await;
            });

        tokio::spawn(srv.run());

        Ok(())
    }

    async fn add_sig(info: Arc<ServerInfo>, key: SigningKey, sig: SignalInfo) -> SigResult<()> {
        let (stream, _) =
            tokio_tungstenite::connect_async(format!("{}/signal", sig.to_ws())).await?;

        // prefix key with underscore to silence unused warning
        init_signal(_key, info, stream).await.unwrap(); // TODO:

        Ok(())
    }

    pub(super) fn shutdown(&mut self) {
        if let Some(c) = self.info.take() {
            c.shutdown_token.cancel()
        }
    }

    pub(super) fn info(&self) -> Option<SignalInfo> {
        self.info.as_ref().map(|c| c.signal_info.clone())
    }
}

fn route(
    key: SigningKey,
    info: Arc<ServerInfo>,
) -> impl warp::Filter<Extract = (impl warp::Reply,), Error = warp::Rejection> + Clone {
    let heartbeat = warp::path("heartbeat").and(heartbeat(Instant::now()));
    let info = warp::any().map(move || (key.clone(), info.clone()));

    let client = warp::path("client")
        .and(info.clone())
        .and(warp::ws())
        .map(client);

    let signal = warp::path("signal").and(info).and(warp::ws()).map(signal);

    heartbeat.or(client).or(signal).with(warp::trace::request())
}

fn heartbeat(on: Instant) -> impl warp::Filter<Extract = (String,), Error = warp::Rejection> + Clone {
    fn format_duration_human(dur: Duration) -> String {
        let mut r = String::new();
        let total_secs = dur.as_secs();

        let hours = total_secs / 3600;
        if hours > 0 {
            r += &format!("{hours}h");
        }

        let minutes = (total_secs % 3600) / 60;
        if minutes > 0 {
            r += &format!("{minutes}m");
        }

        let seconds = total_secs % 60;
        if seconds > 0 {
            r += &format!("{seconds}s");
        }

        let millis = dur.subsec_millis();
        if millis > 0 {
            r += &format!("{millis}ms");
        }

        if r.is_empty() {
            "0s".to_string()
        } else {
            r
        }
    }

    warp::get().map(move || format_duration_human(Instant::now().saturating_duration_since(on)))
}

// handshake
// -> signal
// signals
// -> client
//
// verify, -> もしSignalInfoと違ったらerr

#[derive(Debug, Serialize, Deserialize)]
struct InitClientToSig {
    known_signals: Vec<SignalInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
struct InitSigToClient {
    known_signals: Vec<SignalInfo>,
}

fn client((key, info): (SigningKey, Arc<ServerInfo>), ws: warp::ws::Ws) -> impl Reply {
    ws.on_upgrade(|ws| async move {
        if let Err(e) = init_client(key, info, ws).await {
            error!("init websocket connection to client: {e}");
        }
    })
}

async fn init_client(
    key: SigningKey,
    info: Arc<ServerInfo>,
    ws: warp::ws::WebSocket,
) -> Result<(), Box<dyn std::error::Error>> {
    let (mut ws_tx, mut ws_rx) = ws.split();

    let origin = {
        let (sig_to_peer, _) = &*info.sigs_peers.read().await;

        let ws_message = Message::from(
            &InitSigToClient {
                known_signals: sig_to_peer.keys().cloned().collect(),
            },
            &key,
        )
        .and_then(|c| rmp_serde::to_vec(&c))
        .map(warp::ws::Message::binary)?;
        ws_tx.send(ws_message).await?;

        match ws_rx.next().await {
            Some(Ok(msg)) => {
                let data = msg.as_bytes();

                let message: MessageT<InitClientToSig> = {
                    rmp_serde::from_slice(data).and_then(|msg: Message| MessageT::try_from(msg))
                }
                .map_err(SignalCError::InvailedHandshake)?;

                if !message.verify_result {
                    Err(SignalCError::Untrust)?;
                }

                for si in message.content.known_signals {
                    if !sig_to_peer.contains_key(&si) {
                        if let Err(e) = info.new_sig_tx.send(si).await {
                            error!("failed to send new signal info, maybe channel has been closed: {e}")
                        }
                    }
                }

                message.origin
            }
            _ => {
                return Err(SignalCError::NoHandshake)?;
            }
        }
    };

    let token = info.shutdown_token.child_token();
    let connection_state_changes_tx = info.connection_state_changes_tx.clone();

    let _ = connection_state_changes_tx.send(Arc::new(PeerSignalStateChange::Connect(
        origin,
        info.signal_info.clone(),
    )));

    {
        let info = info.clone();
        let token = token.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    msg = ws_rx.next() => {
                        let Some(Ok(msg)) = msg else { break };

                        let data = msg.as_bytes();

                        let message: MessageT<SignalMessage> = match rmp_serde::from_slice(data)
                            .and_then(|msg: Message| MessageT::try_from(msg)) {
                            Ok(msg) => msg,
                            Err(e) => { error!("client connection via ws: {e}"); continue }
                        };

                        let to = message.content.to;

                        if let Some(conn) = info.peer_conns.get(&to) {
                            let _ = conn.1.send(message.content).await;
                        } else if let (_, p2s) = &*info.sigs_peers.read().await
                            && let Some(signal_info) = p2s.get(&to)
                            && let Some(conn) = info.signal_conns.get(signal_info)
                        {
                            let _ = conn.1.send(message.content).await;
                        }
                    }
                    _ = token.cancelled() => {
                        break;
                    }
                }
            }

            info.peer_conns.remove(&origin);

            token.cancel();

            let _ = connection_state_changes_tx.send(Arc::new(PeerSignalStateChange::DisConnect(
                origin,
                info.signal_info.clone(),
            )));

            info.peer_conns.remove(&origin);
        });
    }

    let (send_to_c_tx, mut send_to_c_rx) = mpsc::channel::<SignalMessage>(super::BUFFER_SIZE);

    {
        let token = token.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    msg = send_to_c_rx.recv() => {
                        let Some(msg) = msg else { break };

                        let ws_message = match Message::from(
                            &msg,
                            &key,
                        )
                        .and_then(|c| rmp_serde::to_vec(&c))
                        .map(warp::ws::Message::binary) {
                            Ok(msg) => msg,
                            Err(e) => { error!("client connection via ws: {e}"); continue }
                        };

                        let _ = ws_tx.send(ws_message).await;
                    }
                    _ = token.cancelled() => {
                        if let Err(e) = ws_tx.send(warp::ws::Message::close()).await {
                             error!("websocket send error: {e}");
                        }

                        break;
                    }
                }
            }

            token.cancel();
        });
    }

    info.peer_conns.insert(origin, (token, send_to_c_tx));

    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
struct InitSignalToSignalClient {
    known_signals: Vec<SignalInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
struct InitSignalClientToSignal {
    known_signals: Vec<SignalInfo>,
    me: SignalInfo,
}

fn signal((key, info): (SigningKey, Arc<ServerInfo>), ws: warp::ws::Ws) -> impl Reply {
    ws.on_upgrade(|ws| async move {
        if let Err(e) = init_signal(key, info, ws).await {
            error!("init websocket connection to signal: {e}");
        }
    })
}

trait WSMessageT {
    fn close() -> Self;
    fn is_close(&self) -> bool;

    fn binary<T: Into<tokio_tungstenite::tungstenite::Bytes>>(value: T) -> Self;
    fn as_bytes_(self) -> Vec<u8>;
}

impl WSMessageT for warp::ws::Message {
    fn close() -> Self {
        Self::close()
    }

    fn is_close(&self) -> bool {
        Self::is_close(self)
    }

    fn binary<T: Into<tokio_tungstenite::tungstenite::Bytes>>(value: T) -> Self {
        Self::binary(value)
    }

    fn as_bytes_(self) -> Vec<u8> {
        self.into_bytes().into()
    }
}

impl WSMessageT for tokio_tungstenite::tungstenite::protocol::Message {
    fn close() -> Self {
        Self::Close(None)
    }

    fn is_close(&self) -> bool {
        Self::is_close(self)
    }

    fn binary<T: Into<tokio_tungstenite::tungstenite::Bytes>>(value: T) -> Self {
        Self::binary(value.into())
    }

    fn as_bytes_(self) -> Vec<u8> {
        self.into_data().into()
    }
}

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

    // TODO:
    Ok(())
}

#[derive(Debug, Error)]
pub(super) enum SignalCError {
    #[error("failed to send http request: {0}")]
    Request(#[from] reqwest::Error),
    #[error("Signalling service is unavailable")]
    NoSignalAvailable,
    #[error("failed to connect via ws: {0}")]
    Ws(#[from] tokio_tungstenite::tungstenite::Error),
    #[error("The first message is not the handshake: {0}")]
    InvailedHandshake(#[source] rmp_serde::decode::Error),
    #[error("failed to convert SignalMessage to MessagePack: {0}")]
    Convertfailed(rmp_serde::encode::Error),
    #[error("No handshake or websocket error")]
    NoHandshake,
    #[error("The sign of message is invailed")]
    Untrust,
}

use crate::{Message, MessageT, PeerIndex};

pub(super) struct SignalClient {
    sigs: HashSet<SignalInfo>,
    pub(super) connection: Option<(
        mpsc::Sender<SignalMessage>,
        mpsc::Receiver<MessageT<SignalMessage>>,
    )>,
    pub(super) connected_to: Option<SignalInfo>,

    id: PeerIndex,
    key: SigningKey,

    token: Option<CancellationToken>,
}

impl SignalClient {
    pub(super) async fn connect(&mut self) -> Result<(), SignalCError> {
        ...
    }

    fn disconnect(&mut self) {
        if let Some(token) = self.token.take() {
            token.cancel();
        }

        self.connection = None;
        self.connected_to = None;
    }
}

#[cfg(test)]
mod tests {
    ...
}

#[cfg(test)]
mod more_tests {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use tokio::sync::RwLock;
    use tokio_util::sync::CancellationToken;
    use warp::Filter;

    #[tokio::test]
    async fn heartbeat_filter_formats_basic() {
        // 1s
        let s: String = warp::test::request()
            .method("GET")
            .filter(&heartbeat((Instant::now() - Duration::from_secs(1)).into()))
            .await
            .unwrap();
        assert!(s.starts_with("1s"));

        // 1m1s
        let s: String = warp::test::request()
            .method("GET")
            .filter(&heartbeat((Instant::now() - Duration::from_secs(61)).into()))
            .await
            .unwrap();
        assert!(s.starts_with("1m1s"));

        // 1h
        let s: String = warp::test::request()
            .method("GET")
            .filter(&heartbeat((Instant::now() - Duration::from_secs(3600)).into()))
            .await
            .unwrap();
        assert!(s.starts_with("1h"));

        // 1h1m1s
        let s: String = warp::test::request()
            .method("GET")
            .filter(&heartbeat((Instant::now() - Duration::from_secs(3661)).into()))
            .await
            .unwrap();
        assert!(s.starts_with("1h1m1s"));
    }

    ...
}