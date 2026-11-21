//! Native signed-HTTPS peer v2 entry (server) and outbound URL transport.
//!
//! TLS here is ordinary WebPKI server authentication; the node signature over
//! the canonical frame proves the pinned Harmion identity. Redirects are never
//! followed: the client only accepts a 200 response bound to its live attempt.
use crate::{Error, SharedStore, tls::TlsIdentity, transport};
use bytes::Bytes;

use harmion_node::{
    application::Host as _,
    https_peer::{self, ChallengeStore, Config, ExchangeOutcome, request_class},
    identity::KnownIdentity,
    membership::{self, Peer, PeerKey},
    membership_state::{self, MembershipSigner},
    storage::{Identity, Store},
};
use harmion_protocol::{
    PeerCommand, PeerReply, PeerRequest, decode_peer_response, encode_peer_request,
    encode_peer_response,
    https_peer::{
        Body, CHALLENGE_PATH, Commitment, Common, EXCHANGE_PATH, ErrorCode, ExchangeContext, Frame,
        MAX_FRAME_BYTES, Nonce, PayloadClass, ResponseResult, validate_origin,
    },
};
use http_body_util::{BodyExt, Full, Limited};
use hyper::{Request, Response, StatusCode, body::Incoming, service::service_fn};
use hyper_util::rt::TokioIo;
use rustls::{ClientConfig, RootCertStore, pki_types::CertificateDer};
use std::{sync::Arc, time::Duration};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{Semaphore, watch},
    time::timeout,
};
use tokio_rustls::{TlsAcceptor, TlsConnector};

const DEADLINE: Duration = Duration::from_secs(15);
const CONTENT_TYPE: &str = "application/vnd.harmion.https-peer-v2";

fn random_nonce() -> Result<Nonce, harmion_node::Error> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(|error| harmion_node::Error::Entropy(error.to_string()))?;
    Ok(bytes)
}

fn authority(origin: &str) -> Result<String, Error> {
    let parsed = url::Url::parse(origin).map_err(transport)?;
    let host = parsed.host_str().ok_or(Error::InvalidResponse)?;
    Ok(match parsed.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    })
}

fn response(status: StatusCode, bytes: impl Into<Bytes>) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header("content-type", CONTENT_TYPE)
        .header("cache-control", "no-store")
        .body(Full::new(bytes.into()))
        .expect("static headers")
}

fn generic(status: StatusCode) -> Response<Full<Bytes>> {
    response(status, Bytes::new())
}

fn status_for(error: &Error) -> StatusCode {
    match error {
        Error::Node(harmion_node::Error::ClockReversal) => StatusCode::SERVICE_UNAVAILABLE,
        Error::Node(harmion_node::Error::Storage(_) | harmion_node::Error::CorruptLog) => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        Error::Node(harmion_node::Error::InvalidRequest) => StatusCode::BAD_REQUEST,
        _ => StatusCode::BAD_REQUEST,
    }
}

/// Authenticated peer entry. Serves only the two fixed routes; any other path,
/// query, Origin header or body over the route's bound is rejected without state.
///
/// The SQLite adapter commits challenge consumption in its own transaction
/// (`synchronous=FULL`) before the node handler runs, and commits the node
/// handler's transaction before the signed response is built.
pub async fn serve<S: Store + ChallengeStore + Send + 'static>(
    listener: TcpListener,
    identity: Arc<TlsIdentity>,
    store: SharedStore<S>,
    signer: Arc<MembershipSigner>,
    config: Config,
    mut stop: watch::Receiver<bool>,
) -> Result<(), Error> {
    let accepted_host = authority(&config.origin)?;
    let acceptor = TlsAcceptor::from(Arc::new(identity.app_server_config()?));
    let slots = Arc::new(Semaphore::new(8));
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
        let (store, signer, config, accepted_host) = (
            store.clone(),
            signer.clone(),
            config.clone(),
            accepted_host.clone(),
        );
        let acceptor = acceptor.clone();
        connections.spawn(async move {
            let _permit = permit;
            let result = timeout(DEADLINE, async {
                let tls = acceptor.accept(tcp).await.map_err(transport)?;
                let service = service_fn(move |request| {
                    respond(
                        store.clone(),
                        signer.clone(),
                        config.clone(),
                        accepted_host.clone(),
                        request,
                    )
                });
                hyper::server::conn::http1::Builder::new()
                    .max_headers(32)
                    .max_buf_size(16 * 1024)
                    .serve_connection(TokioIo::new(tls), service)
                    .await
                    .map_err(transport)
            })
            .await;
            crate::trace_connection("signed peer HTTPS", result);
        });
    }
}

