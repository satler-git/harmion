//! Native App API client. Does not open node databases or execute node use cases.
pub use harmion_local::ProtectedDir;
pub use harmion_protocol as protocol;
mod export;
mod export_directory;
mod provider;
mod resolution;
mod subscription;
pub use provider::{ProviderClient, ProviderEvent};
pub use subscription::{Subscription, SubscriptionCheckpoint, SubscriptionNotification};

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{Request, Response, body::Incoming};
use hyper_util::rt::TokioIo;
use protocol::connect::{
    ConnectionApproval, ConnectionCompleteRequest, ConnectionOffer, ConnectionStatusRequest,
};
use protocol::interpreter::{CallRequest, InvocationId, ProviderId, ProviderRegistration};
use protocol::query::{Condition, QueryCursor, QueryRequest};
use protocol::{ConnectionInfo, MAX_APP_RESPONSE_BYTES, MAX_RECORD_BYTES, TLS_NAME};
use rustls::{ClientConfig, RootCertStore, pki_types::CertificateDer};
use std::{net::SocketAddr, path::Path, sync::Arc, time::Duration};
use tokio::{net::TcpStream, time::timeout};
use tokio_rustls::TlsConnector;

const DEADLINE: Duration = Duration::from_secs(60);

/// Abort the transport when the request or its response consumer is dropped.
struct Connection(tokio::task::JoinHandle<()>);
impl Drop for Connection {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub fn random_id() -> Result<[u8; 32], Error> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(|_| Error::InvalidRequest)?;
    Ok(bytes)
}

#[derive(Debug, thiserror::Error)]
#[error("{self:?}")]
pub enum Error {
    Io(#[from] std::io::Error),
    InvalidDiscovery,
    InvalidRequest,
    InvalidResponse,
    Transport,
    Timeout,
    Http(u16),
    ProviderAttachRejected { code: i64 },
}

/// No Debug implementation: the bearer credential must never enter diagnostics.
pub struct Client {
    endpoint: SocketAddr,
    certificate: Vec<u8>,
    credential: [u8; 32],
    tls: Arc<ClientConfig>,
}

pub struct QueryPage {
    pub items: Vec<serde_json::Value>,
    pub next: Option<QueryCursor>,
}
impl Client {
    pub fn from_directory(path: impl AsRef<Path>) -> Result<Self, Error> {
        let directory = ProtectedDir::open(path)?;
        let info = Self::read_connection(&directory)?;
        let credential = directory
            .read("admin.secret", 32)?
            .try_into()
            .map_err(|_| Error::InvalidDiscovery)?;
        Self::new(info, credential)
    }

    /// Connect with an OAuth access token using the node's pinned App API certificate.
    pub fn from_directory_with_token(path: impl AsRef<Path>, token: &str) -> Result<Self, Error> {
        let credential = protocol::unhex(token).map_err(|_| Error::InvalidRequest)?;
        Self::new(
            Self::read_connection(&ProtectedDir::open(path)?)?,
            credential,
        )
    }

    fn read_connection(directory: &ProtectedDir) -> Result<ConnectionInfo, Error> {
        serde_json::from_slice(&directory.read("connection.json", 150_000)?)
            .map_err(|_| Error::InvalidDiscovery)
    }

    pub fn new(info: ConnectionInfo, credential: [u8; 32]) -> Result<Self, Error> {
        let pinned = PinnedEndpoint::new(&info)?;
        Ok(Self {
            endpoint: pinned.endpoint,
            certificate: pinned.certificate,
            credential,
            tls: pinned.tls,
        })
    }

    pub async fn call(&self, operation: &str, body: &[u8]) -> Result<serde_json::Value, Error> {
        timeout(DEADLINE, async {
            let (response, _connection) = self.request(operation, body).await?;
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

    /// Verify the pinned TLS leaf before constructing or sending Authorization.
    async fn request(
        &self,
        operation: &str,
        body: &[u8],
    ) -> Result<(Response<Incoming>, Connection), Error> {
        if operation.is_empty()
            || !operation
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b == b'-')
            || body.len() > MAX_RECORD_BYTES
        {
            return Err(Error::InvalidRequest);
        }
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
            let _ = connection.await;
        });
        let connection = Connection(connection);
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
        Ok((response, connection))
    }

    pub async fn save(&self, request: &protocol::SaveRequest) -> Result<serde_json::Value, Error> {
        let bytes = protocol::encode_save_request(request).map_err(|_| Error::InvalidRequest)?;
        self.call("save", &bytes).await
    }

    /// Join through an owner-verified peer registration and publish the updated
    /// local registration for subsequent peer approvals.
    pub async fn join_directory(
        &self,
        directory: &ProtectedDir,
        peer_registration: &[u8],
        recovery: bool,
    ) -> Result<serde_json::Value, Error> {
        let operation = if recovery { "join-recovery" } else { "join" };
        let registration = self.call(operation, peer_registration).await?;
        let bytes = serde_json::to_vec(&registration).map_err(|_| Error::InvalidResponse)?;
        directory.replace("peer.json", &bytes)?;
        Ok(registration)
    }
    pub async fn get(
        &self,
        content: protocol::domain::ContentId,
    ) -> Result<serde_json::Value, Error> {
        self.call("get", &content.0).await
    }

