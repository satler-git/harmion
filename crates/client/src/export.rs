use crate::{Client, DEADLINE, Error, protocol};
use http_body_util::BodyExt;
use protocol::export::{CONTENT_TYPE, FORMAT, MAX_FRAME_BYTES, SCOPE, VERSION};
use serde_json::Value;
use std::io;
use tokio::time::timeout;

impl Client {
    /// Consume an owner export one record at a time. A successful return verifies
    /// the format header, completion marker, record count and end of response.
    pub async fn export(
        &self,
        mut consume: impl FnMut(Value) -> io::Result<()>,
    ) -> Result<(), Error> {
        let (response, _connection) = timeout(DEADLINE, self.request("export", &[]))
            .await
            .map_err(|_| Error::Timeout)??;
        if response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            != Some(CONTENT_TYPE)
        {
            return Err(Error::InvalidResponse);
        }
        let mut body = response.into_body();
        let mut line = Vec::new();
        let mut decoder = Records::default();
        while let Some(frame) = timeout(DEADLINE, body.frame())
            .await
            .map_err(|_| Error::Timeout)?
        {
            let frame = frame.map_err(|_| Error::Transport)?;
            let Some(bytes) = frame.data_ref() else {
                continue;
            };
            let mut rest = bytes.as_ref();
            while !rest.is_empty() {
                let newline = rest.iter().position(|&byte| byte == b'\n');
                let length = newline.unwrap_or(rest.len());
                if length > MAX_FRAME_BYTES - line.len() {
                    return Err(Error::InvalidResponse);
                }
                line.extend_from_slice(&rest[..length]);
                if newline.is_some() {
                    decoder.line(&line, &mut consume)?;
                    line.clear();
                    rest = &rest[length + 1..];
                } else {
                    break;
                }
            }
        }
        decoder.finish(&line)
    }
}

#[derive(Default)]
struct Records {
    header: bool,
    ended: bool,
    count: u64,
}

impl Records {
    fn finish(&self, partial_line: &[u8]) -> Result<(), Error> {
        if self.ended && partial_line.is_empty() {
            Ok(())
        } else {
            Err(Error::InvalidResponse)
        }
    }

    fn line(
        &mut self,
        bytes: &[u8],
        consume: &mut impl FnMut(Value) -> io::Result<()>,
    ) -> Result<(), Error> {
        if self.ended {
            return Err(Error::InvalidResponse);
        }
        let record = protocol::decode_app_json(bytes).map_err(|_| Error::InvalidResponse)?;
        match record["type"].as_str() {
            Some("header")
                if !self.header
                    && record["format"] == FORMAT
                    && record["version"].as_u64() == Some(VERSION.into())
                    && record["scope"] == SCOPE =>
            {
                self.header = true;
            }
            Some("end") if self.header && record["records"].as_u64() == Some(self.count) => {
                self.ended = true;
                return Ok(());
            }
            Some("content" | "body" | "field" | "candidate" | "existence_head" | "deletion")
                if self.header =>
            {
                self.count = self.count.checked_add(1).ok_or(Error::InvalidResponse)?;
            }
            _ => return Err(Error::InvalidResponse),
        }
        consume(record)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn export_requires_a_matching_end_record_and_clean_eof() {
        let header = json!({"type":"header","format":FORMAT,"version":VERSION,"scope":SCOPE});
        let mut decoder = Records::default();
        let mut consume = |_| Ok(());
        decoder
            .line(&serde_json::to_vec(&header).unwrap(), &mut consume)
            .unwrap();
        decoder
            .line(br#"{"type":"content"}"#, &mut consume)
            .unwrap();
        assert!(decoder.finish(&[]).is_err());
        assert!(
            decoder
                .line(br#"{"type":"end","records":0}"#, &mut consume)
                .is_err()
        );
        decoder
            .line(br#"{"type":"end","records":1}"#, &mut consume)
            .unwrap();
        assert!(decoder.finish(b"trailing partial record").is_err());
        assert!(
            decoder
                .line(br#"{"type":"content"}"#, &mut consume)
                .is_err()
        );
        assert!(decoder.finish(&[]).is_ok());
    }
}
