//! Minimal OAuth refresh client for the node's separately trusted HTTPS origin.
//!
//! The App API client pins the node's App TLS leaf and must not be reused for
//! `/oauth/token`: the OAuth listener uses a different certificate rooted in the
//! node's OAuth CA. This client trusts only that CA, sends no Origin or
//! Authorization header (the token endpoint rejects both), and keeps token
//! values out of `Debug` output.

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::Request;
use hyper_util::rt::TokioIo;
use rustls::{
    ClientConfig, RootCertStore,
    pki_types::{CertificateDer, ServerName},
};
use std::{fmt, net::SocketAddr, sync::Arc, time::Duration};
use tokio::{net::TcpStream, time::timeout};
use tokio_rustls::TlsConnector;

use crate::Error;

const TOKEN_DEADLINE: Duration = Duration::from_secs(20);
const MAX_TOKEN_RESPONSE: usize = 16 * 1024;
const TOKEN_PATH: &str = "/oauth/token";

/// Rotated token pair returned by `/oauth/token` with
/// `grant_type=refresh_token`. Tokens never enter diagnostics.
#[derive(Clone, PartialEq, Eq)]
pub struct OAuthTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
    pub scope: String,
}

impl fmt::Debug for OAuthTokens {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuthTokens")
            .field("access_token", &"[redacted]")
            .field("refresh_token", &"[redacted]")
            .field("expires_in", &self.expires_in)
            .field("scope", &self.scope)
            .finish()
    }
}

/// HTTPS client for one OAuth origin, trusting exactly the supplied root DER.
pub struct OAuthTokenClient {
    endpoint: SocketAddr,
    host: String,
    tls: Arc<ClientConfig>,
}

impl OAuthTokenClient {
    /// `origin` must be the canonical `https://IP:PORT` origin published as
    /// `oauth_origin` in `connection.json` / capabilities.
    pub fn new(origin: &str, root_der: &[u8]) -> Result<Self, Error> {
        let authority = origin
            .strip_prefix("https://")
            .ok_or(Error::InvalidDiscovery)?;
        if authority.is_empty()
            || authority.contains(['/', '?', '#', '@'])
            || origin.strip_prefix("https://") != Some(authority)
        {
            return Err(Error::InvalidDiscovery);
        }
        let endpoint: SocketAddr = authority.parse().map_err(|_| Error::InvalidDiscovery)?;
        if !endpoint.ip().is_loopback() || endpoint.port() == 0 {
            return Err(Error::InvalidDiscovery);
        }
        if root_der.is_empty() {
            return Err(Error::InvalidDiscovery);
        }
        let mut roots = RootCertStore::empty();
        roots
            .add(CertificateDer::from(root_der.to_vec()))
            .map_err(|_| Error::InvalidDiscovery)?;
        let tls =
            ClientConfig::builder_with_provider(rustls::crypto::ring::default_provider().into())
                .with_safe_default_protocol_versions()
                .map_err(|_| Error::InvalidDiscovery)?
                .with_root_certificates(roots)
                .with_no_client_auth();
        Ok(Self {
            endpoint,
            host: authority.to_owned(),
            tls: Arc::new(tls),
        })
    }

    /// Exchange a refresh token. `Error::OAuth("invalid_grant")` means the
    /// family was consumed, expired, or revoked and the app must re-connect.
    pub async fn refresh(
        &self,
        client_id: &str,
        refresh_token: &str,
    ) -> Result<OAuthTokens, Error> {
        validate_hex32(client_id)?;
        validate_hex32(refresh_token)?;
        let body = token_request_body(client_id, refresh_token);
        timeout(TOKEN_DEADLINE, async {
            let tcp = TcpStream::connect(self.endpoint).await?;
            let tls = TlsConnector::from(self.tls.clone())
                .connect(ServerName::IpAddress(self.endpoint.ip().into()), tcp)
                .await
                .map_err(|_| Error::Transport)?;
            let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(tls))
                .await
                .map_err(|_| Error::Transport)?;
            let connection = tokio::spawn(async move {
                let _ = connection.await;
            });
            let request = Request::post(TOKEN_PATH)
                .header("host", self.host.as_str())
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Full::new(Bytes::from(body)))
                .map_err(|_| Error::InvalidRequest)?;
            let response = sender
                .send_request(request)
                .await
                .map_err(|_| Error::Transport)?;
            let status = response.status().as_u16();
            let bytes = Limited::new(response.into_body(), MAX_TOKEN_RESPONSE)
                .collect()
                .await
                .map_err(|_| Error::InvalidResponse)?
                .to_bytes();
            connection.abort();
            parse_token_response(status, &bytes)
        })
        .await
        .map_err(|_| Error::Timeout)?
    }
}

