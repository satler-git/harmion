//! Rust Provider SDK: authenticated loopback WebSocket to one local node.
use crate::{Connection, Error, PinnedEndpoint, protocol, random_id};
use base64::Engine;
use bytes::Bytes;
use http_body_util::Full;
use hyper::{Request, header, upgrade::on};
use hyper_util::rt::TokioIo;
use protocol::interpreter::{
    Display, InvocationId, InvokeChunkParams, InvokeEndParams, InvokeParams, InvokeStream,
    ProviderFailure, ProviderId, RPC_ATTACH, RPC_CANCEL, RPC_INVOKE, RPC_INVOKE_CHUNK,
    RPC_INVOKE_END, RPC_METHOD_NOT_FOUND, RpcMessage, decode_provider_result, decode_rpc,
    rpc_error, rpc_request, rpc_result, validate_invoke_stream,
};
use protocol::websocket::{Frame, MAX_FRAME_PAYLOAD, Opcode, encode_frame, parse_frame};
use protocol::{ConnectionInfo, TLS_NAME};
use serde_json::{Value as Json, json};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::{sleep, timeout},
};
use tokio_rustls::TlsConnector;

const CONNECT_DEADLINE: Duration = Duration::from_secs(60);
const ATTACH_DEADLINE: Duration = Duration::from_secs(10);

/// One message delivered to a Provider implementation.
#[derive(Clone, Debug, PartialEq)]
pub enum ProviderEvent {
    Invocation {
        invocation: InvocationId,
        params: InvokeParams,
    },
    Cancel {
        invocation: InvocationId,
    },
    Closed,
}

