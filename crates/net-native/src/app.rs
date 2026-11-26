//! Native App API using pinned loopback HTTPS. Browser authorization has a separate listener.
use crate::{Error, SharedStore, SystemHost, peer, tls::TlsIdentity, transport};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use bytes::Bytes;
use harmion_domain::{ClusterId, ContentId, NodeId};
use harmion_node::{
    access::{self, Credential},
    application::{self, Host},
    membership, owner,
    storage::Store,
};
use harmion_protocol::*;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{Request, Response, StatusCode, body::Incoming, service::service_fn};
use hyper_util::rt::TokioIo;
use serde_json::{Value as Json, json};
use std::{convert::Infallible, sync::Arc, time::Duration};
use tokio::{
    net::TcpListener,
    sync::{Semaphore, watch},
    time::timeout,
};
use tokio_rustls::TlsAcceptor;

use crate::export::Body;
use crate::interpreter::{Service, switching_protocols, validate_upgrade};
const DEADLINE: Duration = Duration::from_secs(60);

struct RequestContext<S> {
    store: SharedStore<S>,
    identity: Arc<crate::NativeTransport>,
    registration: PeerRegistration,
    subscriptions: Arc<crate::subscriptions::Service>,
    interpreter: Arc<Service>,
    export_slot: Arc<Semaphore>,
}

pub async fn serve<S: Store + Send + 'static>(
    listener: TcpListener,
    store: SharedStore<S>,
    identity: Arc<TlsIdentity>,
    registration: PeerRegistration,
    stop: watch::Receiver<bool>,
) -> Result<(), Error> {
    serve_with_transport(
        listener,
        store,
        Arc::new(crate::NativeTransport {
            membership_signer: None,
            tls: identity,
            iroh: None,
            signed_https_roots: Vec::new(),
        }),
        registration,
        stop,
    )
    .await
}

pub async fn serve_with_transport<S: Store + Send + 'static>(
    listener: TcpListener,
    store: SharedStore<S>,
    identity: Arc<crate::NativeTransport>,
    registration: PeerRegistration,
    mut stop: watch::Receiver<bool>,
) -> Result<(), Error> {
    if !listener.local_addr()?.ip().is_loopback() {
        return Err(harmion_node::Error::InvalidRequest.into());
    }
    let acceptor = TlsAcceptor::from(Arc::new(identity.tls.app_server_config()?));
    let slots = Arc::new(Semaphore::new(16));
    let interpreter = store.interpreter.clone();
    let context = Arc::new(RequestContext {
        store,
        identity: identity.clone(),
        registration,
        subscriptions: Arc::new(crate::subscriptions::Service::new()),
        interpreter,
        export_slot: Arc::new(Semaphore::new(1)),
    });
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
        let acceptor = acceptor.clone();
        let context = context.clone();
        connections.spawn(async move {
            let _permit = permit;
            let (activity, mut progress) = watch::channel(());
            let idle = async move {
                while let Ok(Ok(())) = timeout(DEADLINE, progress.changed()).await {}
            };
            let serve = async {
                let tls = acceptor.accept(tcp).await.map_err(transport)?;
                let service =
                    service_fn(move |request| respond(context.clone(), activity.clone(), request));
                hyper::server::conn::http1::Builder::new()
                    .max_headers(32)
                    .max_buf_size(16 * 1024)
                    .serve_connection(TokioIo::new(tls), service)
                    .with_upgrades()
                    .await
                    .map_err(transport)
            };
            tokio::select! {
                result = serve => {
                    crate::trace_connection("App HTTPS", Ok(result));
                },
                _ = idle => tracing::debug!(service = "App HTTPS", "connection idle timeout"),
            }
        });
    }
}

