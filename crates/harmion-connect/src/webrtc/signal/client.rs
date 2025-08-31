use ed25519_dalek::SigningKey;

use futures_util::{SinkExt as _, StreamExt as _};
use tokio::{sync::mpsc, time::Duration};
use tokio_tungstenite::tungstenite::Message as WSMessage;
use tokio_util::sync::CancellationToken;

use std::collections::HashSet;

use thiserror::Error;
use tracing::{debug, error, info, warn};

use crate::{
    webrtc::{
        signal::{InitClientToSig, InitSigToClient, SignalInfo, SignalMessage},
        BUFFER_SIZE,
    },
    Message, MessageT, PeerIndex,
};

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
    #[error("invalid message signature")]
    Untrust,
    #[error("Not connected to a Signalling service")]
    NotConnected,
    #[error("fialed to send message: {0}")]
    Send(#[from] Box<mpsc::error::SendError<SignalMessage>>),
}

pub(super) struct SignalClient {
    sigs: HashSet<SignalInfo>,
    pub(super) connection: Option<(
        mpsc::Sender<SignalMessage>,
        mpsc::Receiver<MessageT<SignalMessage>>, // 外でverify
    )>,
    pub(super) connected_to: Option<SignalInfo>,

    id: PeerIndex,
    key: SigningKey,

    token: Option<CancellationToken>,
}

impl SignalClient {
    pub(super) fn new(id: PeerIndex, key: SigningKey) -> Self {
        Self {
            sigs: HashSet::new(),
            id,
            key,

            connection: None,
            connected_to: None,
            token: None,
        }
    }

    pub(super) fn insert_signal(&mut self, sig: SignalInfo) -> bool {
        self.sigs.insert(sig)
    }

    pub(super) async fn send(&self, message: SignalMessage) -> Result<(), SignalCError> {
        if let Some((tx, _)) = &self.connection {
            tx.send(message).await.map_err(Box::new)?;

            Ok(())
        } else {
            Err(SignalCError::NotConnected)
        }
    }

    pub(super) async fn recv(&mut self) -> Result<MessageT<SignalMessage>, SignalCError> {
        if let Some((_, rx)) = &mut self.connection {
            rx.recv().await.ok_or(SignalCError::NotConnected)
        } else {
            Err(SignalCError::NotConnected)
        }
    }

    pub(super) fn origin(&self) -> PeerIndex {
        self.id
    }

    pub(super) async fn connect(&mut self, to: Option<SignalInfo>) -> Result<(), SignalCError> {
        let info = if let Some(info) = to {
            if !self.sigs.contains(&info) {
                self.sigs.insert(info.clone());
            }

            Some(info)
        } else {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()?;

            // stream::iter で並列にリクエストを投げる
            let mut futures = futures_util::stream::iter(
                self.sigs
                    .iter()
                    .map(|info| (format!("{}/heartbeat", info.to_http()), info))
                    .map(|(url, info)| {
                        let client = client.clone();
                        async move {
                            let res = client.get(url).send().await;
                            res.ok()
                                .filter(|resp| resp.status().is_success())
                                .map(|_| info)
                        }
                    }),
            )
            .buffer_unordered(10);

            let mut info: Option<SignalInfo> = None;
            while let Some(resp) = futures.next().await {
                if let Some(r) = resp {
                    info = Some(r.clone());
                    break;
                }
            }

            info
        };

        if let Some(info) = info {
            let (stream, _) =
                tokio_tungstenite::connect_async(format!("{}/client", info.to_ws())).await?;

            let (mut ws_tx, mut ws_rx) = stream.split();

            match Message::from(
                &InitClientToSig {
                    known_signals: self.sigs.iter().cloned().collect(),
                },
                &self.key,
            )
            .and_then(|c| rmp_serde::to_vec(&c))
            .map(|c| WSMessage::Binary(c.into()))
            {
                Ok(ws_message) => {
                    ws_tx.send(ws_message).await?;
                }
                Err(e) => Err(SignalCError::Convertfailed(e))?,
            };

            match ws_rx.next().await {
                Some(Ok(msg)) => {
                    if msg.is_close() {
                        error!("received close message while waiting for the handshake message");
                        return Err(SignalCError::NoHandshake)?;
                    }

                    let data = msg.into_data();

                    let message: MessageT<InitSigToClient> = {
                        rmp_serde::from_slice(&data)
                            .and_then(|msg: Message| MessageT::try_from(msg))
                    }
                    .map_err(SignalCError::InvailedHandshake)?;

                    if !message.verify_result || message.origin != info.id {
                        Err(SignalCError::Untrust)?;
                    }

                    self.sigs.extend(message.content.known_signals);
                }
                _ => {
                    return Err(SignalCError::NoHandshake);
                }
            }

            let token = CancellationToken::new();

            let (sender, mut rx): (_, mpsc::Receiver<SignalMessage>) = mpsc::channel(BUFFER_SIZE);

            {
                let token = token.clone();
                let key = self.key.clone();

                tokio::task::spawn(async move {
                    loop {
                        tokio::select! {
                            message = rx.recv() => {
                                let Some(message) = message else { break };

                                match Message::from(&message, &key)
                                    .and_then(|c| rmp_serde::to_vec(&c))
                                    .map(|c| WSMessage::Binary(c.into()))
                                {
                                    Ok(ws_message) => if let Err(e) = ws_tx.send(ws_message).await {
                                         error!("websocket send error: {e}");
                                         break;
                                    },
                                    Err(e) => error!("failed to convert SignalMessage to MessagePack: {e}"),
                                }
                            }
                            _ = token.cancelled() => {
                                break;
                            }
                        }
                    }

                    info!("sending close");
                    if let Err(e) = ws_tx.send(WSMessage::Close(None)).await {
                        warn!("websocket send error: {e}");
                    }

                    token.cancel();
                });
            }

            let (tx, receiver) = mpsc::channel(BUFFER_SIZE);

            {
                let token = token.clone();

                tokio::task::spawn(async move {
                    loop {
                        tokio::select! {
                            message = ws_rx.next() => {
                                let Some(Ok(msg)) = message else {
                                    if let Some(Err(e)) = message {
                                        error!("failed to receive message: {e}");
                                    }
                                    break
                                };

                                debug!("received signal to signal message: {msg:?}");

                                if msg.is_close() {
                                    info!("received close message");
                                    break;
                                }


                                let data = msg.into_data();

                                let message: Result<MessageT<SignalMessage>, rmp_serde::decode::Error> = {
                                    rmp_serde::from_slice(&data).and_then(|msg: Message| MessageT::try_from(msg))
                                };

                                if let Err(e) = &message {
                                    error!("failed to convert to SignalMessage: {e}",);
                                    continue;
                                }

                                if let Err(e) = tx.send(message.unwrap()).await {
                                    error!("failed to send receiver channel, channel was closed?: {e}");
                                    break;
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

            self.connection = Some((sender, receiver));
            self.connected_to = Some(info.clone());
            self.token = Some(token);

            Ok(())
        } else {
            Err(SignalCError::NoSignalAvailable)
        }
    }

    pub(super) fn disconnect(&mut self) {
        if let Some(token) = self.token.take() {
            token.cancel();
        }

        self.connection = None;
        self.connected_to = None;
    }

    #[cfg(test)]
    pub(super) fn signal_count(&self) -> usize {
        self.sigs.len()
    }
}
