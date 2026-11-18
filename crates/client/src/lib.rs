//! Native App API client. Does not open node databases or execute node use cases.
pub use harmion_local::ProtectedDir;
pub use harmion_protocol as protocol;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::Request;
use hyper_util::rt::TokioIo;
use protocol::{ConnectionInfo, MAX_APP_RESPONSE_BYTES, MAX_RECORD_BYTES, TLS_NAME};
use rustls::{ClientConfig, RootCertStore, pki_types::CertificateDer};
use std::{net::SocketAddr, path::Path, sync::Arc, time::Duration};
use tokio::{net::TcpStream, time::timeout};
use tokio_rustls::TlsConnector;

const DEADLINE: Duration = Duration::from_secs(60);

pub fn random_id() -> Result<[u8; 32], Error> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(|_| Error::InvalidRequest)?;
    Ok(bytes)
}

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    InvalidDiscovery,
    InvalidRequest,
    InvalidResponse,
    Transport,
    Timeout,
    Http(u16),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// No Debug implementation: the bearer credential must never enter diagnostics.
pub struct Client {
    endpoint: SocketAddr,
    certificate: Vec<u8>,
    credential: [u8; 32],
    tls: Arc<ClientConfig>,
}
impl Client {
    pub fn from_directory(path: impl AsRef<Path>) -> Result<Self, Error> {
        let directory = ProtectedDir::open(path)?;
        let info: ConnectionInfo =
            serde_json::from_slice(&directory.read("connection.json", 150_000)?)
                .map_err(|_| Error::InvalidDiscovery)?;
        let credential = directory
            .read("admin.secret", 32)?
            .try_into()
            .map_err(|_| Error::InvalidDiscovery)?;
        Self::new(info, credential)
    }

    pub fn new(info: ConnectionInfo, credential: [u8; 32]) -> Result<Self, Error> {
        let endpoint: SocketAddr = info.endpoint.parse().map_err(|_| Error::InvalidDiscovery)?;
        if info.version != 1
            || !endpoint.ip().is_loopback()
            || endpoint.port() == 0
            || protocol::unhex::<32>(&info.cluster_id).is_err()
            || protocol::unhex::<32>(&info.node_id).is_err()
        {
            return Err(Error::InvalidDiscovery);
        }
        let certificate = protocol::unhex_bytes(&info.certificate, 64 * 1024)
            .map_err(|_| Error::InvalidDiscovery)?;
        let mut roots = RootCertStore::empty();
        roots
            .add(CertificateDer::from(certificate.clone()))
            .map_err(|_| Error::InvalidDiscovery)?;
        let tls =
            ClientConfig::builder_with_provider(rustls::crypto::ring::default_provider().into())
                .with_safe_default_protocol_versions()
                .map_err(|_| Error::InvalidDiscovery)?
                .with_root_certificates(roots)
                .with_no_client_auth();
        Ok(Self {
            endpoint,
            certificate,
            credential,
            tls: Arc::new(tls),
        })
    }

    /// Verify the pinned TLS leaf before constructing or sending Authorization.
    /// There is no proxy, redirect, or plaintext fallback.
    pub async fn call(&self, operation: &str, body: &[u8]) -> Result<serde_json::Value, Error> {
        if operation.is_empty()
            || !operation
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b == b'-')
            || body.len() > MAX_RECORD_BYTES
        {
            return Err(Error::InvalidRequest);
        }
        timeout(DEADLINE, async {
            let tcp = TcpStream::connect(self.endpoint).await?;
            let tls = TlsConnector::from(self.tls.clone())
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
                .and_then(|c| c.first())
                .map(|c| c.as_ref())
                != Some(self.certificate.as_slice())
            {
                return Err(Error::InvalidResponse);
            }
            let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(tls))
                .await
                .map_err(|_| Error::Transport)?;
            let connection = tokio::spawn(async move {
                let _ = timeout(DEADLINE, connection).await;
            });
            // Abort the transport even if this future is cancelled or times out.
            struct Connection(tokio::task::JoinHandle<()>);
            impl Drop for Connection {
                fn drop(&mut self) {
                    self.0.abort();
                }
            }
            let _connection = Connection(connection);
            let mut authorization = hyper::header::HeaderValue::from_str(&format!(
                "Bearer {}",
                protocol::hex(&self.credential)
            ))
            .map_err(|_| Error::InvalidRequest)?;
            authorization.set_sensitive(true);
            let request = Request::post(format!("/app/v1/{operation}"))
                .header("host", TLS_NAME)
                .header("authorization", authorization)
                .header("content-type", "application/octet-stream")
                .body(Full::new(Bytes::copy_from_slice(body)))
                .map_err(|_| Error::InvalidRequest)?;
            let response = sender
                .send_request(request)
                .await
                .map_err(|_| Error::Transport)?;
            if response.status() != hyper::StatusCode::OK {
                return Err(Error::Http(response.status().as_u16()));
            }
            let bytes = Limited::new(response.into_body(), MAX_APP_RESPONSE_BYTES)
                .collect()
                .await
                .map_err(|_| Error::InvalidResponse)?
                .to_bytes();
            protocol::decode_app_json(&bytes).map_err(|_| Error::InvalidResponse)
        })
        .await
        .map_err(|_| Error::Timeout)?
    }

    pub async fn save(&self, request: &protocol::SaveRequest) -> Result<serde_json::Value, Error> {
        let bytes = protocol::encode_save_request(request).map_err(|_| Error::InvalidRequest)?;
        self.call("save", &bytes).await
    }
    pub async fn get(
        &self,
        content: protocol::domain::ContentId,
    ) -> Result<serde_json::Value, Error> {
        self.call("get", &content.0).await
    }
}