/// Refuse each write before extending the encoded buffer beyond its budget.
struct BoundedJson {
    bytes: Vec<u8>,
    limit: usize,
}
impl std::io::Write for BoundedJson {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(std::io::ErrorKind::FileTooLarge.into());
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn encode_response(value: &Json, limit: usize) -> Option<Vec<u8>> {
    let mut output = BoundedJson {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer(&mut output, value).ok()?;
    Some(output.bytes)
}
fn response(status: StatusCode, value: Json) -> Response<Body> {
    let (status, bytes) = match encode_response(&value, MAX_APP_RESPONSE_BYTES) {
        Some(bytes) => (status, bytes),
        None => (
            StatusCode::PAYLOAD_TOO_LARGE,
            br#"{"error":"response_too_large"}"#.to_vec(),
        ),
    };
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .header("cache-control", "no-store")
        .header("x-content-type-options", "nosniff")
        .body(
            Full::new(Bytes::from(bytes))
                .map_err(|never| match never {})
                .boxed_unsync(),
        )
        .expect("static headers")
}
fn failure(error: Error) -> Response<Body> {
    use harmion_node::Error as N;
    let (status, code) = match error {
        Error::Node(N::Unauthenticated) => (StatusCode::UNAUTHORIZED, "unauthenticated"),
        Error::Node(N::Forbidden) => (StatusCode::FORBIDDEN, "forbidden"),
        Error::Node(N::PeerRetired) => (StatusCode::FORBIDDEN, "peer_retired"),
        Error::Node(N::MembershipConflict) => (StatusCode::CONFLICT, "membership_conflict"),
        Error::Node(N::NotFound) => (StatusCode::NOT_FOUND, "not_found"),
        Error::Node(N::RequestIdReuse) => (StatusCode::CONFLICT, "request_id_reuse"),
        Error::Node(N::TokenExpired) => (StatusCode::GONE, "token_expired"),
        Error::Node(N::QueryExpired) => (StatusCode::GONE, "query_expired"),
        Error::Node(N::QueryCapacity) => (StatusCode::TOO_MANY_REQUESTS, "query_capacity"),
        Error::Node(N::ExportBusy) => (StatusCode::TOO_MANY_REQUESTS, "export_busy"),
        Error::Node(N::OAuthCapacity) => (StatusCode::TOO_MANY_REQUESTS, "oauth_capacity"),
        Error::Node(N::SubscriptionCapacity) => {
            (StatusCode::TOO_MANY_REQUESTS, "subscription_capacity")
        }
        Error::Node(N::ResponseTooLarge) => (StatusCode::PAYLOAD_TOO_LARGE, "response_too_large"),
        Error::Node(N::QueryItemTooLarge) => {
            (StatusCode::PAYLOAD_TOO_LARGE, "query_item_too_large")
        }
        Error::Node(N::InterpreterNameConflict) => {
            (StatusCode::CONFLICT, "interpreter_name_conflict")
        }
        Error::Node(N::InterpreterNotApproved) => {
            (StatusCode::FORBIDDEN, "interpreter_not_approved")
        }
        Error::Node(N::InterpreterProviderMismatch) => {
            (StatusCode::FORBIDDEN, "interpreter_provider_mismatch")
        }
        Error::Node(N::InterpreterUnavailable) => {
            (StatusCode::SERVICE_UNAVAILABLE, "interpreter_unavailable")
        }
        Error::Node(N::InterpreterCapacity) => {
            (StatusCode::TOO_MANY_REQUESTS, "interpreter_capacity")
        }
        Error::Node(N::InterpreterCancelled) => (StatusCode::CONFLICT, "interpreter_cancelled"),
        Error::Node(N::InterpreterInputTooLarge) => {
            (StatusCode::PAYLOAD_TOO_LARGE, "interpreter_input_too_large")
        }
        Error::Node(N::InterpreterInvalidResponse) => {
            (StatusCode::BAD_GATEWAY, "interpreter_invalid_response")
        }
        Error::Node(N::InterpreterTimeout) => (StatusCode::GATEWAY_TIMEOUT, "interpreter_timeout"),
        Error::Node(N::Unsupported) => (StatusCode::NOT_IMPLEMENTED, "unsupported"),
        Error::Node(N::OwnerAssertionRequired) => {
            (StatusCode::FORBIDDEN, "owner_assertion_required")
        }
        Error::Node(N::OwnerAssertionInvalid) => (StatusCode::FORBIDDEN, "owner_assertion_invalid"),
        Error::Node(N::OwnerChallengeExpired) => (StatusCode::GONE, "owner_challenge_expired"),
        Error::Node(N::OwnerChallengeUsed) => (StatusCode::CONFLICT, "owner_challenge_used"),
        Error::Node(N::OwnerRecoveryInvalid) => (StatusCode::FORBIDDEN, "owner_recovery_invalid"),
        Error::Node(N::OwnerCapacity) => (StatusCode::TOO_MANY_REQUESTS, "owner_capacity"),
        Error::Node(N::OwnerVerifierUnavailable) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "owner_verifier_unavailable",
        ),
        Error::Node(N::ConnectionNotFound) => (StatusCode::NOT_FOUND, "connection_not_found"),
        Error::Node(N::ConnectionExpired) => (StatusCode::GONE, "connection_expired"),
        Error::Node(N::ConnectionCompleted) => (StatusCode::CONFLICT, "connection_completed"),
        Error::Node(N::ConnectionPending) => (StatusCode::CONFLICT, "connection_pending"),
        Error::Node(N::ConnectionDenied) => (StatusCode::FORBIDDEN, "connection_denied"),
        Error::Node(N::ConnectionCapacity) => {
            (StatusCode::TOO_MANY_REQUESTS, "connection_capacity")
        }
        Error::Node(N::Storage(_) | N::CorruptLog | N::Entropy(_)) => {
            (StatusCode::INTERNAL_SERVER_ERROR, "internal_error")
        }
        Error::Capacity => (StatusCode::TOO_MANY_REQUESTS, "peer_capacity"),
        Error::Transport(_) | Error::Io(_) | Error::Timeout | Error::RemoteRejected { .. } => {
            (StatusCode::BAD_GATEWAY, "peer_unavailable")
        }
        _ => (StatusCode::BAD_REQUEST, "invalid_request"),
    };
    response(status, json!({"error":code}))
}
struct OwnerHeaders {
    challenge: Option<[u8; 32]>,
    signature: Option<Vec<u8>>,
}

fn owner_headers(headers: &hyper::HeaderMap) -> Result<OwnerHeaders, harmion_node::Error> {
    let challenge = single_header(headers, harmion_protocol::owner::CHALLENGE_HEADER)?;
    let signature = single_header(headers, harmion_protocol::owner::SIGNATURE_HEADER)?;
    if challenge.is_some() != signature.is_some() {
        return Err(invalid());
    }
    let challenge = challenge.map(harmion_protocol::unhex::<32>).transpose()?;
    let signature = signature
        .map(|value| {
            URL_SAFE_NO_PAD
                .decode(value)
                .map_err(|_| harmion_node::Error::InvalidRequest)
        })
        .transpose()?;
    if signature
        .as_ref()
        .is_some_and(|value| value.len() > harmion_protocol::owner::MAX_SSH_SIGNATURE_BYTES)
    {
        return Err(invalid());
    }
    Ok(OwnerHeaders {
        challenge,
        signature,
    })
}

fn single_header<'a>(
    headers: &'a hyper::HeaderMap,
    name: &str,
) -> Result<Option<&'a str>, harmion_node::Error> {
    if headers.get_all(name).iter().count() > 1 {
        return Err(invalid());
    }
    Ok(headers.get(name).and_then(|value| value.to_str().ok()))
}