fn validate_hex32(value: &str) -> Result<(), Error> {
    crate::protocol::unhex::<32>(value).map_err(|_| Error::InvalidRequest)?;
    Ok(())
}

/// Fixed form encoding; both values are validated lowercase hex, so no
/// percent-encoding is required and no caller-controlled syntax leaks in.
fn token_request_body(client_id: &str, refresh_token: &str) -> Vec<u8> {
    format!("grant_type=refresh_token&client_id={client_id}&refresh_token={refresh_token}")
        .into_bytes()
}

fn parse_token_response(status: u16, bytes: &[u8]) -> Result<OAuthTokens, Error> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| Error::InvalidResponse)?;
    if status != 200 {
        let code = value
            .get("error")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("oauth_error");
        return Err(Error::OAuth(code.chars().take(64).collect()));
    }
    let field = |name: &str| {
        value
            .get(name)
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .ok_or(Error::InvalidResponse)
    };
    let access_token = field("access_token")?;
    let refresh_token = field("refresh_token")?;
    validate_hex32(&access_token)?;
    validate_hex32(&refresh_token)?;
    let expires_in = value
        .get("expires_in")
        .and_then(serde_json::Value::as_i64)
        .filter(|seconds| *seconds > 0)
        .ok_or(Error::InvalidResponse)?;
    let scope = value
        .get("scope")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned();
    Ok(OAuthTokens {
        access_token,
        refresh_token,
        expires_in,
        scope,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACCESS: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    const REFRESH: &str = "2222222222222222222222222222222222222222222222222222222222222222";

    #[test]
    fn refresh_response_rotates_both_tokens() {
        let body = serde_json::to_vec(&serde_json::json!({
            "token_type": "Bearer",
            "access_token": ACCESS,
            "refresh_token": REFRESH,
            "expires_in": 600,
            "scope": "read:kind:d2lraQ write:kind:d2lraQ",
        }))
        .unwrap();
        let tokens = parse_token_response(200, &body).unwrap();
        assert_eq!(tokens.access_token, ACCESS);
        assert_eq!(tokens.refresh_token, REFRESH);
        assert_eq!(tokens.expires_in, 600);
        assert_eq!(tokens.scope, "read:kind:d2lraQ write:kind:d2lraQ");
        // Token values must never appear in diagnostics.
        assert!(!format!("{tokens:?}").contains(ACCESS));
        assert!(!format!("{tokens:?}").contains(REFRESH));
    }

    #[test]
    fn consumed_refresh_token_is_reported_as_invalid_grant() {
        let body = br#"{"error":"invalid_grant"}"#;
        assert!(matches!(
            parse_token_response(400, body),
            Err(Error::OAuth(code)) if code == "invalid_grant"
        ));
    }

    #[test]
    fn malformed_refresh_responses_are_rejected() {
        let body = br#"{"access_token":"short","refresh_token":"also-short","expires_in":600}"#;
        assert!(matches!(
            parse_token_response(200, body),
            Err(Error::InvalidRequest)
        ));
        let body = br#"{"access_token":"1111111111111111111111111111111111111111111111111111111111111111"}"#;
        assert!(matches!(
            parse_token_response(200, body),
            Err(Error::InvalidResponse)
        ));
    }

    #[test]
    fn only_loopback_https_origins_are_accepted() {
        assert!(OAuthTokenClient::new("http://127.0.0.1:8788", &[1]).is_err());
        assert!(OAuthTokenClient::new("https://192.0.2.1:8788", &[1]).is_err());
        assert!(OAuthTokenClient::new("https://127.0.0.1:0", &[1]).is_err());
        assert!(OAuthTokenClient::new("https://127.0.0.1:8788/path", &[1]).is_err());
        // A loopback origin proceeds to root parsing; an invalid DER is rejected there.
        assert!(OAuthTokenClient::new("https://127.0.0.1:8788", &[1]).is_err());
    }

    #[test]
    fn token_request_has_no_caller_controlled_fields() {
        assert_eq!(
            String::from_utf8(token_request_body(ACCESS, REFRESH)).unwrap(),
            format!("grant_type=refresh_token&client_id={ACCESS}&refresh_token={REFRESH}")
        );
    }
}
