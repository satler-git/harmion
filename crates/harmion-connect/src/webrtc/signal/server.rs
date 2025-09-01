use super::{InitClientToSig, InitSigToClient, SignalInfo, SignalMessage};

use crate::webrtc::signal::client::SignalCError;

use ed25519_dalek::SigningKey;

use dashmap::DashMap;
use futures_util::{Sink, SinkExt as _, Stream, StreamExt as _};
use tokio::sync::{broadcast, mpsc, RwLock};
use tokio_util::sync::CancellationToken;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tracing::{debug, error, info, warn};

use thiserror::Error;

use crate::{webrtc::BUFFER_SIZE, Message, MessageT, PeerIndex};

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

#[derive(Debug)]
pub(in crate::webrtc) struct Signal {
    port: u16, // sig.port /= info.port の場合あり
    info: Option<Arc<ServerInfo>>,
    id: PeerIndex,

    key: SigningKey,
}

type SigPeers = Arc<
    RwLock<(
        HashMap<SignalInfo, HashSet<PeerIndex>>,
        HashMap<PeerIndex, SignalInfo>,
    )>,
>;

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
    received_connection_state_changes: mpsc::Sender<Arc<PeerSignalStateChange>>,
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
    Signal(Box<SignalMessage>),
    State(Arc<PeerSignalStateChange>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum PeerSignalStateChange {
    Connect(PeerIndex, SignalInfo),
    DisConnect(PeerIndex, SignalInfo),

    All(Vec<PeerIndex>, SignalInfo),
}

async fn handle_peer_signal_state_changes(msg: Arc<PeerSignalStateChange>, sig_peers: &SigPeers) {
    debug!("handling PeerSignalStateChange: {msg:?}");

    match Arc::<PeerSignalStateChange>::unwrap_or_clone(msg) {
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

// let mut lock = sig_peers.read().await;
// let (_, peers) = &mut *lock;

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

        let (tx, mut rx) = mpsc::channel::<Arc<PeerSignalStateChange>>(BUFFER_SIZE);

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

                            handle_peer_signal_state_changes(msg, &sigs_peers).await;
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
            broadcast::channel(BUFFER_SIZE);

        let (new_sig_tx, mut new_sig_rx) = mpsc::channel(BUFFER_SIZE);

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

        let srv = warp::serve(filters::route(self.key.clone(), server_info))
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

        init_signal(key, info, stream, Some(sig)).await.unwrap(); // TODO:

        Ok(())
    }

    pub(super) fn shutdown(&mut self) {
        if let Some(c) = self.info.take() {
            c.shutdown_token.cancel()
        }
        self.info = None;
    }

    pub(super) fn info(&self) -> Option<SignalInfo> {
        self.info.as_ref().map(|c| c.signal_info.clone())
    }
}

mod filters {
    use ed25519_dalek::SigningKey;

    use std::sync::Arc;
    use tokio::time::{Duration, Instant};
    use tracing::error;

    use crate::webrtc::signal::server::{init_client, init_signal, ServerInfo};

    use warp::Filter;

    pub(super) fn route(
        key: SigningKey,
        info: Arc<ServerInfo>,
    ) -> impl Filter<Extract = (impl warp::Reply,), Error = warp::Rejection> + Clone {
        let heartbeat = warp::path("heartbeat").and(heartbeat(Instant::now()));

        let info = warp::any().map(move || (key.clone(), info.clone()));

        let client = warp::path("client")
            .and(info.clone())
            .and(warp::ws())
            .map(client);

        let signal = warp::path("signal").and(info).and(warp::ws()).map(signal);

        heartbeat.or(client).or(signal).with(warp::trace::request())
    }

    fn heartbeat(on: Instant) -> impl Filter<Extract = (String,), Error = warp::Rejection> + Clone {
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

    fn client((key, info): (SigningKey, Arc<ServerInfo>), ws: warp::ws::Ws) -> impl warp::Reply {
        ws.on_upgrade(|ws| async move {
            if let Err(e) = init_client(key, info, ws).await {
                error!("init websocket connection to client: {e}");
            }
        })
    }

    fn signal((key, info): (SigningKey, Arc<ServerInfo>), ws: warp::ws::Ws) -> impl warp::Reply {
        ws.on_upgrade(|ws| async move {
            if let Err(e) = init_signal(key, info, ws, None).await {
                error!("init websocket connection to signal: {e}");
            }
        })
    }
}

async fn init_client(
    key: SigningKey,
    info: Arc<ServerInfo>,
    ws: warp::ws::WebSocket,
) -> Result<(), Box<dyn std::error::Error>> {
    // TODO: もうちょっとerror
    let (mut ws_tx, mut ws_rx) = ws.split();

    info!("staring vs client handshake");

    let origin = {
        let ws_message = Message::from(
            &InitSigToClient {
                known_signals: info.sigs_peers.read().await.0.keys().cloned().collect(),
            },
            &key,
        )
        .and_then(|c| rmp_serde::to_vec(&c))
        .map(warp::ws::Message::binary)?;

        ws_tx.send(ws_message).await?;

        match ws_rx.next().await {
            Some(Ok(msg)) => {
                if msg.is_close() {
                    error!("received close message while waiting for the handshake message");
                    return Err(SignalCError::NoHandshake)?;
                }

                let data = msg.as_bytes();

                let message: MessageT<InitClientToSig> = {
                    rmp_serde::from_slice(data).and_then(|msg: Message| MessageT::try_from(msg))
                }
                .map_err(SignalCError::InvailedHandshake)?;

                if !message.verify_result {
                    Err(SignalCError::Untrust)?;
                }

                let (sig_to_peer, _) = &*info.sigs_peers.read().await;

                for si in message.content.known_signals {
                    if si != info.signal_info && !sig_to_peer.contains_key(&si) {
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

    info!("connected to {origin:?} (client)");

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

                        debug!("received signal to signal message: {msg:?}");

                        if msg.is_close() {
                            info!("received close message");
                            break;
                        }


                        let data = msg.as_bytes();

                        let message: MessageT<SignalMessage> = match rmp_serde::from_slice(data)
                            .and_then(|msg: Message| MessageT::try_from(msg)) {
                            Ok(msg) => msg,
                            Err(e) => { error!("client connection via ws: {e}"); continue }
                        };


                        if  message.content.origin != origin {
                            error!("the origin of message from the client({origin:?}) does not match the sender");
                            continue;
                        }

                        if message.origin != origin || !message.verify_result {
                            error!("failed to verify the message. signature or origin is incorrect.");
                            continue;
                        }

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
        });
    }

    let (send_to_c_tx, mut send_to_c_rx) = mpsc::channel::<SignalMessage>(BUFFER_SIZE);

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

                        if let Err(e) = ws_tx.send(ws_message).await{
                            error!("websocket send error: {e}");
                            break;
                        }
                    }
                    _ = token.cancelled() => {
                        break;
                    }
                }
            }

            info!("Sending close");
            if let Err(e) = ws_tx.send(warp::ws::Message::close()).await {
                warn!("websocket send error: {e}");
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
    all: PeerSignalStateChange,
}

#[derive(Debug, Serialize, Deserialize)]
struct InitSignalClientToSignal {
    known_signals: Vec<SignalInfo>,
    all: PeerSignalStateChange,
    me: SignalInfo,
}

async fn init_signal<
    T: Sized + Stream<Item = Result<M, E>> + Sink<M, Error = E> + Send + 'static,
    M: messages::WSMessageT + Send + std::fmt::Debug + 'static,
    E: std::error::Error + 'static,
>(
    key: SigningKey,
    info: Arc<ServerInfo>,
    conn: T,
    to: Option<SignalInfo>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (mut ws_tx, mut ws_rx) = conn.split();

    let is_client = to.is_some();

    let received_connection_state_changes_tx = info.received_connection_state_changes.clone();

    let origin = {
        info!("starting vs signal handshake.");

        let known_signals = info.sigs_peers.read().await.0.keys().cloned().collect();
        let connected_nodes = info.peer_conns.iter().map(|e| *e.key()).collect();

        let handshake_message = if is_client {
            Ok(InitSignalClientToSignal {
                me: info.signal_info.clone(),
                all: PeerSignalStateChange::All(connected_nodes, info.signal_info.clone()),
                known_signals,
            })
            .and_then(|c| Message::from(&c, &key))
        } else {
            Ok(InitSignalToSignalClient {
                known_signals,
                all: PeerSignalStateChange::All(connected_nodes, info.signal_info.clone()),
            })
            .and_then(|c| Message::from(&c, &key))
        }
        .and_then(|c| rmp_serde::to_vec(&c))
        .map(M::binary)?;

        ws_tx.send(handshake_message).await?;

        info!("sent handshake message to the other Signalling service");

        match ws_rx.next().await {
            Some(Ok(msg)) => {
                if msg.is_close() {
                    error!("received close message while waiting for the handshake message");
                    return Err(SignalCError::NoHandshake)?;
                }

                let data = msg.into_bytes_t();

                let (verify_result, known_signals, origin, all) = if is_client {
                    info!("parsing the received handshake message as Signal-Client");

                    let to = to.unwrap();

                    let message: MessageT<InitSignalToSignalClient> = rmp_serde::from_slice(&data)
                        .and_then(|msg: Message| MessageT::try_from(msg))?;

                    if message.origin != to.id {
                        Err(SignalCError::Untrust)?;
                    }

                    (
                        message.verify_result,
                        message.content.known_signals,
                        to,
                        message.content.all,
                    )
                } else {
                    let message: MessageT<InitSignalClientToSignal> = rmp_serde::from_slice(&data)
                        .and_then(|msg: Message| MessageT::try_from(msg))?;

                    info!("parsing the received handshake message as Signal-Server");

                    (
                        message.verify_result,
                        message.content.known_signals,
                        message.content.me,
                        message.content.all,
                    )
                };

                if !verify_result {
                    Err(SignalCError::Untrust)?;
                }

                let (sig_to_peer, _) = &*info.sigs_peers.read().await;

                for si in known_signals {
                    if si != info.signal_info && !sig_to_peer.contains_key(&si) {
                        if let Err(e) = info.new_sig_tx.send(si).await {
                            error!("failed to send new signal info, maybe channel has been closed: {e}")
                        }
                    }
                }

                let _ = received_connection_state_changes_tx.send(all.into()).await;

                origin
            }
            _ => {
                return Err(SignalCError::NoHandshake)?;
            }
        }
    };

    info!("connected to {origin:?} (signal)");

    let token = info.shutdown_token.child_token();

    let (sender, mut rx) = mpsc::channel(BUFFER_SIZE);

    {
        let mut our_connection_state_changes_rx = info.connection_state_changes_tx.subscribe();
        let token = token.clone();

        tokio::spawn(async move {
            loop {
                let message = tokio::select! {
                    msg = rx.recv() => {
                        let Some(msg) = msg else {
                            info!("the message from mpsc channel was None, maybe channel(registered in signal_conns) has been closed or dropped?");
                            break
                        };
                        SignalToSignal::Signal(Box::new(msg))
                    }
                    msg = our_connection_state_changes_rx.recv() => {
                        let Ok(msg) = msg else {
                            info!("the message from mpsc channel was None, maybe channel(our_connection_state_changes_rx) has been closed?");
                            break
                        };
                        SignalToSignal::State(msg)
                    }
                    _ = token.cancelled() => {
                        info!("vs signal sending thread has been cancelled");
                        break
                    }
                };

                match Ok(message)
                    .and_then(|c| Message::from(&c, &key))
                    .and_then(|c| rmp_serde::to_vec(&c))
                    .map(M::binary)
                {
                    Ok(msg) => {
                        if let Err(e) = ws_tx.send(msg).await {
                            error!("websocket send error: {e}");
                            break;
                        }
                    }
                    Err(e) => {
                        error!("failed to serialize message: {e}");
                        continue;
                    }
                }
            }

            token.cancel();

            info!("sending close");
            if let Err(e) = ws_tx.send(M::close()).await {
                // WebSocket protocol error: Sending after closing is not allowed
                // は許容
                warn!("websocket send error: {e}"); // TODO
            }
        });
    }

    {
        let token = token.clone();
        let info = info.clone();
        let origin = origin.clone();

        tokio::spawn(async move {
            loop {
                let message = tokio::select! {
                    msg = ws_rx.next() => {
                        let Some(Ok(msg)) = msg else {
                            info!("message from ws_rx(vs signal) is not vailed. Closing");
                            debug!("message from ws_rx(vs signal): {msg:?}");

                            break
                        };

                        debug!("received signal to signal message: {msg:?}");

                        if msg.is_close() {
                            info!("received close message");
                            break;
                        }

                        let data = msg.into_bytes_t();

                        let message: MessageT<SignalToSignal> = match rmp_serde::from_slice(&data)
                            .and_then(|msg: Message| MessageT::try_from(msg)) {
                                Ok(msg) => msg,
                                Err(e) => { error!("failed to decode message: {e}"); continue }
                            };

                        message
                    }
                    _ = token.cancelled() => {
                        info!("vs signal receiving thread has been cancelled");
                        break
                    }
                };

                if message.origin != origin.id || !message.verify_result {
                    error!("failed to verify the message. signature or origin is incorrect.");
                    continue;
                }

                match message.content {
                    SignalToSignal::Signal(sig) => {
                        let to = sig.to;

                        if let Some(conn) = info.peer_conns.get(&to) {
                            info!(
                                "message to {to:?} from {:?} has been transferred from another Signalling service, {:?}",
                                sig.origin, message.origin
                            );

                            let _ = conn.1.send(*sig).await;
                        } else {
                            error!("we are not connected to the peer({to:?}) via WebSocket, but a SignalMessage has been transferred from another Signalling service. Maybe the syncing state has broken?");
                        }
                    }
                    SignalToSignal::State(state) => {
                        let _ = received_connection_state_changes_tx.send(state).await;
                    }
                }
            }

            token.cancel();

            info!("Closing Channel(vs signal). Removing the connection from signal_conns");
            info.signal_conns.remove(&origin);
        });
    }

    info.signal_conns.insert(origin, (token, sender));

    Ok(())
}

mod messages {
    pub(super) trait WSMessageT {
        fn close() -> Self;
        fn is_close(&self) -> bool;

        fn binary<T: Into<tokio_tungstenite::tungstenite::Bytes>>(value: T) -> Self;
        fn into_bytes_t(self) -> Vec<u8>;
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

        fn into_bytes_t(self) -> Vec<u8> {
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

        fn into_bytes_t(self) -> Vec<u8> {
            self.into_data().into()
        }
    }
}

#[cfg(test)]
mod tests {
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
}