async fn require_admin<S: Store + Send + 'static>(
    store: &SharedStore<S>,
    credential: &Credential,
    now: i64,
) -> Result<(), Error> {
    let credential = credential.clone();
    let grants = store
        .run(move |store| access::permissions(store, &credential, now))
        .await?;
    if grants.is_administrator() {
        Ok(())
    } else {
        Err(harmion_node::Error::Forbidden.into())
    }
}

async fn respond<S: Store + Send + 'static>(
    context: Arc<RequestContext<S>>,
    activity: watch::Sender<()>,
    mut request: Request<Incoming>,
) -> Result<Response<Body>, Infallible> {
    if request.uri().query().is_some()
        || request.headers().get_all("host").iter().count() != 1
        || request.headers().get("host").and_then(|v| v.to_str().ok()) != Some(TLS_NAME)
        || request.headers().contains_key("origin")
    {
        return Ok(response(
            StatusCode::BAD_REQUEST,
            json!({"error":"invalid_request"}),
        ));
    }
    let credential = if request.headers().get_all("authorization").iter().count() == 1 {
        request
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .and_then(|v| unhex(v).ok())
            .map(Credential::from_bytes)
    } else {
        None
    };
    if request.method() == hyper::Method::GET && request.uri().path() == "/provider/v1/connect" {
        let Some(credential) = credential else {
            return Ok(failure(harmion_node::Error::Unauthenticated.into()));
        };
        let key = match validate_upgrade(request.headers()) {
            Ok(key) => key,
            Err(_) => {
                return Ok(response(
                    StatusCode::BAD_REQUEST,
                    json!({"error":"invalid_websocket_upgrade"}),
                ));
            }
        };
        let upgrade = hyper::upgrade::on(&mut request);
        let interpreter = context.interpreter.clone();
        let store = context.store.clone();
        tokio::spawn(async move {
            match upgrade.await {
                Ok(upgraded) => {
                    if let Err(error) = interpreter
                        .serve_provider(store, credential, upgraded)
                        .await
                    {
                        tracing::debug!(%error, "Provider connection ended");
                    }
                }
                Err(error) => tracing::debug!(%error, "Provider upgrade failed"),
            }
        });
        return Ok(switching_protocols(&key));
    }
    if request.method() != hyper::Method::POST {
        return Ok(response(
            StatusCode::BAD_REQUEST,
            json!({"error":"invalid_request"}),
        ));
    }
    let operation = request
        .uri()
        .path()
        .strip_prefix("/app/v1/")
        .unwrap_or("")
        .to_owned();
    let owner = match owner_headers(request.headers()) {
        Ok(headers) => headers,
        Err(_) => {
            return Ok(response(
                StatusCode::BAD_REQUEST,
                json!({"error":"invalid_owner_headers"}),
            ));
        }
    };
    let body = match Limited::new(request.into_body(), MAX_RECORD_BYTES)
        .collect()
        .await
    {
        Ok(body) => body.to_bytes(),
        Err(_) => {
            return Ok(response(
                StatusCode::PAYLOAD_TOO_LARGE,
                json!({"error":"invalid_body"}),
            ));
        }
    };
    // A connection offer starts without an app credential. The one-time
    // request secret authorizes status and completion; owner approval is a
    // separate management operation.
    if matches!(
        operation.as_str(),
        "connect-start" | "connect-status" | "connect-complete"
    ) {
        return Ok(
            match crate::connect::handle_public(&context.store, &operation, body.to_vec()).await {
                Ok(value) => response(StatusCode::OK, value),
                Err(error) => failure(error),
            },
        );
    }
    let Some(credential) = credential else {
        return Ok(failure(harmion_node::Error::Unauthenticated.into()));
    };
    if harmion_protocol::owner::requires_assertion(&operation) {
        let store = context.store.clone();
        let owner_credential = credential.clone();
        let owner_operation = operation.clone();
        let owner_body = body.to_vec();
        let now = SystemHost.now_micros();
        if let Err(error) = store
            .run(move |store| {
                store.transaction(|tx| {
                    owner::authorize(
                        tx,
                        &owner_credential,
                        &owner_operation,
                        &owner_body,
                        owner.challenge,
                        owner.signature.as_deref(),
                        "app",
                        now,
                    )
                })
            })
            .await
        {
            return Ok(failure(error));
        }
    }
    if operation == "export" {
        if !body.is_empty() {
            return Ok(failure(invalid().into()));
        }
        return Ok(
            match crate::export::response(
                &context.store,
                credential,
                context.export_slot.clone(),
                activity,
            )
            .await
            {
                Ok(response) => response,
                Err(error) => failure(error),
            },
        );
    }
    let result = dispatch(&context, credential, operation, body.to_vec()).await;
    Ok(match result {
        Ok(value) => response(StatusCode::OK, value),
        Err(error) => failure(error),
    })
}

