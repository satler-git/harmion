//! Bounded HTTPS peer transport and fixed-high bidirectional sync rounds.
use crate::{
    Error, SharedStore,
    tls::{SERVER_NAME, TlsIdentity},
    transport,
};
use bytes::Bytes;
use harmion_domain::NodeId;
use harmion_node::{
    membership::{self, Peer},
    storage::{Identity, Store},
};
use harmion_protocol::*;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{Request, Response, StatusCode, body::Incoming, service::service_fn};
use hyper_util::rt::TokioIo;
use std::{convert::Infallible, net::SocketAddr, sync::Arc, time::Duration};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{Semaphore, watch},
    time::timeout,
};
use tokio_rustls::{TlsAcceptor, TlsConnector};

/// Validate host-specific routing and certificate syntax before persisting approval.
pub async fn approve<S: Store + Send + 'static>(
    store: &SharedStore<S>,
    credential: harmion_node::access::Credential,
    now: i64,
    cluster: harmion_domain::ClusterId,
    peer: Peer,
) -> Result<(), Error> {
    validate_binding(&peer)?;
    store
        .run(move |s| membership::approve(s, now, &credential, cluster, peer))
        .await
}

pub fn validate_binding(peer: &Peer) -> Result<(), Error> {
    match &peer.key {
        membership::PeerKey::TlsCertificate(certificate) => {
            crate::tls::validate_certificate(certificate)?;
            peer.endpoint.parse::<SocketAddr>().map_err(transport)?;
        }
        membership::PeerKey::Iroh(key) => {
            iroh::EndpointId::from_bytes(key).map_err(transport)?;
            if !peer.endpoint.is_empty() {
                peer.endpoint.parse::<iroh::RelayUrl>().map_err(transport)?;
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_membership_page(
    page: &Page<harmion_protocol::membership::Certificate>,
) -> Result<(), Error> {
    for entry in &page.entries {
        if let harmion_protocol::membership::Action::Admit(member) =
            &entry.record.statement().action
        {
            validate_binding(&harmion_node::membership_state::as_peer(member))?;
        }
    }
    Ok(())
}

const DEADLINE: Duration = Duration::from_secs(15);
type Body = Full<Bytes>;

pub async fn serve<S: Store + Send + 'static>(
    listener: TcpListener,
    store: SharedStore<S>,
    identity: Arc<TlsIdentity>,
    mut stop: watch::Receiver<bool>,
) -> Result<(), Error> {
    let slots = Arc::new(Semaphore::new(16));
    let mut connections = tokio::task::JoinSet::new();
    loop {
        let (tcp, _) = tokio::select! {
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() {
                    connections.abort_all();
                    while connections.join_next().await.is_some() {}
                    return Ok(());
                }
                continue;
            },
            Some(_) = connections.join_next(), if !connections.is_empty() => continue,
            accepted = listener.accept() => accepted?,
        };
        let Ok(permit) = slots.clone().try_acquire_owned() else {
            continue;
        };
        let store = store.clone();
        let identity = identity.clone();
        connections.spawn(async move {
            let _permit = permit;
            let _ = timeout(DEADLINE, async {
                let peers = store.run(|s| s.transaction(|tx| tx.peers())).await?;
                let config = identity.peer_server_config(&peers)?;
                let tls = TlsAcceptor::from(Arc::new(config))
                    .accept(tcp)
                    .await
                    .map_err(transport)?;
                let certificate = tls
                    .get_ref()
                    .1
                    .peer_certificates()
                    .and_then(|chain| chain.first())
                    .ok_or(Error::InvalidResponse)?
                    .to_vec();
                let service =
                    service_fn(move |request| respond(store.clone(), certificate.clone(), request));
                hyper::server::conn::http1::Builder::new()
                    .max_headers(32)
                    .max_buf_size(16 * 1024)
                    .serve_connection(TokioIo::new(tls), service)
                    .await
                    .map_err(transport)
            })
            .await;
        });
    }
}

fn response(status: StatusCode, bytes: impl Into<Bytes>) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("content-type", "application/vnd.harmion.peer-v1")
        .header("cache-control", "no-store")
        .body(Full::new(bytes.into()))
        .expect("static headers")
}
async fn respond<S: Store + Send + 'static>(
    store: SharedStore<S>,
    certificate: Vec<u8>,
    request: Request<Incoming>,
) -> Result<Response<Body>, Infallible> {
    if request.method() != hyper::Method::POST
        || request.uri().path() != "/peer/v1"
        || request.uri().query().is_some()
        || request.headers().get("host").and_then(|h| h.to_str().ok()) != Some(SERVER_NAME)
        || request.headers().contains_key("origin")
    {
        return Ok(response(StatusCode::BAD_REQUEST, "invalid peer request"));
    }
    let body = match Limited::new(request.into_body(), MAX_PEER_BYTES)
        .collect()
        .await
    {
        Ok(body) => body.to_bytes(),
        Err(_) => return Ok(response(StatusCode::PAYLOAD_TOO_LARGE, "invalid body")),
    };
    let request = match decode_peer_request(&body) {
        Ok(request) => request,
        Err(_) => return Ok(response(StatusCode::BAD_REQUEST, "invalid peer encoding")),
    };
    if let PeerCommand::MembershipAccept(page) = &request.command
        && validate_membership_page(page).is_err()
    {
        return Ok(response(StatusCode::BAD_REQUEST, "invalid member binding"));
    }
    match store
        .run(move |s| {
            membership::handle(
                s,
                &membership::PeerKey::TlsCertificate(certificate),
                &request,
            )
        })
        .await
    {
        Ok(reply) => match encode_peer_response(&reply) {
            Ok(bytes) => Ok(response(StatusCode::OK, bytes)),
            Err(_) => Ok(response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "response encoding failed",
            )),
        },
        Err(Error::Node(
            harmion_node::Error::Unauthenticated
            | harmion_node::Error::PeerRetired
            | harmion_node::Error::MembershipConflict
            | harmion_node::Error::IdentityMismatch,
        )) => Ok(response(StatusCode::FORBIDDEN, "peer not authorized")),
        Err(Error::Node(harmion_node::Error::Storage(_) | harmion_node::Error::CorruptLog)) => Ok(
            response(StatusCode::INTERNAL_SERVER_ERROR, "storage failure"),
        ),
        Err(_) => Ok(response(StatusCode::BAD_REQUEST, "peer operation rejected")),
    }
}

