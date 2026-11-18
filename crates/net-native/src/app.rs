//! Native-only local App HTTPS endpoint. Browser OAuth is not implemented.
use crate::{Error, SharedStore, SystemHost, peer, tls::TlsIdentity, transport};
use bytes::Bytes;
use harmion_domain::{ClusterId, ContentId, NodeId};
use harmion_node::{
    access::{self, Credential, Filter, Grants},
    application::{self, Host},
    membership,
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

type Body = Full<Bytes>;
const DEADLINE: Duration = Duration::from_secs(60);

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
        let (acceptor, store, identity, registration) = (
            acceptor.clone(),
            store.clone(),
            identity.clone(),
            registration.clone(),
        );
        connections.spawn(async move {
            let _permit = permit;
            let _ = timeout(DEADLINE, async {
                let tls = acceptor.accept(tcp).await.map_err(transport)?;
                let service = service_fn(move |request| {
                    respond(
                        store.clone(),
                        identity.clone(),
                        registration.clone(),
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
        });
    }
}

fn response(status: StatusCode, value: Json) -> Response<Body> {
    let bytes = serde_json::to_vec(&value).expect("JSON value is serializable");
    let (status, bytes) = if bytes.len() > MAX_APP_RESPONSE_BYTES {
        (
            StatusCode::PAYLOAD_TOO_LARGE,
            br#"{"error":"response_too_large"}"#.to_vec(),
        )
    } else {
        (status, bytes)
    };
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .header("cache-control", "no-store")
        .header("x-content-type-options", "nosniff")
        .body(Full::new(Bytes::from(bytes)))
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
        Error::Node(N::Storage(_) | N::CorruptLog | N::Entropy(_)) => {
            (StatusCode::INTERNAL_SERVER_ERROR, "internal_error")
        }
        Error::Transport(_) | Error::Io(_) | Error::Timeout | Error::RemoteRejected { .. } => {
            (StatusCode::BAD_GATEWAY, "peer_unavailable")
        }
        _ => (StatusCode::BAD_REQUEST, "invalid_request"),
    };
    response(status, json!({"error":code}))
}
async fn respond<S: Store + Send + 'static>(
    store: SharedStore<S>,
    identity: Arc<crate::NativeTransport>,
    registration: PeerRegistration,
    request: Request<Incoming>,
) -> Result<Response<Body>, Infallible> {
    if request.method() != hyper::Method::POST
        || request.uri().query().is_some()
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
    let Some(credential) = credential else {
        return Ok(failure(harmion_node::Error::Unauthenticated.into()));
    };
    let operation = request
        .uri()
        .path()
        .strip_prefix("/app/v1/")
        .unwrap_or("")
        .to_owned();
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
    let result = dispatch(
        &store,
        &identity,
        registration,
        credential,
        operation,
        body.to_vec(),
    )
    .await;
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
fn filter(spec: FilterSpec) -> Filter {
    match spec {
        FilterSpec::All => Filter::All,
        FilterSpec::Kinds { kinds } => Filter::Kinds(kinds.into_iter().collect()),
    }
}
fn grants(spec: GrantSpec) -> Grants {
    Grants {
        read: spec.read.map(filter),
        write: spec.write.map(filter),
        manage_apps: spec.manage_apps,
        manage_nodes: spec.manage_nodes,
    }
}
fn grant_spec(grants: Grants) -> GrantSpec {
    fn filter_spec(filter: Filter) -> FilterSpec {
        match filter {
            Filter::All => FilterSpec::All,
            Filter::Kinds(kinds) => FilterSpec::Kinds {
                kinds: kinds.into_iter().collect(),
            },
        }
    }
    GrantSpec {
        read: grants.read.map(filter_spec),
        write: grants.write.map(filter_spec),
        manage_apps: grants.manage_apps,
        manage_nodes: grants.manage_nodes,
    }
}
async fn dispatch<S: Store + Send + 'static>(
    store: &SharedStore<S>,
    identity: &crate::NativeTransport,
    registration: PeerRegistration,
    credential: Credential,
    operation: String,
    body: Vec<u8>,
) -> Result<Json, Error> {
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
    if matches!(
        operation.as_str(),
        "approve" | "trust-recovery-peer" | "join" | "join-recovery"
    ) {
        let info: PeerRegistration = serde_json::from_slice(&body).map_err(|_| invalid())?;
        if info.version
            != if identity.membership_signer.is_some() {
                3
            } else {
                2
            }
        {
            return Err(invalid().into());
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
                signing_key: unhex(info.signing_key.as_deref().ok_or_else(invalid)?)?,
                binding: match peer.key {
                    membership::PeerKey::TlsCertificate(bytes) => {
                        harmion_protocol::membership::Binding::Https(bytes)
                    }
                    membership::PeerKey::Iroh(key) => {
                        harmion_protocol::membership::Binding::Iroh(key)
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
    store.run(move |s| {
        let mut host = SystemHost;
        match operation.as_str() {
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
            "conflicts" => Ok(conflict_view_json(&application::get_conflicts(s, &mut host, &credential, ContentId(id(&body)?))?)),
            "inspect" => Ok(conflict_view_json(&application::inspect(s, &mut host, &credential, ContentId(id(&body)?))?)),
            "save" => Ok(save_result_json(&application::save(s, &mut host, &credential, &decode_save_request(&body)?)?)),
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
                let (app, secret) = application::issue_app(s, &mut host, &credential, grants(request.grants), request.expires_at.parse().map_err(|_| invalid())?)?;
                Ok(json!({"app_id":hex(&app.0),"credential":hex(secret.expose_bytes())}))
            },
            "change-grants" => {
                let request: ChangeGrantsRequest = serde_json::from_slice(&body).map_err(|_| invalid())?;
                application::change_grants(s, host.now_micros(), &credential, AppId(unhex(&request.app_id)?), grants(request.grants), request.revoked)?;
                Ok(json!({"changed":true}))
            },
            "identity" | "capabilities" | "permissions" if body.is_empty() => {
                let permissions = access::permissions(s, &credential, host.now_micros())?;
                Ok(match operation.as_str() {
                    "identity" => {
                        let mut registration = registration;
                        registration.cluster_root = s.transaction(|tx| Ok(tx.membership_root()?.map(|root| hex(root.bytes()))))?;
                        json!(registration)
                    },
                    "permissions" => json!(grant_spec(permissions)),
                    _ => json!({"version":1,"oauth":false,"query":false,"subscriptions":false,
                        "methods":["get","conflicts","inspect","save","identity","permissions","capabilities","members","approve","join","join-recovery","retire","confirm-retirement","dismiss-retirement","trust-recovery-peer","sync","issue-app","change-grants"]}),
                })
            },
            _ => Err(invalid()),
        }
    }).await
}
