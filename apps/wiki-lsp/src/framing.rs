//! LSP base protocol: `Content-Length` framed JSON messages on stdio.

use std::fmt;
use std::io::{self, BufRead, Write};

use serde_json::Value;

/// Match the Interpreter input ceiling rather than allocating an unbounded
/// buffer from an untrusted header.
const MAX_CONTENT_LENGTH: usize = 128 * 1024 * 1024;

#[derive(Debug)]
pub enum FrameError {
    Io(io::Error),
    MissingContentLength,
    InvalidContentLength(String),
    HeaderWithoutColon(String),
    TooLarge(usize),
    UnexpectedEof,
    InvalidJson(serde_json::Error),
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "io error: {error}"),
            Self::MissingContentLength => write!(f, "missing Content-Length header"),
            Self::InvalidContentLength(value) => {
                write!(f, "invalid Content-Length header value: {value:?}")
            }
            Self::HeaderWithoutColon(line) => write!(f, "malformed header line: {line:?}"),
            Self::TooLarge(length) => {
                write!(
                    f,
                    "Content-Length {length} exceeds {MAX_CONTENT_LENGTH} bytes"
                )
            }
            Self::UnexpectedEof => write!(f, "unexpected EOF while reading a framed message"),
            Self::InvalidJson(error) => write!(f, "invalid JSON body: {error}"),
        }
    }
}

impl std::error::Error for FrameError {}

impl From<io::Error> for FrameError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Read one `Content-Length` framed message.
///
/// `Ok(None)` means clean EOF before a new message. Header names are matched
/// case-insensitively and CRLF or bare LF line endings are accepted.
pub fn read_message<R: BufRead>(reader: &mut R) -> Result<Option<Value>, FrameError> {
    let mut content_length = None;
    let mut saw_header = false;

    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            if saw_header {
                return Err(FrameError::UnexpectedEof);
            }
            return Ok(None);
        }

        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            if !saw_header {
                // Tolerate stray blank lines between messages.
                continue;
            }
            break;
        }

        saw_header = true;
        let Some((name, value)) = line.split_once(':') else {
            return Err(FrameError::HeaderWithoutColon(line.to_owned()));
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            let value = value.trim();
            let length = value
                .parse::<usize>()
                .map_err(|_| FrameError::InvalidContentLength(value.to_owned()))?;
            if length > MAX_CONTENT_LENGTH {
                return Err(FrameError::TooLarge(length));
            }
            content_length = Some(length);
        }
    }

    let length = content_length.ok_or(FrameError::MissingContentLength)?;
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(FrameError::InvalidJson)
}

/// Write one `Content-Length` framed message and flush it.
pub fn write_message<W: Write>(writer: &mut W, message: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(message).map_err(io::Error::other)?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn round_trips_multiple_messages_with_byte_lengths() {
        let messages = [
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {"processId": null}
            }),
            serde_json::json!({
                "jsonrpc": "2.0",
                "method": "textDocument/didOpen",
                "params": {"textDocument": {"text": "家計の記録"}}
            }),
        ];

        let mut bytes = Vec::new();
        for message in &messages {
            write_message(&mut bytes, message).unwrap();
        }

        let text = String::from_utf8_lossy(&bytes);
        assert!(text.starts_with("Content-Length: "), "{text:?}");
        assert!(text.contains("\r\n\r\n"), "missing header terminator");

        let mut reader = Cursor::new(bytes);
        assert_eq!(read_message(&mut reader).unwrap().unwrap(), messages[0]);
        assert_eq!(read_message(&mut reader).unwrap().unwrap(), messages[1]);
        assert!(read_message(&mut reader).unwrap().is_none());
    }

    #[test]
    fn accepts_extra_headers_and_crlf() {
        let body = r#"{"jsonrpc":"2.0","id":7,"result":null}"#;
        let framed = format!(
            "Content-Type: application/vscode-jsonrpc; charset=utf-8\r\ncontent-length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        let mut reader = Cursor::new(framed.into_bytes());
        let message = read_message(&mut reader).unwrap().unwrap();
        assert_eq!(message["id"], 7);
    }

    #[test]
    fn rejects_missing_content_length() {
        let mut reader = Cursor::new(b"Content-Type: application/json\r\n\r\n{}".to_vec());
        let error = read_message(&mut reader).unwrap_err();
        assert!(matches!(error, FrameError::MissingContentLength));
    }
}