/// A fresh TLS connection verifies possession and the pinned leaf before sending
/// any record bytes. HTTP redirects/proxies are not used.
pub async fn exchange(
    identity: &TlsIdentity,
    local: Identity,
    peer: &Peer,
    command: PeerCommand,
) -> Result<PeerReply, Error> {
    timeout(DEADLINE, async {
        let endpoint: SocketAddr = peer.endpoint.parse().map_err(transport)?;
        let tcp = TcpStream::connect(endpoint).await?;
        let tls = TlsConnector::from(Arc::new(identity.peer_client_config(peer)?))
            .connect(SERVER_NAME.try_into().map_err(transport)?, tcp)
            .await
            .map_err(transport)?;
        if tls
            .get_ref()
            .1
            .peer_certificates()
            .and_then(|c| c.first())
            .map(|c| c.as_ref())
            != Some(peer.key.tls_certificate()?)
        {
            return Err(Error::InvalidResponse);
        }
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(tls))
            .await
            .map_err(transport)?;
        tokio::spawn(async move {
            let _ = timeout(DEADLINE, connection).await;
        });
        let bytes = encode_peer_request(&PeerRequest {
            cluster: local.cluster,
            node: local.node,
            command,
        })?;
        let request = Request::post("/peer/v1")
            .header("host", SERVER_NAME)
            .header("content-type", "application/vnd.harmion.peer-v1")
            .body(Full::new(Bytes::from(bytes)))
            .map_err(transport)?;
        let response = sender.send_request(request).await.map_err(transport)?;
        if response.status() != StatusCode::OK {
            return Err(Error::Transport(format!("peer HTTP {}", response.status())));
        }
        let body = Limited::new(response.into_body(), MAX_PEER_BYTES)
            .collect()
            .await
            .map_err(transport)?
            .to_bytes();
        let response = decode_peer_response(&body)?;
        if response.cluster != local.cluster || response.node != peer.node {
            return Err(Error::InvalidResponse);
        }
        if let PeerReply::Rejected { update_id, reason } = response.reply {
            return Err(Error::RemoteRejected { update_id, reason });
        }
        Ok(response.reply)
    })
    .await
    .map_err(|_| Error::Timeout)?
}

pub use crate::sync::SyncReport;

/// HTTPS adapter retained for the explicit HTTPS peer path.
pub struct HttpsTransport<'a>(pub &'a TlsIdentity);
impl crate::sync::PeerTransport for HttpsTransport<'_> {
    async fn exchange(
        &self,
        local: Identity,
        peer: &Peer,
        command: PeerCommand,
    ) -> Result<PeerReply, Error> {
        exchange(self.0, local, peer, command).await
    }
}
pub async fn sync_once<S: Store + Send + 'static>(
    store: &SharedStore<S>,
    tls: &TlsIdentity,
    node: NodeId,
) -> Result<SyncReport, Error> {
    crate::sync::sync_once(store, &HttpsTransport(tls), node).await
}