/// An attached Provider connection. A Provider keeps this alive while it can
/// serve calls and shares it only behind whatever concurrency its work needs.
pub struct ProviderClient {
    io: TokioIo<hyper::upgrade::Upgraded>,
    buffer: Vec<u8>,
    mask: [u8; 4],
    info: ConnectionInfo,
    credential: [u8; 32],
    provider: ProviderId,
    _connection: Connection,
}
impl ProviderClient {
    /// Open the pinned loopback WebSocket and perform `interpreter.attach`.
    /// The credential must belong to the app that registered this Provider.
    pub async fn connect(
        info: ConnectionInfo,
        credential: [u8; 32],
        provider: ProviderId,
    ) -> Result<Self, Error> {
        let provider_id = provider;
        let pinned = PinnedEndpoint::new(&info)?;
        let tcp = TcpStream::connect(pinned.endpoint).await?;
        let tls = TlsConnector::from(pinned.tls.clone())
            .connect(
                TLS_NAME.try_into().map_err(|_| Error::InvalidDiscovery)?,
                tcp,
            )
            .await
            .map_err(|_| Error::Transport)?;
        if tls
            .get_ref()
            .1
            .peer_certificates()
            .and_then(|certificates| certificates.first())
            .map(|certificate| certificate.as_ref())
            != Some(pinned.certificate.as_slice())
        {
            return Err(Error::InvalidResponse);
        }
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(tls))
            .await
            .map_err(|_| Error::Transport)?;
        let connection = Connection(tokio::spawn(async move {
            let _ = connection.with_upgrades().await;
        }));
        let random = random_id()?;
        let websocket_key = base64::engine::general_purpose::STANDARD.encode(&random[..16]);
        let expected_accept =
            protocol::websocket::accept_key(&websocket_key).map_err(|_| Error::InvalidRequest)?;
        let mut authorization =
            header::HeaderValue::from_str(&format!("Bearer {}", protocol::hex(&credential)))
                .map_err(|_| Error::InvalidRequest)?;
        authorization.set_sensitive(true);
        let request = Request::builder()
            .method(hyper::Method::GET)
            .uri("/provider/v1/connect")
            .header(header::HOST, TLS_NAME)
            .header(header::AUTHORIZATION, authorization)
            .header(header::UPGRADE, "websocket")
            .header(header::CONNECTION, "Upgrade")
            .header(header::SEC_WEBSOCKET_VERSION, "13")
            .header(header::SEC_WEBSOCKET_KEY, &websocket_key)
            .body(Full::new(Bytes::new()))
            .map_err(|_| Error::InvalidRequest)?;
        let response = timeout(CONNECT_DEADLINE, sender.send_request(request))
            .await
            .map_err(|_| Error::Timeout)?
            .map_err(|_| Error::Transport)?;
        if response.status() != hyper::StatusCode::SWITCHING_PROTOCOLS {
            return Err(Error::Http(response.status().as_u16()));
        }
        if response
            .headers()
            .get(header::SEC_WEBSOCKET_ACCEPT)
            .and_then(|value| value.to_str().ok())
            != Some(expected_accept.as_str())
        {
            return Err(Error::InvalidResponse);
        }
        let upgraded = on(response).await.map_err(|_| Error::Transport)?;
        let mut provider = Self {
            io: TokioIo::new(upgraded),
            buffer: Vec::new(),
            mask: random[16..20].try_into().expect("four bytes"),
            info,
            credential,
            provider: provider_id,
            _connection: connection,
        };
        provider
            .send_json(&rpc_request(
                &json!("attach"),
                RPC_ATTACH,
                json!({"provider_id": provider_id}),
            ))
            .await?;
        let reply = timeout(ATTACH_DEADLINE, provider.read_json())
            .await
            .map_err(|_| Error::Timeout)??;
        if let Some(error) = reply.get("error") {
            let code = error.get("code").and_then(Json::as_i64).unwrap_or(0);
            return Err(Error::ProviderAttachRejected { code });
        }
        let result = reply.get("result").ok_or(Error::InvalidResponse)?;
        if result.get("ok").and_then(Json::as_bool) != Some(true) {
            return Err(Error::InvalidResponse);
        }
        let attached: ProviderId = serde_json::from_value(
            result
                .get("provider_id")
                .cloned()
                .ok_or(Error::InvalidResponse)?,
        )
        .map_err(|_| Error::InvalidResponse)?;
        if attached != provider_id {
            return Err(Error::InvalidResponse);
        }
        Ok(provider)
    }
    /// Wait for the next invocation or cancel notification. Returns Closed after
    /// a close frame. Unknown request methods are answered and skipped. A
    /// streamed invocation is assembled from its ordered chunk notifications and
    /// is returned only after the end notification.
    pub async fn next(&mut self) -> Result<ProviderEvent, Error> {
        loop {
            let Some(message) = self.read_rpc().await? else {
                return Ok(ProviderEvent::Closed);
            };
            match message {
                RpcMessage::Request { id, method, params } if method == RPC_INVOKE => {
                    let mut params: InvokeParams =
                        serde_json::from_value(params).map_err(|_| Error::InvalidResponse)?;
                    let invocation = decode_invocation(&id)?;
                    if params.invocation_id != invocation {
                        return Err(Error::InvalidResponse);
                    }
                    if let Some(stream) = params.stream.take()
                        && let Some(event) =
                            self.read_stream(invocation, stream, &mut params).await?
                    {
                        return Ok(event);
                    }
                    return Ok(ProviderEvent::Invocation { invocation, params });
                }
                RpcMessage::Request { id, .. } => {
                    self.send_json(&rpc_error(&id, RPC_METHOD_NOT_FOUND, "method not found"))
                        .await?;
                }
                RpcMessage::Notification { method, params } if method == RPC_CANCEL => {
                    let value = params
                        .get("invocation_id")
                        .cloned()
                        .ok_or(Error::InvalidResponse)?;
                    let invocation: InvocationId =
                        serde_json::from_value(value).map_err(|_| Error::InvalidResponse)?;
                    return Ok(ProviderEvent::Cancel { invocation });
                }
                RpcMessage::Notification { .. } | RpcMessage::Response { .. } => {}
            }
        }
    }

    /// Assemble a streamed invocation. `None` means the complete invocation is
    /// ready; `Some` is a Cancel or Closed event that ended the stream.
    async fn read_stream(
        &mut self,
        invocation: InvocationId,
        stream: InvokeStream,
        params: &mut InvokeParams,
    ) -> Result<Option<ProviderEvent>, Error> {
        validate_invoke_stream(&stream).map_err(|_| Error::InvalidResponse)?;
        let mut contents = Vec::new();
        let mut total_bytes = 0u64;
        let mut seq = 0u64;
        loop {
            let Some(message) = self.read_rpc().await? else {
                return Ok(Some(ProviderEvent::Closed));
            };
            match message {
                RpcMessage::Notification {
                    method,
                    params: value,
                } if method == RPC_INVOKE_CHUNK => {
                    let chunk: InvokeChunkParams =
                        serde_json::from_value(value).map_err(|_| Error::InvalidResponse)?;
                    if chunk.invocation_id != invocation || chunk.seq != seq {
                        return Err(Error::InvalidResponse);
                    }
                    for item in &chunk.contents {
                        total_bytes = total_bytes.saturating_add(
                            serde_json::to_vec(item)
                                .map_err(|_| Error::InvalidResponse)?
                                .len() as u64,
                        );
                    }
                    if total_bytes > stream.total_bytes
                        || contents.len() as u64 + chunk.contents.len() as u64
                            > stream.total_contents
                    {
                        return Err(Error::InvalidResponse);
                    }
                    contents.extend(chunk.contents);
                    seq = seq.saturating_add(1);
                }
                RpcMessage::Notification {
                    method,
                    params: value,
                } if method == RPC_INVOKE_END => {
                    let end: InvokeEndParams =
                        serde_json::from_value(value).map_err(|_| Error::InvalidResponse)?;
                    if end.invocation_id != invocation
                        || end.seq != seq
                        || end.seq != stream.chunks
                        || end.total_contents != contents.len() as u64
                        || end.total_bytes != total_bytes
                    {
                        return Err(Error::InvalidResponse);
                    }
                    params.contents = contents;
                    return Ok(None);
                }
                RpcMessage::Notification { method, params } if method == RPC_CANCEL => {
                    let value = params
                        .get("invocation_id")
                        .cloned()
                        .ok_or(Error::InvalidResponse)?;
                    let invocation: InvocationId =
                        serde_json::from_value(value).map_err(|_| Error::InvalidResponse)?;
                    return Ok(Some(ProviderEvent::Cancel { invocation }));
                }
                RpcMessage::Request { id, .. } => {
                    self.send_json(&rpc_error(&id, RPC_METHOD_NOT_FOUND, "method not found"))
                        .await?;
                }
                RpcMessage::Notification { .. } | RpcMessage::Response { .. } => {
                    return Err(Error::InvalidResponse);
                }
            }
        }
    }
    /// Send a successful result. The same validation as the node applies, so a
    /// Provider can trust that an accepted response fits the contract.
    pub async fn succeed(
        &mut self,
        invocation: InvocationId,
        data: Json,
        display: Option<Display>,
    ) -> Result<(), Error> {
        let result = json!({"status": "ok", "data": data, "display": display});
        decode_provider_result(result.clone()).map_err(|_| Error::InvalidRequest)?;
        self.send_json(&rpc_result(&json!(protocol::hex(&invocation.0)), result))
            .await
    }
    /// Send an application error result.
    pub async fn fail(
        &mut self,
        invocation: InvocationId,
        failure: ProviderFailure,
    ) -> Result<(), Error> {
        let result = json!({"status": "error", "error": failure});
        decode_provider_result(result.clone()).map_err(|_| Error::InvalidRequest)?;
        self.send_json(&rpc_result(&json!(protocol::hex(&invocation.0)), result))
            .await
    }
    /// Reattach after the previous connection closed. The caller remains
    /// responsible for deciding which in-flight work must be retried.
    pub async fn reconnect(&mut self) -> Result<(), Error> {
        let next = Self::connect(self.info.clone(), self.credential, self.provider).await?;
        *self = next;
        Ok(())
    }

    /// Reattach with bounded exponential backoff for transport-level failures.
    /// Attach rejections and invalid responses are returned immediately.
    pub async fn reconnect_with_retry(&mut self, attempts: u32) -> Result<(), Error> {
        let next =
            Self::connect_with_retry(self.info.clone(), self.credential, self.provider, attempts)
                .await?;
        *self = next;
        Ok(())
    }

    /// Initial connection with bounded exponential backoff for transient
    /// transport failures.
    pub async fn connect_with_retry(
        info: ConnectionInfo,
        credential: [u8; 32],
        provider: ProviderId,
        attempts: u32,
    ) -> Result<Self, Error> {
        let attempts = attempts.max(1);
        let mut attempt = 0;
        loop {
            match Self::connect(info.clone(), credential, provider).await {
                Ok(client) => return Ok(client),
                Err(error) if attempt + 1 < attempts && retryable(&error) => {
                    let delay = Duration::from_millis(100_u64 << attempt.min(5))
                        .min(Duration::from_secs(2));
                    sleep(delay).await;
                    attempt += 1;
                }
                Err(error) => return Err(error),
            }
        }
    }

    pub async fn close(&mut self) -> Result<(), Error> {
        self.send_frame(Frame::close(1000, "")).await
    }
    async fn send_json(&mut self, value: &Json) -> Result<(), Error> {
        let text = serde_json::to_string(value).map_err(|_| Error::InvalidRequest)?;
        if text.len() > protocol::interpreter::MAX_PROVIDER_MESSAGE_BYTES {
            return Err(Error::InvalidRequest);
        }
        self.send_frame(Frame::text(&text)).await
    }
    async fn send_frame(&mut self, frame: Frame) -> Result<(), Error> {
        let bytes = encode_frame(&frame, Some(self.mask)).map_err(|_| Error::InvalidRequest)?;
        self.io
            .write_all(&bytes)
            .await
            .map_err(|_| Error::Transport)?;
        self.io.flush().await.map_err(|_| Error::Transport)
    }
    /// None means the peer sent a close frame.
    async fn read_text(&mut self) -> Result<Option<String>, Error> {
        loop {
            if let Some((frame, used)) =
                parse_frame(&self.buffer, false).map_err(|_| Error::InvalidResponse)?
            {
                self.buffer.drain(..used);
                match frame.opcode {
                    Opcode::Text => {
                        return String::from_utf8(frame.payload)
                            .map(Some)
                            .map_err(|_| Error::InvalidResponse);
                    }
                    Opcode::Ping => {
                        self.send_frame(Frame::new(true, Opcode::Pong, frame.payload))
                            .await?;
                    }
                    Opcode::Pong => {}
                    Opcode::Close => {
                        let _ = self.send_frame(Frame::close(1000, "")).await;
                        return Ok(None);
                    }
                    _ => return Err(Error::InvalidResponse),
                }
            } else {
                let mut chunk = [0u8; 8192];
                let read = self
                    .io
                    .read(&mut chunk)
                    .await
                    .map_err(|_| Error::Transport)?;
                if read == 0 {
                    return Err(Error::Transport);
                }
                self.buffer.extend_from_slice(&chunk[..read]);
                if self.buffer.len() > MAX_FRAME_PAYLOAD + 14 {
                    return Err(Error::InvalidResponse);
                }
            }
        }
    }
    async fn read_json(&mut self) -> Result<Json, Error> {
        let Some(text) = self.read_text().await? else {
            return Err(Error::InvalidResponse);
        };
        protocol::decode_app_json(text.as_bytes()).map_err(|_| Error::InvalidResponse)
    }
    async fn read_rpc(&mut self) -> Result<Option<RpcMessage>, Error> {
        let Some(text) = self.read_text().await? else {
            return Ok(None);
        };
        decode_rpc(text.as_bytes())
            .map(Some)
            .map_err(|_| Error::InvalidResponse)
    }
}
fn retryable(error: &Error) -> bool {
    matches!(
        error,
        Error::Transport | Error::Timeout | Error::Http(429 | 503)
    )
}

fn decode_invocation(id: &Json) -> Result<InvocationId, Error> {
    let text = id.as_str().ok_or(Error::InvalidResponse)?;
    protocol::unhex::<32>(text)
        .map(InvocationId)
        .map_err(|_| Error::InvalidResponse)
}
