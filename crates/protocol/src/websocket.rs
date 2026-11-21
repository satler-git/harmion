//! RFC 6455 framing subset used by the loopback Provider connection.
//!
//! This is not a general WebSocket implementation: extensions, subprotocols,
//! and UTF-8 fragmentation beyond what the Provider contract needs are not
//! included. The handshake remains the responsibility of the host transport.
use crate::CodecError;
use base64::Engine;

pub const MAX_FRAME_PAYLOAD: usize = 2 * 1024 * 1024;
const WEBSOCKET_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opcode {
    Continuation,
    Text,
    Binary,
    Close,
    Ping,
    Pong,
}
impl Opcode {
    fn from_byte(byte: u8) -> Option<Self> {
        Some(match byte {
            0 => Self::Continuation,
            1 => Self::Text,
            2 => Self::Binary,
            8 => Self::Close,
            9 => Self::Ping,
            10 => Self::Pong,
            _ => return None,
        })
    }
    fn byte(self) -> u8 {
        match self {
            Self::Continuation => 0,
            Self::Text => 1,
            Self::Binary => 2,
            Self::Close => 8,
            Self::Ping => 9,
            Self::Pong => 10,
        }
    }
    pub fn is_control(self) -> bool {
        matches!(self, Self::Close | Self::Ping | Self::Pong)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub fin: bool,
    pub opcode: Opcode,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn new(fin: bool, opcode: Opcode, payload: impl Into<Vec<u8>>) -> Self {
        Self {
            fin,
            opcode,
            payload: payload.into(),
        }
    }
    pub fn text(text: &str) -> Self {
        Self::new(true, Opcode::Text, text.as_bytes().to_vec())
    }
    pub fn close(code: u16, reason: &str) -> Self {
        let mut payload = code.to_be_bytes().to_vec();
        payload.extend_from_slice(reason.as_bytes());
        Self::new(true, Opcode::Close, payload)
    }
}

/// Compute the `Sec-WebSocket-Accept` value from a client key.
pub fn accept_key(client_key: &str) -> Result<String, CodecError> {
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(client_key)
        .map_err(|_| CodecError::InvalidFieldValue)?;
    if decoded.len() != 16 {
        return Err(CodecError::InvalidFieldValue);
    }
    use sha1::{Digest, Sha1};
    let mut digest = Sha1::new();
    digest.update(client_key.as_bytes());
    digest.update(WEBSOCKET_GUID.as_bytes());
    Ok(base64::engine::general_purpose::STANDARD.encode(digest.finalize()))
}

/// Encode one complete frame. The server sends unmasked frames; tests and
/// clients may supply a mask key, which is required by RFC 6455.
pub fn encode_frame(frame: &Frame, mask: Option<[u8; 4]>) -> Result<Vec<u8>, CodecError> {
    if frame.opcode.is_control() && (!frame.fin || frame.payload.len() > 125) {
        return Err(CodecError::InvalidFieldValue);
    }
    if frame.payload.len() > MAX_FRAME_PAYLOAD {
        return Err(CodecError::TooLarge);
    }
    let mut out = Vec::with_capacity(frame.payload.len() + 14);
    out.push((frame.fin as u8) << 7 | frame.opcode.byte());
    let mask_bit = if mask.is_some() { 0x80 } else { 0 };
    match frame.payload.len() {
        length @ 0..=125 => out.push(mask_bit | length as u8),
        126..=65_535 => {
            out.push(mask_bit | 126);
            out.extend_from_slice(&(frame.payload.len() as u16).to_be_bytes());
        }
        _ => {
            out.push(mask_bit | 127);
            out.extend_from_slice(&(frame.payload.len() as u64).to_be_bytes());
        }
    }
    if let Some(key) = mask {
        out.extend_from_slice(&key);
        out.extend(
            frame
                .payload
                .iter()
                .enumerate()
                .map(|(index, byte)| byte ^ key[index % 4]),
        );
    } else {
        out.extend_from_slice(&frame.payload);
    }
    Ok(out)
}

/// Parse one frame from the beginning of `buffer`. `Ok(None)` means the buffer
/// does not contain a complete frame yet. `consumed` includes header and mask.
pub fn parse_frame(
    buffer: &[u8],
    expect_masked: bool,
) -> Result<Option<(Frame, usize)>, CodecError> {
    if buffer.len() < 2 {
        return Ok(None);
    }
    let first = buffer[0];
    let second = buffer[1];
    if first & 0x70 != 0 {
        return Err(CodecError::InvalidFieldValue);
    }
    let fin = first & 0x80 != 0;
    let opcode = Opcode::from_byte(first & 0x0f).ok_or(CodecError::InvalidFieldValue)?;
    let masked = second & 0x80 != 0;
    if masked != expect_masked {
        return Err(CodecError::InvalidFieldValue);
    }
    let length_code = second & 0x7f;
    let (length, mut offset) = match length_code {
        0..=125 => (length_code as usize, 2),
        126 => {
            if buffer.len() < 4 {
                return Ok(None);
            }
            (u16::from_be_bytes([buffer[2], buffer[3]]) as usize, 4)
        }
        _ => {
            if buffer.len() < 10 {
                return Ok(None);
            }
            let length = u64::from_be_bytes(
                buffer[2..10]
                    .try_into()
                    .map_err(|_| CodecError::InvalidFieldValue)?,
            );
            if length > MAX_FRAME_PAYLOAD as u64 {
                return Err(CodecError::TooLarge);
            }
            (
                usize::try_from(length).map_err(|_| CodecError::TooLarge)?,
                10,
            )
        }
    };
    if opcode.is_control() && (length > 125 || !fin) {
        return Err(CodecError::InvalidFieldValue);
    }
    if length > MAX_FRAME_PAYLOAD {
        return Err(CodecError::TooLarge);
    }
    let mask = if masked {
        if buffer.len() < offset + 4 {
            return Ok(None);
        }
        let key: [u8; 4] = buffer[offset..offset + 4]
            .try_into()
            .map_err(|_| CodecError::InvalidFieldValue)?;
        offset += 4;
        Some(key)
    } else {
        None
    };
    if buffer.len() < offset + length {
        return Ok(None);
    }
    let mut payload = buffer[offset..offset + length].to_vec();
    if let Some(key) = mask {
        for (index, byte) in payload.iter_mut().enumerate() {
            *byte ^= key[index % 4];
        }
    }
    Ok(Some((
        Frame {
            fin,
            opcode,
            payload,
        },
        offset + length,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accept_key_matches_the_rfc_example_and_rejects_bad_length() {
        assert_eq!(
            accept_key("dGhlIHNhbXBsZSBub25jZQ==").unwrap(),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
        assert!(accept_key("not-base64!").is_err());
        assert!(accept_key("c2hvcnQ=").is_err());
    }

    #[test]
    fn frames_roundtrip_and_partial_input_is_not_an_error() {
        for payload in [
            Vec::new(),
            vec![1],
            vec![7; 125],
            vec![9; 126],
            vec![3; 70_000],
        ] {
            let frame = Frame::new(true, Opcode::Text, payload);
            let encoded = encode_frame(&frame, Some([1, 2, 3, 4])).unwrap();
            let mut split = 0;
            while split < encoded.len() {
                assert_eq!(
                    parse_frame(&encoded[..split], true).unwrap(),
                    None,
                    "prefix {split}"
                );
                split += 1;
            }
            assert_eq!(
                parse_frame(&encoded, true).unwrap(),
                Some((frame, encoded.len()))
            );
        }
    }

    #[test]
    fn client_mask_is_mandatory_and_server_frames_are_unmasked() {
        let frame = Frame::text("hello");
        let masked = encode_frame(&frame, Some([1, 2, 3, 4])).unwrap();
        assert_eq!(
            parse_frame(&masked, false),
            Err(CodecError::InvalidFieldValue)
        );
        let unmasked = encode_frame(&frame, None).unwrap();
        assert_eq!(
            parse_frame(&unmasked, true),
            Err(CodecError::InvalidFieldValue)
        );
        assert_eq!(
            parse_frame(&unmasked, false).unwrap(),
            Some((frame, unmasked.len()))
        );
    }

    #[test]
    fn reserved_bits_unknown_opcodes_and_bad_control_frames_are_rejected() {
        assert_eq!(
            parse_frame(&[0xc1, 0x00], false),
            Err(CodecError::InvalidFieldValue)
        );
        assert_eq!(
            parse_frame(&[0x83, 0x00], false),
            Err(CodecError::InvalidFieldValue)
        );
        assert_eq!(
            parse_frame(&[0x09, 0x7e, 0, 126], false),
            Err(CodecError::InvalidFieldValue)
        );
        assert_eq!(
            parse_frame(&[0x01, 0x00], false).unwrap(),
            Some((Frame::new(false, Opcode::Text, Vec::new()), 2))
        );
    }

    #[test]
    fn oversized_length_is_rejected_before_waiting_for_the_body() {
        let mut header = vec![0x82, 0x7f];
        header.extend_from_slice(&((MAX_FRAME_PAYLOAD as u64) + 1).to_be_bytes());
        assert_eq!(parse_frame(&header, false), Err(CodecError::TooLarge));
    }
}
