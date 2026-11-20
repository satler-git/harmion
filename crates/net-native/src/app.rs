//! Native App API using pinned loopback HTTPS. Browser authorization has a separate listener.
use crate::{Error, SharedStore, SystemHost, peer, tls::TlsIdentity, transport};
use bytes::Bytes;
use harmion_domain::{ClusterId, ContentId, NodeId};
use harmion_node::{
    access::{self, Credential},
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

use crate::export::Body;
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
    let subscriptions = Arc::new(crate::subscriptions::Service::new());
    let export_slot = Arc::new(Semaphore::new(1));
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
        let (acceptor, store, identity, registration, subscriptions, export_slot) = (
            acceptor.clone(),
            store.clone(),
            identity.clone(),
            registration.clone(),
            subscriptions.clone(),
            export_slot.clone(),
        );
        connections.spawn(async move {
            let _permit = permit;
            let (activity, mut progress) = watch::channel(());
            let idle = async move {
                while let Ok(Ok(())) = timeout(DEADLINE, progress.changed()).await {}
            };
            let serve = async {
                let tls = acceptor.accept(tcp).await.map_err(transport)?;
                let service = service_fn(move |request| {
                    respond(
                        store.clone(),
                        identity.clone(),
                        registration.clone(),
                        subscriptions.clone(),
                        (export_slot.clone(), activity.clone()),
                        request,
                    )
                });
                hyper::server::conn::http1::Builder::new()
                    .max_headers(32)
                    .max_buf_size(16 * 1024)
                    .serve_connection(TokioIo::new(tls), service)
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
        Error::Node(N::QueryExpired) => (StatusCode::GONE, "query_expired"),
        Error::Node(N::QueryCapacity) => (StatusCode::TOO_MANY_REQUESTS, "query_capacity"),
        Error::Node(N::ExportBusy) => (StatusCode::TOO_MANY_REQUESTS, "export_busy"),
        Error::Node(N::OAuthCapacity) => (StatusCode::TOO_MANY_REQUESTS, "oauth_capacity"),
        Error::Node(N::SubscriptionCapacity) => {
            (StatusCode::TOO_MANY_REQUESTS, "subscription_capacity")
        }
        Error::Node(N::QueryItemTooLarge) => {
            (StatusCode::PAYLOAD_TOO_LARGE, "query_item_too_large")
        }
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
    subscriptions: Arc<crate::subscriptions::Service>,
    export: (Arc<Semaphore>, watch::Sender<()>),
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
    if operation == "export" {
        if !body.is_empty() {
            return Ok(failure(invalid().into()));
        }
        return Ok(
            match crate::export::response(&store, credential, export.0, export.1).await {
                Ok(response) => response,
                Err(error) => failure(error),
            },
        );
    }
    let result = dispatch(
        &store,
        &identity,
        registration,
        subscriptions,
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
async fn dispatch<S: Store + Send + 'static>(
    store: &SharedStore<S>,
    identity: &crate::NativeTransport,
    registration: PeerRegistration,
    subscriptions: Arc<crate::subscriptions::Service>,
    credential: Credential,
    operation: String,
    body: Vec<u8>,
) -> Result<Json, Error> {
    if matches!(operation.as_str(), "subscribe" | "resume" | "unsubscribe") {
        return subscriptions
            .handle(store, credential, &operation, &body)
            .await;
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
    let local_save = store.local_save.clone();
    let changes = store.changes.clone();
    let oauth = store.oauth.get().cloned();
    store.run(move |s| {
        let mut host = SystemHost;
        match operation.as_str() {
            "metrics" if body.is_empty() => Ok(json!(application::metrics(s, &credential, host.now_micros())?)),
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
            "change-grants" => {
                let request: ChangeGrantsRequest = serde_json::from_slice(&body).map_err(|_| invalid())?;
                application::change_grants(s, host.now_micros(), &credential, AppId(unhex(&request.app_id)?), request.grants.into(), request.revoked)?;
                changes.send_replace(());
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
                    "permissions" => json!(GrantSpec::from(permissions)),
                    _ => {
                        let mut methods = vec!["get","query","subscribe","resume","unsubscribe","export","conflicts","inspect","merge-input","save","identity","permissions","capabilities","members","approve","join","join-recovery","retire","confirm-retirement","dismiss-retirement","trust-recovery-peer","sync","issue-app","change-grants"];
                        methods.push("metrics");
                        if oauth.is_some() { methods.extend(["oauth-register","oauth-consent","oauth-preview"]); }
                        json!({"version":1,"oauth":oauth.is_some(),"query":true,"subscriptions":true,
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