async fn respond<S: Store + ChallengeStore + Send + 'static>(
    store: SharedStore<S>,
    signer: Arc<MembershipSigner>,
    config: Config,
    accepted_host: String,
    request: Request<Incoming>,
) -> Result<Response<Full<Bytes>>, std::convert::Infallible> {
    let path = request.uri().path().to_owned();
    if request.method() != hyper::Method::POST
        || (path != CHALLENGE_PATH && path != EXCHANGE_PATH)
        || request.uri().query().is_some()
        || request.headers().contains_key("origin")
        || request.headers().get("host").and_then(|h| h.to_str().ok())
            != Some(accepted_host.as_str())
    {
        return Ok(generic(StatusCode::BAD_REQUEST));
    }
    let limit = if path == CHALLENGE_PATH {
        harmion_protocol::https_peer::MAX_METADATA_BYTES
    } else {
        MAX_FRAME_BYTES
    };
    let body = match Limited::new(request.into_body(), limit).collect().await {
        Ok(body) => body.to_bytes(),
        Err(_) => return Ok(generic(StatusCode::PAYLOAD_TOO_LARGE)),
    };
    let frame = match Frame::decode(&body) {
        Ok(frame) => frame,
        Err(_) => return Ok(generic(StatusCode::BAD_REQUEST)),
    };
    let local: Identity = match store.run(|s| s.transaction(|tx| Ok(tx.identity()))).await {
        Ok(local) => local,
        Err(_) => return Ok(generic(StatusCode::SERVICE_UNAVAILABLE)),
    };
    if https_peer::check_common(&frame, local, &config.origin).is_err() {
        return Ok(generic(StatusCode::BAD_REQUEST));
    }
    let requester = frame.common.sender;
    // Resolve the durable pin before trusting anything in the frame.
    let pin: Option<KnownIdentity> = match store
        .run(move |s| s.transaction(|tx| tx.known_peer_identity(requester)))
        .await
    {
        Ok(pin) => pin,
        Err(_) => return Ok(generic(StatusCode::SERVICE_UNAVAILABLE)),
    };
    let Some(pin) = pin else {
        return Ok(generic(StatusCode::FORBIDDEN));
    };
    if https_peer::verify_frame(&frame, &pin.signing_key).is_err() {
        return Ok(generic(StatusCode::FORBIDDEN));
    }
    // Preliminary class authorization: data needs both peers currently Active.
    // The node handler rechecks this in its own accept transaction.
    if matches!(request_class(&frame), Ok(PayloadClass::Data)) {
        let checked = store
            .run(move |s| {
                s.transaction(|tx| membership_state::active_member(tx, requester).map(|_| ()))
            })
            .await;
        if let Err(error) = checked {
            return Ok(generic(match error {
                Error::Node(harmion_node::Error::PeerRetired) => StatusCode::FORBIDDEN,
                Error::Node(
                    harmion_node::Error::Unauthenticated
                    | harmion_node::Error::MembershipConflict
                    | harmion_node::Error::IdentityMismatch,
                ) => StatusCode::FORBIDDEN,
                _ => StatusCode::SERVICE_UNAVAILABLE,
            }));
        }
    }
    let now = crate::SystemHost.now_micros();
    let session = https_peer::Session {
        signer: &signer,
        config: &config,
        local,
        requester,
        now,
    };
    let outcome = if path == CHALLENGE_PATH {
        let signer = signer.clone();
        let config = config.clone();
        let frame = frame.clone();
        match store
            .run_challenge(move |s| {
                let session = https_peer::Session {
                    signer: &signer,
                    config: &config,
                    local,
                    requester,
                    now,
                };
                https_peer::issue_challenge(s, &session, &frame, &mut random_nonce)
            })
            .await
        {
            Ok(outcome) => outcome,
            Err(error) => return Ok(generic(status_for(&error))),
        }
    } else {
        let begin = {
            let signer = signer.clone();
            let config = config.clone();
            let frame = frame.clone();
            store
                .run_challenge(move |s| {
                    let session = https_peer::Session {
                        signer: &signer,
                        config: &config,
                        local,
                        requester,
                        now,
                    };
                    https_peer::begin_exchange(s, &session, &frame, &mut random_nonce)
                })
                .await
        };
        let consumed = match begin {
            Ok(ExchangeOutcome::Rejected(rejected)) => {
                let status = StatusCode::from_u16(rejected.http_status())
                    .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
                let bytes = rejected.encode().unwrap_or_default();
                return Ok(response(status, Bytes::from(bytes)));
            }
            Ok(ExchangeOutcome::Consumed(consumed)) => consumed,
            Err(error) => return Ok(generic(status_for(&error))),
        };
        let key = PeerKey::SignedHttps(pin.signing_key);
        let request = consumed.request.clone();
        let handled = store
            .run_notifying(true, move |s| {
                membership::handle(s, &key, &request)
                    .and_then(|reply| Ok(encode_peer_response(&reply)?))
            })
            .await
            .map_err(|error| match error {
                Error::Node(error) => error,
                other => harmion_node::Error::Storage(other.to_string()),
            });
        match https_peer::complete_exchange(&session, &consumed, handled) {
            Ok(outcome) => outcome,
            Err(_) => return Ok(generic(StatusCode::INTERNAL_SERVER_ERROR)),
        }
    };
    let status =
        StatusCode::from_u16(outcome.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let bytes = match outcome.encode() {
        Ok(bytes) => bytes,
        Err(_) => return Ok(generic(StatusCode::INTERNAL_SERVER_ERROR)),
    };
    Ok(finish(status, bytes))
}

fn finish(status: StatusCode, bytes: Vec<u8>) -> Response<Full<Bytes>> {
    response(status, Bytes::from(bytes))
}

/// Outbound signed-HTTPS transport to one owner-approved origin.
pub struct SignedHttpsClient {
    origin: String,
    authority: String,
    server_name: rustls::pki_types::ServerName<'static>,
    signer: Arc<MembershipSigner>,
    config: ClientConfig,
}

impl SignedHttpsClient {
    /// `extra_roots` are additional owner-supplied trust anchors for ordinary
    /// full WebPKI validation (for example a private CA). They never disable
    /// chain, hostname, expiry or server-auth checks.
    pub fn new(
        origin: String,
        signer: Arc<MembershipSigner>,
        extra_roots: &[Vec<u8>],
    ) -> Result<Self, Error> {
        validate_origin(&origin)?;
        let parsed = url::Url::parse(&origin).map_err(transport)?;
        let host = parsed.host_str().ok_or(Error::InvalidResponse)?.to_owned();
        let mut roots = RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        for der in extra_roots {
            roots
                .add(CertificateDer::from(der.clone()))
                .map_err(transport)?;
        }
        let config =
            ClientConfig::builder_with_provider(rustls::crypto::ring::default_provider().into())
                .with_safe_default_protocol_versions()
                .map_err(transport)?
                .with_root_certificates(roots)
                .with_no_client_auth();
        let server_name = host.clone().try_into().map_err(transport)?;
        Ok(Self {
            authority: authority(&origin)?,
            origin,
            server_name,
            signer,
            config,
        })
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }

    async fn post(&self, path: &str, body: Vec<u8>) -> Result<(StatusCode, Bytes), Error> {
        let port = url::Url::parse(&self.origin)
            .map_err(transport)?
            .port_or_known_default()
            .ok_or(Error::InvalidResponse)?;
        let host = self.server_name.to_str().into_owned();
        let tcp = TcpStream::connect((host.as_ref(), port)).await?;
        let tls = TlsConnector::from(Arc::new(self.config.clone()))
            .connect(self.server_name.clone(), tcp)
            .await
            .map_err(transport)?;
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(tls))
            .await
            .map_err(transport)?;
        tokio::spawn(async move {
            let _ = connection.await;
        });
        let request = Request::post(path)
            .header("host", &self.authority)
            .header("content-type", CONTENT_TYPE)
            .body(Full::new(Bytes::from(body)))
            .map_err(transport)?;
        let response = sender.send_request(request).await.map_err(transport)?;
        let status = response.status();
        let body = Limited::new(response.into_body(), MAX_FRAME_BYTES)
            .collect()
            .await
            .map_err(transport)?
            .to_bytes();
        Ok((status, body))
    }

    /// Challenge first, verify the server proof, then upload the exchange. Only
    /// the exact live attempt's signed response advances a caller.
    pub async fn exchange(
        &self,
        local: Identity,
        peer: &Peer,
        command: PeerCommand,
    ) -> Result<PeerReply, Error> {
        let PeerKey::SignedHttps(server_key) = &peer.key else {
            return Err(Error::Node(harmion_node::Error::IdentityMismatch));
        };
        if peer.endpoint != self.origin || peer.node == local.node {
            return Err(Error::Node(harmion_node::Error::IdentityMismatch));
        }
        let server_key = *server_key;
        timeout(DEADLINE, async move {
            let request = encode_peer_request(&PeerRequest {
                cluster: local.cluster,
                node: local.node,
                command: command.clone(),
            })?;
            let commitment = Commitment::from_request(&request)?;
            let nonce = random_nonce()?;
            let common = Common {
                cluster: local.cluster,
                sender: local.node,
                audience: peer.node,
                origin: self.origin.clone(),
            };
            let challenge_request =
                self.signer
                    .sign_challenge_request(common.clone(), nonce, commitment.clone())?;
            let (status, body) = self
                .post(CHALLENGE_PATH, challenge_request.encode()?)
                .await?;
            // A challenge-route error is signed but never a success.
            if status != StatusCode::OK {
                let frame = Frame::decode(&body).ok();
                if let Some(frame) = frame {
                    https_peer::verify_frame(&frame, &server_key)
                        .map_err(|_| Error::InvalidResponse)?;
                    if let Body::ChallengeError { error, .. } = &frame.body
                        && error.code == ErrorCode::Capacity
                    {
                        return Err(Error::Transport("signed peer capacity".into()));
                    }
                }
                return Err(Error::InvalidResponse);
            }
            let challenge = Frame::decode(&body)?;
            https_peer::verify_frame(&challenge, &server_key)
                .map_err(|_| Error::Node(harmion_node::Error::InvalidSignature))?;
            let expected = Common {
                cluster: local.cluster,
                sender: peer.node,
                audience: local.node,
                origin: self.origin.clone(),
            };
            if challenge.common != expected {
                return Err(Error::InvalidResponse);
            }
            let Body::Challenge {
                challenge_request_hash,
                client_nonce,
                generation,
                challenge_nonce,
                ttl_ms,
                commitment: echoed,
            } = &challenge.body
            else {
                return Err(Error::InvalidResponse);
            };
            if *ttl_ms == 0
                || *challenge_request_hash != challenge_request.hash()?
                || *client_nonce != nonce
                || *echoed != commitment
            {
                return Err(Error::InvalidResponse);
            }
            let context = ExchangeContext {
                challenge_request_hash: *challenge_request_hash,
                challenge_hash: challenge.hash()?,
                client_nonce: nonce,
                generation: *generation,
                challenge_nonce: *challenge_nonce,
            };
            let class = match &command {
                PeerCommand::MembershipStatus
                | PeerCommand::MembershipCursor { .. }
                | PeerCommand::MembershipPage { .. }
                | PeerCommand::MembershipAccept(_) => PayloadClass::Evidence,
                _ => PayloadClass::Data,
            };
            let exchange = self
                .signer
                .sign_exchange(common, context.clone(), class, request)?;
            let exchange_hash = exchange.hash()?;
            let (status, body) = self.post(EXCHANGE_PATH, exchange.encode()?).await?;
            let frame = Frame::decode(&body).map_err(|_| Error::InvalidResponse)?;
            https_peer::verify_frame(&frame, &server_key)
                .map_err(|_| Error::Node(harmion_node::Error::InvalidSignature))?;
            let Body::Response {
                context: bound,
                result,
            } = &frame.body
            else {
                return Err(Error::InvalidResponse);
            };
            let expected_status = match result {
                ResponseResult::Peer(_) => 200,
                ResponseResult::Error(error) => error.code.http_status(),
            };
            if bound.exchange_hash != exchange_hash
                || bound.challenge_hash != context.challenge_hash
                || bound.client_nonce != nonce
                || bound.generation != context.generation
                || bound.challenge_nonce != context.challenge_nonce
                || status.as_u16() != expected_status
            {
                return Err(Error::InvalidResponse);
            }
            match result {
                ResponseResult::Peer(bytes) => {
                    let reply = decode_peer_response(bytes)?;
                    if reply.cluster != local.cluster || reply.node != peer.node {
                        return Err(Error::InvalidResponse);
                    }
                    if let PeerReply::Rejected { update_id, reason } = reply.reply {
                        return Err(Error::RemoteRejected { update_id, reason });
                    }
                    Ok(reply.reply)
                }
                ResponseResult::Error(error) => Err(match error.code {
                    ErrorCode::NotAuthorized => Error::Node(harmion_node::Error::Unauthenticated),
                    ErrorCode::ChallengeUnavailable | ErrorCode::BindingMismatch => {
                        Error::InvalidResponse
                    }
                    _ => Error::Transport(format!("signed peer error {:?}", error.code)),
                }),
            }
        })
        .await
        .map_err(|_| Error::Timeout)?
    }
}