fn invalid() -> harmion_node::Error {
    harmion_node::Error::InvalidRequest
}
fn id(bytes: &[u8]) -> Result<[u8; 32], harmion_node::Error> {
    bytes.try_into().map_err(|_| invalid())
}
fn member_json(member: &harmion_protocol::membership::Member) -> Json {
    let binding = match &member.binding {
        harmion_protocol::membership::Binding::Https(bytes) => {
            json!({"transport":"https","certificate":hex(bytes)})
        }
        harmion_protocol::membership::Binding::Iroh(key) => {
            json!({"transport":"iroh","endpoint_id":hex(key)})
        }
        harmion_protocol::membership::Binding::SignedHttps(key) => {
            json!({"transport":"signed_https","key":hex(key)})
        }
    };
    json!({
        "node": hex(&member.node.0),
        "signing_key": hex(&member.signing_key),
        "binding": binding,
        "endpoint": member.endpoint,
    })
}
fn manifest_json(manifest: &harmion_node::provisioning::ExportManifest) -> Json {
    json!({
        "own": member_json(&manifest.own),
        "root": hex(manifest.root.bytes()),
        "evidence": manifest.evidence.iter().map(|certificate| hex(certificate.bytes())).collect::<Vec<_>>(),
        "contacts": manifest.contacts.iter().map(member_json).collect::<Vec<_>>(),
    })
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnerEnrollRequest {
    public_key: String,
    #[serde(default)]
    label: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnerRevokeRequest {
    credential: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnerRecoverRequest {
    recovery_code: String,
    public_key: String,
    #[serde(default)]
    label: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnerEnforceRequest {
    enforce: bool,
}

async fn owner_operation<S: Store + Send + 'static>(
    store: &SharedStore<S>,
    credential: Credential,
    operation: &str,
    body: Vec<u8>,
) -> Result<Json, Error> {
    match operation {
        "owner-status" => {
            if !body.is_empty() {
                return Err(invalid().into());
            }
            require_admin(store, &credential, SystemHost.now_micros()).await?;
            let status = store
                .run(|store| store.transaction(|tx| owner::status(tx)))
                .await?;
            Ok(json!(status))
        }
        "owner-challenge" => {
            let request: harmion_protocol::owner::ChallengeRequest =
                serde_json::from_slice(&body).map_err(|_| invalid())?;
            let id = SystemHost.random_bytes()?;
            let now = SystemHost.now_micros();
            let challenge = store
                .run(move |store| {
                    store.transaction(|tx| owner::challenge(tx, &credential, request, id, now))
                })
                .await?;
            Ok(json!(challenge))
        }
        "owner-enroll" => {
            let request: OwnerEnrollRequest =
                serde_json::from_slice(&body).map_err(|_| invalid())?;
            let id = SystemHost.random_bytes()?;
            let now = SystemHost.now_micros();
            let credential = store
                .run(move |store| {
                    store.transaction(|tx| {
                        owner::enroll(tx, &request.public_key, &request.label, id, "owner", now)
                    })
                })
                .await?;
            Ok(json!({
                "credential":hex(&credential.id),
                "fingerprint":credential.fingerprint,
                "label":credential.label,
            }))
        }
        "owner-revoke" => {
            let request: OwnerRevokeRequest =
                serde_json::from_slice(&body).map_err(|_| invalid())?;
            let id = harmion_protocol::unhex::<32>(&request.credential)?;
            let now = SystemHost.now_micros();
            store
                .run(move |store| store.transaction(|tx| owner::revoke(tx, id, "owner", now)))
                .await?;
            Ok(json!({"revoked":true}))
        }
        "owner-recover" => {
            let request: OwnerRecoverRequest =
                serde_json::from_slice(&body).map_err(|_| invalid())?;
            require_admin(store, &credential, SystemHost.now_micros()).await?;
            let recovery_code = harmion_protocol::unhex::<32>(&request.recovery_code)?;
            let new_recovery_code = SystemHost.random_bytes()?;
            let id = SystemHost.random_bytes()?;
            let now = SystemHost.now_micros();
            let credential = store
                .run(move |store| {
                    store.transaction(|tx| {
                        owner::recover(
                            tx,
                            &recovery_code,
                            &request.public_key,
                            &request.label,
                            &new_recovery_code,
                            id,
                            now,
                        )
                    })
                })
                .await?;
            Ok(json!({
                "credential":hex(&credential.id),
                "fingerprint":credential.fingerprint,
                "label":credential.label,
                "recovery_code":hex(&new_recovery_code),
            }))
        }
        "owner-enforce" => {
            let request: OwnerEnforceRequest =
                serde_json::from_slice(&body).map_err(|_| invalid())?;
            let now = SystemHost.now_micros();
            store
                .run(move |store| {
                    store.transaction(|tx| owner::set_enforce(tx, request.enforce, "owner", now))
                })
                .await?;
            Ok(json!({"enforce":request.enforce}))
        }
        "owner-rotate-recovery" => {
            if !body.is_empty() {
                return Err(invalid().into());
            }
            let new_recovery_code = SystemHost.random_bytes()?;
            let now = SystemHost.now_micros();
            store
                .run(move |store| {
                    store.transaction(|tx| {
                        owner::rotate_recovery(tx, &new_recovery_code, "owner", now)
                    })
                })
                .await?;
            Ok(json!({"recovery_code":hex(&new_recovery_code)}))
        }
        _ => Err(harmion_node::Error::NotFound.into()),
    }
}

async fn dispatch<S: Store + Send + 'static>(
    context: &RequestContext<S>,
    credential: Credential,
    operation: String,
    body: Vec<u8>,
) -> Result<Json, Error> {
    let store = &context.store;
    let identity = context.identity.as_ref();
    let subscriptions = context.subscriptions.clone();
    let interpreter = &context.interpreter;
    let registration = context.registration.clone();
    if operation.starts_with("owner-") {
        return owner_operation(store, credential, &operation, body).await;
    }
    if matches!(operation.as_str(), "connect-list" | "connect-approve") {
        return crate::connect::handle_admin(store, credential, &operation, body).await;
    }
    if operation == "interpreter-ticket" {
        return crate::connect::provider_ticket(store, credential, body).await;
    }
    if matches!(
        operation.as_str(),
        "interpreter-list"
            | "interpreter-describe"
            | "interpreter-register"
            | "interpreter-approve"
            | "interpreter-call"
            | "interpreter-call-stream"
            | "interpreter-cancel"
    ) {
        return interpreter
            .handle(store, credential, &operation, body)
            .await;
    }
    if matches!(operation.as_str(), "subscribe" | "resume" | "unsubscribe") {
        return subscriptions
            .handle(store, credential, &operation, &body)
            .await;
    }
    if operation == "change-grants" {
        use harmion_node::access::Grants;
        let request: ChangeGrantsRequest = serde_json::from_slice(&body).map_err(|_| invalid())?;
        let app = AppId(unhex(&request.app_id)?);
        let grants = Grants::from(request.grants.clone());
        let revoked = request.revoked;
        let expires_at = request
            .expires_at
            .as_deref()
            .map(str::parse::<i64>)
            .transpose()
            .map_err(|_| invalid())?;
        let grants_for_db = grants.clone();
        store
            .run(move |s| {
                application::change_grants(
                    s,
                    SystemHost.now_micros(),
                    &credential,
                    app,
                    grants_for_db,
                    revoked,
                    expires_at,
                )
            })
            .await?;
        store.notify_app_changed(app, &grants, revoked).await?;
        store.changes.send_replace(());
        return Ok(json!({"changed":true}));
    }
    if operation == "sync" {
        let node = NodeId(id(&body)?);
        let allowed = store
            .run(move |s| access::permissions(s, &credential, SystemHost.now_micros()))
            .await?;
        if !allowed.manage_nodes {
            return Err(harmion_node::Error::Forbidden.into());
        }
        let report = crate::sync::sync_once(store, identity, node).await?;
        return Ok(
            json!({"uploaded":report.uploaded.to_string(),"downloaded":report.downloaded.to_string(),"local_high":report.local_high.to_string(),"remote_high":report.remote_high.to_string()}),
        );
    }
    if operation == "provision-manifest" {
        let info: PeerRegistration = serde_json::from_slice(&body).map_err(|_| invalid())?;
        if info.version != 4 {
            return Err(invalid().into());
        }
        let PeerBinding::SignedHttps { origin } = info.binding else {
            return Err(invalid().into());
        };
        let signing_key: [u8; 32] = unhex(info.signing_key.as_deref().ok_or_else(invalid)?)?;
        harmion_node::identity::validate_signing_key(&signing_key).map_err(|_| invalid())?;
        let member = harmion_protocol::membership::Member {
            node: NodeId(unhex(&info.node_id)?),
            signing_key,
            binding: harmion_protocol::membership::Binding::SignedHttps(signing_key),
            endpoint: origin,
        };
        let cluster = ClusterId(unhex(&info.cluster_id)?);
        let manifest = store
            .run(move |s| {
                harmion_node::provisioning::export_manifest(
                    s,
                    &credential,
                    SystemHost.now_micros(),
                    cluster,
                    member,
                )
            })
            .await?;
        return Ok(manifest_json(&manifest));
    }
    if matches!(
        operation.as_str(),
        "approve" | "trust-recovery-peer" | "join" | "join-recovery"
    ) {
        let info: PeerRegistration = serde_json::from_slice(&body).map_err(|_| invalid())?;
        let has_signer = identity.membership_signer.is_some();
        let supported = match &info.binding {
            // Registration v4 is the explicit signed-HTTPS binding form; it
            // requires a membership signer and the top-level node signing key.
            PeerBinding::SignedHttps { .. } => {
                info.version == 4 && has_signer && info.signing_key.is_some()
            }
            _ => info.version == if has_signer { 3 } else { 2 },
        };
        if !supported {
            return Err(invalid().into());
        }
        let signing_key: Option<[u8; 32]> =
            info.signing_key.as_deref().map(unhex::<32>).transpose()?;
        if let Some(key) = signing_key {
            // A member signing key that is not a usable Ed25519 point could never
            // authenticate; fail the registration instead of storing it.
            harmion_node::identity::validate_signing_key(&key).map_err(|_| invalid())?;
        }
        let (key, endpoint) = match info.binding {
            PeerBinding::Https {
                certificate,
                endpoint,
            } => (
                membership::PeerKey::TlsCertificate(unhex_bytes(
                    &certificate,
                    membership::MAX_CERTIFICATE_BYTES,
                )?),
                endpoint,
            ),
            PeerBinding::Iroh {
                endpoint_id,
                relay_url,
            } => (
                membership::PeerKey::Iroh(unhex(&endpoint_id)?),
                relay_url.unwrap_or_default(),
            ),
            PeerBinding::SignedHttps { origin } => (
                membership::PeerKey::SignedHttps(signing_key.ok_or_else(invalid)?),
                origin,
            ),
        };
        let peer = membership::Peer {
            node: NodeId(unhex(&info.node_id)?),
            key,
            endpoint,
            retired: false,
        };
        if let Some(signer) = identity.membership_signer.clone() {
            peer::validate_binding(&peer)?;
            let member = harmion_protocol::membership::Member {
                node: peer.node,
                signing_key: signing_key.ok_or_else(invalid)?,
                binding: match peer.key {
                    membership::PeerKey::TlsCertificate(bytes) => {
                        harmion_protocol::membership::Binding::Https(bytes)
                    }
                    membership::PeerKey::Iroh(key) => {
                        harmion_protocol::membership::Binding::Iroh(key)
                    }
                    membership::PeerKey::SignedHttps(key) => {
                        harmion_protocol::membership::Binding::SignedHttps(key)
                    }
                },
                endpoint: peer.endpoint,
            };
            let cluster = ClusterId(unhex(&info.cluster_id)?);
            if operation == "join" || operation == "join-recovery" {
                let root = harmion_protocol::membership::Certificate::from_bytes(&unhex_bytes(
                    info.cluster_root.as_deref().ok_or_else(invalid)?,
                    harmion_protocol::membership::MAX_CERTIFICATE_BYTES,
                )?)?;
                let root_hex = hex(root.bytes());
                let recovery = operation == "join-recovery";
                store
                    .run(move |s| {
                        harmion_node::membership_state::join(
                            s,
                            SystemHost.now_micros(),
                            &credential,
                            cluster,
                            member,
                            root,
                            recovery,
                        )
                    })
                    .await?;
                let mut registration = registration;
                registration.cluster_root = Some(root_hex);
                return Ok(json!(registration));
            }
            let recovery = operation == "trust-recovery-peer";
            store
                .run(move |s| {
                    if recovery {
                        harmion_node::membership_state::trust_recovery_peer(
                            s,
                            SystemHost.now_micros(),
                            &credential,
                            &signer,
                            cluster,
                            member,
                            SystemHost.random_bytes()?,
                        )
                    } else {
                        harmion_node::membership_state::approve(
                            s,
                            SystemHost.now_micros(),
                            &credential,
                            &signer,
                            cluster,
                            member,
                            SystemHost.random_bytes()?,
                        )
                    }
                })
                .await?;
            return Ok(if recovery {
                json!({"trusted_locally":true,"requires_owner_verification_on_each_affected_node":true})
            } else {
                json!({"approved":true})
            });
        }
        if operation != "approve" {
            return Err(invalid().into());
        }
        peer::approve(
            store,
            credential,
            SystemHost.now_micros(),
            ClusterId(unhex(&info.cluster_id)?),
            peer,
        )
        .await?;
        return Ok(json!({"approved":true}));
    }
    let signer = identity.membership_signer.clone();
    let local_save = store.local_save.clone();
    let changes = store.changes.clone();
    let oauth = store.oauth.get().cloned();
    store.run(move |s| {
        let mut host = SystemHost;
        match operation.as_str() {
            "metrics" if body.is_empty() => Ok(json!(application::metrics(s, &credential, host.now_micros())?)),
            "sync-status" => {
                let update = if body.is_empty() { None } else { Some(harmion_domain::UpdateId(id(&body)?)) };
                Ok(json!(harmion_node::sync_status::get(s, &credential, host.now_micros(), update)?))
            },
            "members" if body.is_empty() => {
                let now = host.now_micros();
                {
                    let state = harmion_node::membership_state::inspect(s, &credential, now)?;
                    let nodes: Vec<_> = state.nodes.into_iter().map(|(node,status)| {
                        let state = match status {
                            harmion_node::membership_state::Status::Active(_) => "active",
                            harmion_node::membership_state::Status::Retired => "retired",
                            harmion_node::membership_state::Status::Conflict => "conflict",
                            harmion_node::membership_state::Status::Unconfirmed => "unconfirmed",
                        };
                        json!({"node_id":hex(&node.0),"state":state})
                    }).collect();
                    let pending: Vec<_> = state.pending_retirements.iter().map(|(id,(issuer,target))| json!({"certificate_id":hex(id),"issuer":hex(&issuer.0),"target":hex(&target.0)})).collect();
                    Ok(json!({"members":nodes,"confirmed_certificates":state.confirmed.len(),"pending_retirements":pending}))
                }
            },
            "get" => Ok(read_view_json(&application::get(s, &mut host, &credential, ContentId(id(&body)?))?)),
            "query" => {
                use harmion_protocol::query::QueryRequest;
                let page = match harmion_protocol::query::decode_request(&body)? {
                    QueryRequest::Start { condition, limit } => harmion_node::query::start(s, &mut host, &credential, &condition, limit)?,
                    QueryRequest::Next { cursor, limit } => harmion_node::query::next(s, &mut host, &credential, cursor, limit)?,
                };
                let items: Vec<_> = page.items.iter().map(read_view_json).collect();
                Ok(json!({"items":items,"next":page.next.map(|cursor| hex(&cursor.0))}))
            },
            "apps" => {
                let apps = application::list_apps(s, &credential, host.now_micros())?;
                let mut output = Vec::with_capacity(apps.len());
                for app in apps {
                    let client = s.transaction(|tx| tx.oauth_client(app.id))?;
                    output.push(json!({
                        "app_id": hex(&app.id.0),
                        "expires_at": app.expires_at.to_string(),
                        "revoked": app.revoked,
                        "grants": harmion_protocol::GrantSpec::from(app.grants.clone()),
                        "name": client.as_ref().map(|c| c.name.clone()),
                        "redirect_uri": client.as_ref().map(|c| c.redirect.clone()),
                        "client_id": client.as_ref().map(|c| hex(&c.id.0)),
                    }));
                }
                Ok(json!({"apps": output}))
            }
            "conflicts" => Ok(conflict_view_json(&application::get_conflicts(s, &mut host, &credential, ContentId(id(&body)?))?)),
            "inspect" => Ok(conflict_view_json(&application::inspect(s, &mut host, &credential, ContentId(id(&body)?))?)),
            "merge-input" => {
                let request = harmion_protocol::resolution::MergeInputRequest::decode(&body)?;
                let input = application::merge_input(s, &credential, host.now_micros(), request)?;
                Ok(json!(input))
            },
            "save" => {
                let request = decode_save_request(&body)?;
                let (result, inserted) = application::save_with_outcome(s, &mut host, &credential, &request)?;
                if inserted {
                    // The DB worker can finish a commit after the HTTP task is cancelled.
                    local_save.notify_one();
                    changes.send_replace(());
                }
                Ok(save_result_json(&result))
            },
            "retire" => {
                let node = NodeId(id(&body)?);
                if let Some(signer) = signer {
                    harmion_node::membership_state::retire(s, host.now_micros(), &credential, &signer, node, host.random_bytes()?)?;
                } else { membership::retire(s, host.now_micros(), &credential, node)?; }
                Ok(json!({"retired":true}))
            },
            "dismiss-retirement" | "confirm-retirement" => {
                let signer = signer.ok_or_else(invalid)?;
                let confirm = operation == "confirm-retirement";
                harmion_node::membership_state::resolve_retirement(s, host.now_micros(), &credential, &signer, id(&body)?, confirm, host.random_bytes()?)?;
                Ok(json!({"resolved":true,"retired":confirm}))
            },
            "issue-app" => {
                let request: IssueAppRequest = serde_json::from_slice(&body).map_err(|_| invalid())?;
                let (app, secret) = application::issue_app(s, &mut host, &credential, request.grants.into(), request.expires_at.parse().map_err(|_| invalid())?)?;
                Ok(json!({"app_id":hex(&app.0),"credential":hex(secret.expose_bytes())}))
            },
            "oauth-register" => {
                let oauth = oauth.ok_or_else(invalid)?;
                let registration: OAuthRegistration = serde_json::from_slice(&body).map_err(|_| invalid())?;
                let client = harmion_node::oauth::register(s, &mut host, &credential, registration.name, registration.redirect_uri)?;
                Ok(json!({"client_id":hex(&client.id.0), "redirect_uri":client.redirect,
                    "authorization_endpoint":format!("{}/oauth/authorize", oauth.origin()),
                    "token_endpoint":format!("{}/oauth/token", oauth.origin())}))
            },
            "oauth-consent" => {
                oauth.ok_or_else(invalid)?.consent_link(s, credential, id(&body)?)
            },
            "oauth-preview" => {
                oauth.ok_or_else(invalid)?;
                let (client, pending) = harmion_node::oauth::pending(s, &credential, host.now_micros(), id(&body)?)?;
                Ok(json!({"client_id":hex(&client.id.0),"name":client.name,
                    "redirect_uri":pending.redirect,"grants":GrantSpec::from(pending.grants),
                    "expires_at":pending.expires_at.to_string()}))
            },
            "identity" | "capabilities" | "permissions" if body.is_empty() => {
                let permissions = access::permissions(s, &credential, host.now_micros())?;
                Ok(match operation.as_str() {
                    "identity" => {
                        let mut registration = registration;
                        registration.cluster_root = s.transaction(|tx| Ok(tx.membership_root()?.map(|root| hex(root.bytes()))))?;
                        json!(registration)
                    },
                    "permissions" => json!(GrantSpec::from(permissions)),
                    _ => {
                        let mut methods = vec!["get","query","apps","subscribe","resume","unsubscribe","export","conflicts","inspect","merge-input","save","identity","permissions","capabilities","members","approve","provision-manifest","join","join-recovery","retire","confirm-retirement","dismiss-retirement","trust-recovery-peer","sync","issue-app","change-grants","interpreter-list","interpreter-describe","interpreter-register","interpreter-approve","interpreter-call","interpreter-call-stream","interpreter-cancel"];
                        methods.extend(["metrics", "sync-status", "owner-status", "owner-challenge", "owner-enroll", "owner-revoke", "owner-recover", "owner-rotate-recovery", "owner-enforce"]);
                        if oauth.is_some() { methods.extend(["oauth-register","oauth-consent","oauth-preview"]); }
                        json!({"version":1,"oauth":oauth.is_some(),"query":true,"subscriptions":true,
                        "interpreter":true,
                        "interpreter_contract_version":harmion_protocol::interpreter::CONTRACT_VERSION,
                        "interpreter_stream_input_version":harmion_protocol::interpreter::STREAM_INPUT_VERSION,
                        "interpreter_max_stream_input_bytes":harmion_protocol::interpreter::MAX_STREAM_INPUT_BYTES,
                        "interpreter_stream_timeout_ms":harmion_protocol::interpreter::INPUT_STREAM_TIMEOUT_MS,
                        "interpreter_max_active_streams":harmion_protocol::interpreter::MAX_ACTIVE_INPUT_STREAMS,
                        "interpreter_max_active":harmion_protocol::interpreter::MAX_ACTIVE_INVOCATIONS,
                        "interpreter_max_active_per_app":harmion_protocol::interpreter::MAX_ACTIVE_INVOCATIONS_PER_APP,
                        "interpreter_default_timeout_ms":harmion_protocol::interpreter::DEFAULT_CALL_TIMEOUT_MS,
                        "interpreter_max_timeout_ms":harmion_protocol::interpreter::MAX_CALL_TIMEOUT_MS,
                        "oauth_origin":oauth.as_ref().map(|service| service.origin()),
                        "query_max_page_items":harmion_protocol::query::MAX_QUERY_PAGE_ITEMS,
                        "query_ttl_seconds":harmion_node::query::QUERY_TTL_MICROS / 1_000_000,
                        "query_max_active":harmion_node::query::MAX_ACTIVE_QUERIES,
                        "query_max_version_refs":harmion_node::query::MAX_QUERY_VERSION_REFS,
                        "subscription_max_active":harmion_node::subscription::MAX_SUBSCRIPTIONS,
                        "subscription_max_visible":harmion_node::subscription::MAX_VISIBLE_CONTENTS,
                        "subscription_retained_changes":harmion_node::storage::MAX_RETAINED_CHANGES,
                        "subscription_idle_seconds":harmion_node::subscription::SUBSCRIPTION_IDLE_MICROS / 1_000_000,
                        "methods":methods})
                    },
                })
            },
            _ => Err(invalid()),
        }
    }).await
}

#[cfg(test)]
mod response_tests {
    use super::*;
    use std::io::Write as _;

    #[test]
    fn bounded_encoding_preserves_json_and_exact_boundary() {
        let value = json!({"body": "quoted\"\\\n\u{1}日本語", "nested": [null, true, 1]});
        let bytes = serde_json::to_vec(&value).unwrap();
        assert_eq!(encode_response(&value, bytes.len()), Some(bytes.clone()));
        assert!(encode_response(&value, bytes.len() - 1).is_none());
        assert!(encode_response(&value, 0).is_none());
    }

    #[tokio::test]
    async fn preflight_error_preserves_the_existing_http_error_contract() {
        let rejected = failure(harmion_node::Error::ResponseTooLarge.into());
        assert_eq!(rejected.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let bytes = rejected.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(bytes.as_ref(), br#"{"error":"response_too_large"}"#);
        let oversized = response(StatusCode::OK, json!("a".repeat(MAX_APP_RESPONSE_BYTES)));
        assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let bytes = oversized.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(bytes.as_ref(), br#"{"error":"response_too_large"}"#);
    }

    #[tokio::test]
    async fn capacity_is_reported_as_a_retryable_peer_error() {
        let response = failure(Error::Capacity);
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(bytes.as_ref(), br#"{"error":"peer_capacity"}"#);
    }

    #[test]
    fn oversized_write_does_not_extend_the_encoded_buffer() {
        let mut output = BoundedJson {
            bytes: Vec::new(),
            limit: 5,
        };
        output.write_all(b"abc").unwrap();
        assert!(output.write_all(b"def").is_err());
        assert_eq!(output.bytes, b"abc");
        output.write_all(b"de").unwrap();
        assert_eq!(output.bytes, b"abcde");
        assert!(output.write_all(b"f").is_err());
        assert_eq!(output.bytes.len(), output.limit);
    }
}
