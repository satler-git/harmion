//! Bounded RFC 6455 server connection over a hyper-upgraded TLS stream.
use crate::export::Body;
use bytes::Bytes;
use harmion_protocol::websocket::{Frame, MAX_FRAME_PAYLOAD, Opcode, encode_frame, parse_frame};
use http_body_util::{BodyExt, Full};
use hyper::{HeaderMap, Response, StatusCode, header, upgrade::Upgraded};
use hyper_util::rt::TokioIo;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const MAX_BUFFERED: usize = MAX_FRAME_PAYLOAD + 14;
pub(crate) const ATTACH_DEADLINE: Duration = Duration::from_secs(10);

#[derive(Debug, thiserror::Error)]
pub enum WsError {
    #[error("websocket I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("websocket protocol error: {0}")]
    Protocol(&'static str),
    #[error("websocket message is too large")]
    TooLarge,
    #[error("websocket peer closed without a close frame")]
    Closed,
    #[error("websocket text is not valid UTF-8")]
    InvalidUtf8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    Text(String),
    Binary(Vec<u8>),
    Close(u16, String),
}

/// Validate the Provider upgrade headers. Browser Origins are rejected by the
/// caller; this function only checks the WebSocket-specific fields.
pub fn validate_upgrade(headers: &HeaderMap) -> Result<String, WsError> {
    let connection = headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|token| token.trim().eq_ignore_ascii_case("upgrade"));
    let upgrade = headers
        .get(header::UPGRADE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("websocket"));
    let version = headers
        .get(header::SEC_WEBSOCKET_VERSION)
        .and_then(|value| value.to_str().ok())
        == Some("13");
    if !connection || !upgrade || !version {
        return Err(WsError::Protocol("invalid upgrade headers"));
    }
    let key = headers
        .get(header::SEC_WEBSOCKET_KEY)
        .and_then(|value| value.to_str().ok())
        .ok_or(WsError::Protocol("missing websocket key"))?;
    harmion_protocol::websocket::accept_key(key)
        .map_err(|_| WsError::Protocol("invalid websocket key"))?;
    Ok(key.to_owned())
}

pub fn switching_protocols(key: &str) -> Response<Body> {
    let accept = harmion_protocol::websocket::accept_key(key).expect("validated key");
    Response::builder()
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .header(header::CONNECTION, "Upgrade")
        .header(header::UPGRADE, "websocket")
        .header(header::SEC_WEBSOCKET_ACCEPT, accept)
        .header(header::CACHE_CONTROL, "no-store")
        .header("x-content-type-options", "nosniff")
        .body(
            Full::new(Bytes::new())
                .map_err(|never| match never {})
                .boxed_unsync(),
        )
        .expect("static upgrade headers")
}

pub struct WsConnection {
    io: TokioIo<Upgraded>,
    buffer: Vec<u8>,
    fragment: Option<(Opcode, Vec<u8>)>,
    closed: bool,
}
impl WsConnection {
    pub async fn accept(upgrade: Upgraded) -> Result<Self, WsError> {
        Ok(Self {
            io: TokioIo::new(upgrade),
            buffer: Vec::new(),
            fragment: None,
            closed: false,
        })
    }
    pub async fn read_message(&mut self) -> Result<Message, WsError> {
        loop {
            if let Some((frame, used)) =
                parse_frame(&self.buffer, true).map_err(|_| WsError::Protocol("invalid frame"))?
            {
                self.buffer.drain(..used);
                match frame.opcode {
                    Opcode::Ping => {
                        self.send_frame(Frame::new(true, Opcode::Pong, frame.payload))
                            .await?;
                    }
                    Opcode::Pong => {}
                    Opcode::Close => {
                        let (code, reason) = close_details(&frame.payload);
                        self.send_close(code, "").await?;
                        return Ok(Message::Close(code, reason));
                    }
                    Opcode::Text | Opcode::Binary => {
                        if self.fragment.is_some() {
                            return Err(WsError::Protocol("new message during fragmentation"));
                        }
                        if frame.fin {
                            return self.finish_message(frame.opcode, frame.payload);
                        }
                        self.fragment = Some((frame.opcode, frame.payload));
                    }
                    Opcode::Continuation => {
                        let Some((opcode, mut payload)) = self.fragment.take() else {
                            return Err(WsError::Protocol("continuation without a message"));
                        };
                        if payload.len() + frame.payload.len() > MAX_FRAME_PAYLOAD {
                            return Err(WsError::TooLarge);
                        }
                        payload.extend_from_slice(&frame.payload);
                        if frame.fin {
                            return self.finish_message(opcode, payload);
                        }
                        self.fragment = Some((opcode, payload));
                    }
                }
            } else {
                let mut chunk = [0u8; 8192];
                let read = self.io.read(&mut chunk).await?;
                if read == 0 {
                    return Err(WsError::Closed);
                }
                self.buffer.extend_from_slice(&chunk[..read]);
                if self.buffer.len() > MAX_BUFFERED {
                    return Err(WsError::TooLarge);
                }
            }
        }
    }
    fn finish_message(&self, opcode: Opcode, payload: Vec<u8>) -> Result<Message, WsError> {
        match opcode {
            Opcode::Text => String::from_utf8(payload)
                .map(Message::Text)
                .map_err(|_| WsError::InvalidUtf8),
            Opcode::Binary => Ok(Message::Binary(payload)),
            _ => Err(WsError::Protocol("invalid data message")),
        }
    }
    pub async fn send_text(&mut self, text: &str) -> Result<(), WsError> {
        self.send_frame(Frame::new(true, Opcode::Text, text.as_bytes().to_vec()))
            .await
    }
    pub async fn send_ping(&mut self) -> Result<(), WsError> {
        self.send_frame(Frame::new(true, Opcode::Ping, Vec::new()))
            .await
    }
    pub async fn send_close(&mut self, code: u16, reason: &str) -> Result<(), WsError> {
        if self.closed {
            return Ok(());
        }
        self.send_frame(Frame::close(code, reason)).await
    }
    async fn send_frame(&mut self, frame: Frame) -> Result<(), WsError> {
        let encoded = encode_frame(&frame, None).map_err(|_| WsError::Protocol("invalid frame"))?;
        self.io.write_all(&encoded).await?;
        self.io.flush().await?;
        if frame.opcode == Opcode::Close {
            self.closed = true;
        }
        Ok(())
    }
}

fn close_details(payload: &[u8]) -> (u16, String) {
    if payload.len() >= 2 {
        let code = u16::from_be_bytes([payload[0], payload[1]]);
        (code, String::from_utf8_lossy(&payload[2..]).into_owned())
    } else {
        (1000, String::new())
    }
}