    /// Public connection endpoints do not require an app credential; the
    // one-time request secret authorizes status and completion.
    pub fn anonymous(info: ConnectionInfo) -> Result<Self, Error> {
        Self::new(info, [0; 32])
    }

    pub async fn connect_start(&self, offer: &ConnectionOffer) -> Result<serde_json::Value, Error> {
        let bytes = serde_json::to_vec(offer).map_err(|_| Error::InvalidRequest)?;
        self.call("connect-start", &bytes).await
    }

    pub async fn connect_status(
        &self,
        request: &ConnectionStatusRequest,
    ) -> Result<serde_json::Value, Error> {
        let bytes = serde_json::to_vec(request).map_err(|_| Error::InvalidRequest)?;
        self.call("connect-status", &bytes).await
    }

    pub async fn connect_complete(
        &self,
        request: &ConnectionCompleteRequest,
    ) -> Result<serde_json::Value, Error> {
        let bytes = serde_json::to_vec(request).map_err(|_| Error::InvalidRequest)?;
        self.call("connect-complete", &bytes).await
    }

    pub async fn connect_list(&self) -> Result<serde_json::Value, Error> {
        self.call("connect-list", &[]).await
    }

    pub async fn connect_approve(
        &self,
        approval: &ConnectionApproval,
    ) -> Result<serde_json::Value, Error> {
        let bytes = serde_json::to_vec(approval).map_err(|_| Error::InvalidRequest)?;
        self.call("connect-approve", &bytes).await
    }

    pub async fn interpreter_list(&self) -> Result<serde_json::Value, Error> {
        self.call("interpreter-list", &[]).await
    }

    pub async fn interpreter_describe(
        &self,
        provider: ProviderId,
    ) -> Result<serde_json::Value, Error> {
        self.call("interpreter-describe", &provider.0).await
    }

    pub async fn interpreter_register(
        &self,
        registration: &ProviderRegistration,
    ) -> Result<serde_json::Value, Error> {
        let bytes = serde_json::to_vec(registration).map_err(|_| Error::InvalidRequest)?;
        self.call("interpreter-register", &bytes).await
    }

    pub async fn interpreter_approve(
        &self,
        provider: ProviderId,
        replace: bool,
    ) -> Result<serde_json::Value, Error> {
        let bytes = serde_json::to_vec(&serde_json::json!({
            "provider_id": provider,
            "replace": replace,
        }))
        .map_err(|_| Error::InvalidRequest)?;
        self.call("interpreter-approve", &bytes).await
    }

    pub async fn interpreter_call(
        &self,
        request: &CallRequest,
    ) -> Result<serde_json::Value, Error> {
        let bytes = serde_json::to_vec(request).map_err(|_| Error::InvalidRequest)?;
        self.call("interpreter-call", &bytes).await
    }

    pub async fn interpreter_cancel(
        &self,
        invocation: InvocationId,
    ) -> Result<serde_json::Value, Error> {
        self.call("interpreter-cancel", &invocation.0).await
    }

    pub async fn interpreter_ticket(
        &self,
        provider: ProviderId,
    ) -> Result<serde_json::Value, Error> {
        self.call("interpreter-ticket", &provider.0).await
    }

    /// Read owner-only historical replication confirmations without contacting peers.
    /// An unconfirmed update may already have arrived through another node.
    pub async fn sync_status(
        &self,
        update: Option<protocol::domain::UpdateId>,
    ) -> Result<protocol::sync_status::SyncStatus, Error> {
        let body = update.as_ref().map_or(&[][..], |id| id.0.as_slice());
        serde_json::from_value(self.call("sync-status", body).await?)
            .map_err(|_| Error::InvalidResponse)
    }

    pub async fn query(&self, condition: Condition, limit: u32) -> Result<QueryPage, Error> {
        self.query_page(QueryRequest::Start { condition, limit })
            .await
    }

    pub async fn query_next(&self, cursor: QueryCursor, limit: u32) -> Result<QueryPage, Error> {
        self.query_page(QueryRequest::Next { cursor, limit }).await
    }

    async fn query_page(&self, request: QueryRequest) -> Result<QueryPage, Error> {
        let request =
            protocol::query::encode_request(&request).map_err(|_| Error::InvalidRequest)?;
        let mut response = self.call("query", &request).await?;
        let items = match response.get_mut("items").map(serde_json::Value::take) {
            Some(serde_json::Value::Array(items)) => items,
            _ => return Err(Error::InvalidResponse),
        };
        let next = match response.get("next") {
            Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::String(cursor)) => Some(QueryCursor(
                protocol::unhex(cursor).map_err(|_| Error::InvalidResponse)?,
            )),
            _ => return Err(Error::InvalidResponse),
        };
        Ok(QueryPage { items, next })
    }
}

/// A loopback App/Provider TLS endpoint with a pinned leaf certificate.
pub(crate) struct PinnedEndpoint {
    pub endpoint: SocketAddr,
    pub certificate: Vec<u8>,
    pub tls: Arc<ClientConfig>,
}
impl PinnedEndpoint {
    pub(crate) fn new(info: &ConnectionInfo) -> Result<Self, Error> {
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
            tls: Arc::new(tls),
        })
    }
}
