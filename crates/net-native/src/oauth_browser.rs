//! Browser authorization on the separately trusted loopback HTTPS origin.

use crate::{Error, SharedStore, SystemHost, tls::BrowserTls, transport};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use bytes::Bytes;
use harmion_node::{
    access::{Credential, Filter, Grants},
    application::Host,
    oauth,
    storage::Store,
};
use harmion_protocol::{AppId, GrantSpec, hex, unhex};
use http_body_util::{BodyExt, Full, Limited};
use hyper::{Method, Request, Response, StatusCode, body::Incoming, service::service_fn};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    convert::Infallible,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    net::TcpListener,
    sync::{Semaphore, watch},
    task::JoinSet,
    time::timeout,
};
use tokio_rustls::TlsAcceptor;
use url::{Url, form_urlencoded};

type Body = Full<Bytes>;
const MAX_FORM: usize = 16 * 1024;
const TICKET_TTL: i64 = 120_000_000;

pub(crate) struct Service {
    origin: String,
    tickets: Mutex<BTreeMap<[u8; 32], Ticket>>,
    provider_tickets: Mutex<BTreeMap<[u8; 32], ProviderTicket>>,
}

#[derive(Clone)]
pub(crate) struct ProviderTicket {
    pub app: AppId,
    pub provider: harmion_protocol::interpreter::ProviderId,
    pub expires_at: i64,
}

#[derive(Clone)]
struct Ticket {
    owner: Credential,
    request: [u8; 32],
    expires_at: i64,
}

pub fn configure<S: Store + Send + 'static>(
    store: &SharedStore<S>,
    endpoint: SocketAddr,
) -> Result<String, Error> {
    if endpoint.ip() != std::net::Ipv4Addr::LOCALHOST || endpoint.port() == 0 {
        return Err(invalid().into());
    }
    let origin = Url::parse(&format!("https://{endpoint}"))
        .map_err(transport)?
        .origin()
        .ascii_serialization();
    store
        .oauth
        .set(Arc::new(Service {
            origin: origin.clone(),
            tickets: Mutex::new(BTreeMap::new()),
            provider_tickets: Mutex::new(BTreeMap::new()),
        }))
        .map_err(|_| invalid())?;
    Ok(origin)
}

impl Service {
    pub(crate) fn origin(&self) -> &str {
        &self.origin
    }

    fn tickets(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, BTreeMap<[u8; 32], Ticket>>, harmion_node::Error> {
        self.tickets
            .lock()
            .map_err(|_| harmion_node::Error::Storage("consent tickets poisoned".into()))
    }

    pub(crate) fn consent_link(
        &self,
        db: &mut impl Store,
        owner: Credential,
        request: [u8; 32],
    ) -> Result<Value, harmion_node::Error> {
        let now = SystemHost.now_micros();
        let (_, pending) = oauth::pending(db, &owner, now, request)?;
        let key = Credential::from_bytes(SystemHost.random_bytes()?);
        let mut tickets = self.tickets()?;
        tickets.retain(|_, ticket| ticket.expires_at > now);
        if tickets.len() >= oauth::MAX_PENDING {
            return Err(harmion_node::Error::OAuthCapacity);
        }
        if tickets.contains_key(&key.verifier()) {
            return Err(harmion_node::Error::Entropy(
                "consent ticket collision".into(),
            ));
        }
        tickets.insert(
            key.verifier(),
            Ticket {
                owner,
                request,
                expires_at: pending.expires_at.min(now.saturating_add(TICKET_TTL)),
            },
        );
        Ok(json!({"url":format!("{}/oauth/consent#{}", self.origin, hex(key.expose_bytes()))}))
    }

    fn ticket(&self, encoded: &str, consume: bool) -> Result<Ticket, harmion_node::Error> {
        let key = Credential::from_bytes(unhex(encoded)?).verifier();
        let mut tickets = self.tickets()?;
        let ticket = if consume {
            tickets.remove(&key)
        } else {
            tickets.get(&key).cloned()
        };
        ticket
            .filter(|ticket| ticket.expires_at > SystemHost.now_micros())
            .ok_or(harmion_node::Error::Forbidden)
    }
}

pub async fn serve<S: Store + Send + 'static>(
    listener: TcpListener,
    store: SharedStore<S>,
    tls: BrowserTls,
    mut stop: watch::Receiver<bool>,
) -> Result<(), Error> {
    let state = store.oauth.get().cloned().ok_or_else(invalid)?;
    let actual = Url::parse(&format!("https://{}", listener.local_addr()?))
        .map_err(transport)?
        .origin()
        .ascii_serialization();
    if state.origin != actual {
        return Err(invalid().into());
    }
    let acceptor = TlsAcceptor::from(Arc::new(tls.server_config()?));
    let slots = Arc::new(Semaphore::new(16));
    let mut tasks = JoinSet::new();
    loop {
        let tcp = tokio::select! {
            result = stop.changed() => {
                if result.is_err() || *stop.borrow() {
                    tasks.shutdown().await;
                    return Ok(());
                }
                continue;
            },
            Some(_) = tasks.join_next(), if !tasks.is_empty() => continue,
            incoming = listener.accept() => incoming?.0,
        };
        let Ok(permit) = slots.clone().try_acquire_owned() else {
            continue;
        };
        let (acceptor, store, state) = (acceptor.clone(), store.clone(), state.clone());
        tasks.spawn(async move {
            let _permit = permit;
            let result = timeout(Duration::from_secs(30), async {
                let stream = acceptor.accept(tcp).await.map_err(transport)?;
                hyper::server::conn::http1::Builder::new()
                    .max_headers(32)
                    .max_buf_size(24 * 1024)
                    .serve_connection(
                        TokioIo::new(stream),
                        service_fn(move |request| handle(store.clone(), state.clone(), request)),
                    )
                    .with_upgrades()
                    .await
                    .map_err(transport)
            })
            .await;
            crate::trace_connection("OAuth HTTPS", result);
        });
    }
}

async fn handle<S: Store + Send + 'static>(
    store: SharedStore<S>,
    state: Arc<Service>,
    mut request: Request<Incoming>,
) -> Result<Response<Body>, Infallible> {
    if provided(&request, "host") != state.origin.strip_prefix("https://") {
        return Ok(error(StatusCode::BAD_REQUEST, "invalid_request"));
    }
    let path = request.uri().path().to_owned();
    if path == "/connect" && request.method() == Method::GET {
        return Ok(connect_page());
    }
    if path == "/provider/connect" && request.method() == Method::GET {
        return Ok(match provider_connect(store, state, &mut request).await {
            Ok(response) => response,
            Err(error) => connect_failure(error),
        });
    }
    if path.starts_with("/connect/") {
        return Ok(match connect_dispatch(store, request).await {
            Ok(response) => response,
            Err(error) => connect_failure(error),
        });
    }
    Ok(match dispatch(store, &state, request).await {
        Ok(response) => response,
        Err(Error::Node(harmion_node::Error::OAuthInvalidGrant)) => {
            error(StatusCode::BAD_REQUEST, "invalid_grant")
        }
        Err(Error::Node(harmion_node::Error::Forbidden | harmion_node::Error::Unauthenticated)) => {
            error(StatusCode::FORBIDDEN, "access_denied")
        }
        Err(Error::Node(harmion_node::Error::OAuthCapacity)) => {
            error(StatusCode::TOO_MANY_REQUESTS, "temporarily_unavailable")
        }
        Err(Error::Node(harmion_node::Error::InterpreterNameConflict)) => json_response(
            StatusCode::CONFLICT,
            json!({"error":"interpreter_name_conflict"}),
        ),
        Err(Error::Node(
            harmion_node::Error::Storage(_)
            | harmion_node::Error::CorruptLog
            | harmion_node::Error::Entropy(_),
        )) => error(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
        Err(_) => error(StatusCode::BAD_REQUEST, "invalid_request"),
    })
}

fn provided<'a>(request: &'a Request<Incoming>, name: &str) -> Option<&'a str> {
    if request.headers().get_all(name).iter().count() != 1 {
        return None;
    }
    request.headers().get(name)?.to_str().ok()
}

async fn dispatch<S: Store + Send + 'static>(
    store: SharedStore<S>,
    state: &Service,
    request: Request<Incoming>,
) -> Result<Response<Body>, Error> {
    if provided(&request, "host") != state.origin.strip_prefix("https://") {
        return Err(invalid().into());
    }
    let path = request.uri().path().to_owned();
    let query = request.uri().query();
    match (request.method(), path.as_str()) {
        (&Method::GET, "/oauth/authorize") => {
            let mut form = Form::parse(query.unwrap_or("").as_bytes())?;
            form.require("response_type", "code")?;
            form.require("code_challenge_method", "S256")?;
            let client = AppId(unhex(&form.take("client_id")?)?);
            let redirect = form.take("redirect_uri")?;
            let challenge = form.take("code_challenge")?;
            let state_value = form.take("state")?;
            let grants = parse_scope(&form.take("scope")?)?;
            form.finish()?;
            let pending = store
                .run(move |db| {
                    oauth::start(
                        db,
                        &mut SystemHost,
                        oauth::Start {
                            client,
                            redirect,
                            challenge,
                            state: state_value,
                            grants,
                        },
                    )
                })
                .await?;
            let page = format!(
                "<!doctype html><meta charset=utf-8><title>Harmion authorization</title>\
                <h1>Harmion authorization</h1><p>Paste this request into the Apps page of Harmion Admin.</p><pre>harmion-auth:v1:{}</pre><p>You can also use the owner CLI.</p>\
                <pre>harmion --node NODE_DIRECTORY oauth-consent {}</pre>\
                <p>Open the consent link returned by the command in this browser.</p>",
                hex(&pending.request),
                hex(&pending.request),
            );
            Ok(response(StatusCode::OK, "text/html; charset=utf-8", page))
        }
        (&Method::GET, "/oauth/consent") if query.is_none() => Ok(response(
            StatusCode::OK,
            "text/html; charset=utf-8",
            CONSENT_HTML,
        )),
        (&Method::GET, "/oauth/consent.js") if query.is_none() => Ok(response(
            StatusCode::OK,
            "text/javascript; charset=utf-8",
            CONSENT_JS,
        )),
        (&Method::POST, "/oauth/preview" | "/oauth/decision" | "/oauth/token")
            if query.is_none() =>
        {
            if path == "/oauth/token" {
                if request.headers().contains_key("origin")
                    || request.headers().contains_key("authorization")
                {
                    return Err(invalid().into());
                }
            } else if provided(&request, "origin") != Some(state.origin.as_str()) {
                return Err(harmion_node::Error::Forbidden.into());
            }
            if provided(&request, "content-type").and_then(|value| value.split(';').next())
                != Some("application/x-www-form-urlencoded")
            {
                return Err(invalid().into());
            }
            let bytes = Limited::new(request.into_body(), MAX_FORM)
                .collect()
                .await
                .map_err(|_| invalid())?
                .to_bytes();
            let mut form = Form::parse(&bytes)?;
            match path.as_str() {
                "/oauth/preview" => {
                    let ticket = state.ticket(&form.take("ticket")?, false)?;
                    form.finish()?;
                    let (client, pending) = store
                        .run(move |db| {
                            oauth::pending(
                                db,
                                &ticket.owner,
                                SystemHost.now_micros(),
                                ticket.request,
                            )
                        })
                        .await?;
                    Ok(json_response(
                        StatusCode::OK,
                        json!({
                            "name":client.name, "client_id":hex(&client.id.0),
                            "redirect_uri":pending.redirect, "grants":GrantSpec::from(pending.grants),
                        }),
                    ))
                }
                "/oauth/decision" => {
                    let allow = match form.take("decision")?.as_str() {
                        "approve" => true,
                        "deny" => false,
                        _ => return Err(invalid().into()),
                    };
                    let encoded = form.take("ticket")?;
                    form.finish()?;
                    let ticket = state.ticket(&encoded, true)?;
                    let decision = store
                        .run_notifying(true, move |db| {
                            oauth::decide(db, &mut SystemHost, &ticket.owner, ticket.request, allow)
                        })
                        .await?;
                    if let (Some(app), Some(grants)) = (decision.app, decision.grants.clone()) {
                        store.notify_app_changed(app, &grants, false).await?;
                    }
                    let mut callback = Url::parse(&decision.redirect).map_err(transport)?;
                    {
                        let mut params = callback.query_pairs_mut();
                        if let Some(code) = decision.code {
                            params.append_pair("code", &hex(&code));
                        } else {
                            params.append_pair("error", "access_denied");
                        }
                        params.append_pair("state", &decision.state);
                    }
                    Ok(json_response(
                        StatusCode::OK,
                        json!({"redirect_uri":callback.as_str()}),
                    ))
                }
                "/oauth/token" => token(&store, form).await,
                _ => Err(invalid().into()),
            }
        }
        _ => Ok(error(StatusCode::NOT_FOUND, "not_found")),
    }
}

async fn token<S: Store + Send + 'static>(
    store: &SharedStore<S>,
    mut form: Form,
) -> Result<Response<Body>, Error> {
    let client = AppId(unhex(&form.take("client_id")?)?);
    let grant = form.take("grant_type")?;
    let changes = store.changes.clone();
    let tokens = match grant.as_str() {
        "authorization_code" => {
            let code =
                unhex(&form.take("code")?).map_err(|_| harmion_node::Error::OAuthInvalidGrant)?;
            let verifier = form.take("code_verifier")?;
            let redirect = form.take("redirect_uri")?;
            form.finish()?;
            store
                .run(move |db| {
                    let result =
                        oauth::exchange(db, &mut SystemHost, client, code, &redirect, &verifier);
                    changes.send_replace(());
                    result
                })
                .await?
        }
        "refresh_token" => {
            let refresh = unhex(&form.take("refresh_token")?)
                .map_err(|_| harmion_node::Error::OAuthInvalidGrant)?;
            form.finish()?;
            store
                .run(move |db| {
                    let result = oauth::refresh(db, &mut SystemHost, client, refresh);
                    changes.send_replace(());
                    result
                })
                .await?
        }
        _ => return Ok(error(StatusCode::BAD_REQUEST, "unsupported_grant_type")),
    };
    Ok(json_response(
        StatusCode::OK,
        json!({
            "token_type":"Bearer", "access_token":hex(tokens.access.expose_bytes()),
            "expires_in":tokens.expires_in, "refresh_token":hex(&tokens.refresh),
            "scope":format_scope(&tokens.grants),
        }),
    ))
}

struct Form(BTreeMap<String, String>);
impl Form {
    fn parse(bytes: &[u8]) -> Result<Self, harmion_node::Error> {
        if bytes.len() > MAX_FORM {
            return Err(invalid());
        }
        let mut fields = BTreeMap::new();
        for (key, value) in form_urlencoded::parse(bytes) {
            if fields
                .insert(key.into_owned(), value.into_owned())
                .is_some()
            {
                return Err(invalid());
            }
        }
        Ok(Self(fields))
    }
    fn take(&mut self, name: &str) -> Result<String, harmion_node::Error> {
        self.0.remove(name).ok_or_else(invalid)
    }
    fn require(&mut self, name: &str, expected: &str) -> Result<(), harmion_node::Error> {
        if self.take(name)? == expected {
            Ok(())
        } else {
            Err(invalid())
        }
    }
    fn finish(self) -> Result<(), harmion_node::Error> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(invalid())
        }
    }
}

fn parse_scope(value: &str) -> Result<Grants, harmion_node::Error> {
    let mut grants = Grants::default();
    for scope in value.split(' ') {
        if scope == "call" {
            grants.call = true;
            continue;
        }
        let (permission, filter) = scope.split_once(':').ok_or_else(invalid)?;
        if permission == "provide" {
            if filter == "all" {
                grants.provide = true;
                continue;
            }
            let name = String::from_utf8(URL_SAFE_NO_PAD.decode(filter).map_err(|_| invalid())?)
                .map_err(|_| invalid())?;
            harmion_protocol::interpreter::validate_provider_name(&name).map_err(|_| invalid())?;
            if !grants.provide_names.insert(name) {
                return Err(invalid());
            }
            grants.provide = true;
            continue;
        }
        let field = match permission {
            "read" => &mut grants.read,
            "write" => &mut grants.write,
            _ => return Err(invalid()),
        };
        if filter == "all" && field.is_none() {
            *field = Some(Filter::All);
        } else {
            let encoded = filter.strip_prefix("kind:").ok_or_else(invalid)?;
            let name = String::from_utf8(URL_SAFE_NO_PAD.decode(encoded).map_err(|_| invalid())?)
                .map_err(|_| invalid())?;
            if name.is_empty() || name.chars().any(char::is_control) {
                return Err(invalid());
            }
            let Filter::Kinds(kinds) = field.get_or_insert_with(|| Filter::Kinds(BTreeSet::new()))
            else {
                return Err(invalid());
            };
            if !kinds.insert(name) {
                return Err(invalid());
            }
        }
    }
    Ok(grants)
}

pub(crate) fn format_scope(grants: &Grants) -> String {
    let mut scopes = Vec::new();
    for (permission, filter) in [("read", &grants.read), ("write", &grants.write)] {
        match filter {
            Some(Filter::All) => scopes.push(format!("{permission}:all")),
            Some(Filter::Kinds(kinds)) => scopes.extend(
                kinds
                    .iter()
                    .map(|kind| format!("{permission}:kind:{}", URL_SAFE_NO_PAD.encode(kind))),
            ),
            None => {}
        }
    }
    if grants.call {
        scopes.push("call".into());
    }
    if grants.provide {
        if grants.provide_names.is_empty() {
            scopes.push("provide:all".into());
        } else {
            scopes.extend(
                grants
                    .provide_names
                    .iter()
                    .map(|name| format!("provide:{}", URL_SAFE_NO_PAD.encode(name))),
            );
        }
    }
    scopes.join(" ")
}

fn invalid() -> harmion_node::Error {
    harmion_node::Error::InvalidRequest
}

fn response(status: StatusCode, content_type: &str, body: impl Into<Bytes>) -> Response<Body> {
    Response::builder().status(status)
        .header("content-type", content_type)
        .header("cache-control", "no-store")
        .header("pragma", "no-cache")
        .header("referrer-policy", "no-referrer")
        .header("x-content-type-options", "nosniff")
        .header("content-security-policy", "default-src 'none'; script-src 'self'; connect-src 'self'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'")
        .body(Full::new(body.into())).expect("static response headers")
}
fn json_response(status: StatusCode, value: Value) -> Response<Body> {
    response(status, "application/json", value.to_string())
}
fn error(status: StatusCode, code: &str) -> Response<Body> {
    json_response(status, json!({"error":code}))
}

const CONSENT_HTML: &str = r#"<!doctype html>
<meta charset="utf-8"><title>Harmion consent</title>
<h1>Harmion consent</h1><p id="status">Checking the owner link…</p>
<pre id="request"></pre>
<form method="post" action="/oauth/decision" id="decision" hidden>
<input type="hidden" name="ticket" id="ticket">
<button name="decision" value="approve">Approve</button>
<button name="decision" value="deny">Deny</button>
</form>
<script src="/oauth/consent.js" defer></script>"#;
const CONSENT_JS: &str = r#"
(async () => {
  const ticket = location.hash.slice(1);
  history.replaceState(null, "", location.pathname);
  const status = document.getElementById("status");
  try {
    if (!/^[0-9a-f]{64}$/.test(ticket)) throw new Error();
    const response = await fetch("/oauth/preview", {
      method: "POST", body: new URLSearchParams({ticket})
    });
    if (!response.ok) throw new Error();
    const request = await response.json();
    document.getElementById("request").textContent = JSON.stringify(request, null, 2);
    document.getElementById("ticket").value = ticket;
    const form = document.getElementById("decision");
    form.addEventListener("submit", async (event) => {
      event.preventDefault();
      if (!event.submitter) return;
      form.hidden = true;
      try {
        const body = new URLSearchParams({ticket, decision: event.submitter.value});
        const response = await fetch("/oauth/decision", {method: "POST", body});
        if (!response.ok) throw new Error();
        location.replace((await response.json()).redirect_uri);
      } catch (_) {
        status.textContent = "The decision failed. Request a new owner consent link.";
      }
    });
    form.hidden = false;
    status.textContent = "Confirm this app, callback and requested permissions.";
  } catch (_) {
    status.textContent = "This owner link is invalid or expired. Request a new consent link.";
  }
})();
"#;

impl Service {
    /// Issue a short-lived, single-use ticket for a browser WebSocket to the
    /// Provider endpoint on this browser-trusted origin.
    pub(crate) fn issue_provider_ticket(
        &self,
        app: AppId,
        provider: harmion_protocol::interpreter::ProviderId,
        now: i64,
    ) -> Result<(String, i64), Error> {
        let secret = Credential::from_bytes(SystemHost.random_bytes()?);
        let expires_at = now.saturating_add(30_000_000);
        let mut tickets = self
            .provider_tickets
            .lock()
            .map_err(|_| harmion_node::Error::Storage("provider tickets poisoned".into()))?;
        tickets.retain(|_, ticket| ticket.expires_at > now);
        if tickets.len() >= 128 || tickets.values().filter(|ticket| ticket.app == app).count() >= 8
        {
            return Err(harmion_node::Error::InterpreterCapacity.into());
        }
        tickets.insert(
            secret.verifier(),
            ProviderTicket {
                app,
                provider,
                expires_at,
            },
        );
        Ok((hex(secret.expose_bytes()), expires_at))
    }

    pub(crate) fn consume_provider_ticket(
        &self,
        encoded: &str,
        now: i64,
    ) -> Result<ProviderTicket, Error> {
        let key = Credential::from_bytes(unhex(encoded)?).verifier();
        let mut tickets = self
            .provider_tickets
            .lock()
            .map_err(|_| harmion_node::Error::Storage("provider tickets poisoned".into()))?;
        let ticket = tickets
            .remove(&key)
            .filter(|ticket| ticket.expires_at > now)
            .ok_or(harmion_node::Error::Forbidden)?;
        Ok(ticket)
    }

    pub(crate) fn provider_websocket_url(&self, ticket: &str) -> String {
        let origin = self.origin.replacen("https://", "wss://", 1);
        format!("{origin}/provider/connect?ticket={ticket}")
    }
}

async fn connect_dispatch<S: Store + Send + 'static>(
    store: SharedStore<S>,
    request: Request<Incoming>,
) -> Result<Response<Body>, Error> {
    let path = request.uri().path().to_owned();
    let origin = cors_origin(&request)?;
    if request.method() == Method::OPTIONS {
        if origin.is_none() || provided(&request, "access-control-request-method") != Some("POST") {
            return Err(invalid().into());
        }
        let mut response = response(StatusCode::NO_CONTENT, "text/plain; charset=utf-8", "");
        add_cors(&mut response, origin.as_deref().expect("checked"));
        return Ok(response);
    }
    if request.method() != Method::POST {
        return Err(invalid().into());
    }
    let operation = match path.as_str() {
        "/connect/start" => "connect-start",
        "/connect/status" => "connect-status",
        "/connect/complete" => "connect-complete",
        "/connect/ticket" => "connect-ticket",
        _ => return Err(invalid().into()),
    };
    let credential = if operation == "connect-ticket" {
        bearer_credential(&request)
    } else {
        None
    };
    let limit = if operation == "connect-start" {
        harmion_protocol::connect::MAX_OFFER_BYTES
    } else {
        16 * 1024
    };
    let body = Limited::new(request.into_body(), limit)
        .collect()
        .await
        .map_err(|_| invalid())?
        .to_bytes();
    let result = if operation == "connect-ticket" {
        match credential {
            Some(credential) => {
                crate::connect::provider_ticket(&store, credential, body.to_vec()).await
            }
            None => Err(Error::Node(harmion_node::Error::Forbidden)),
        }
    } else {
        crate::connect::handle_public(&store, operation, body.to_vec()).await
    };
    let mut response = match result {
        Ok(value) => json_response(StatusCode::OK, value),
        Err(error) => connect_failure(error),
    };
    if let Some(origin) = origin {
        add_cors(&mut response, &origin);
    }
    Ok(response)
}

async fn provider_connect<S: Store + Send + 'static>(
    store: SharedStore<S>,
    state: Arc<Service>,
    request: &mut Request<Incoming>,
) -> Result<Response<Body>, Error> {
    if request.uri().path() != "/provider/connect" {
        return Err(invalid().into());
    }
    let mut form = Form::parse(request.uri().query().unwrap_or("").as_bytes())?;
    let ticket = form.take("ticket")?;
    form.finish()?;
    // Validate the upgrade before spending the single-use ticket: a malformed
    // request must not be able to burn an app's 30-second capability.
    let key = crate::interpreter::validate_upgrade(request.headers()).map_err(|_| invalid())?;
    let ticket = state.consume_provider_ticket(&ticket, SystemHost.now_micros())?;
    let upgrade = hyper::upgrade::on(&mut *request);
    let interpreter = store.interpreter.clone();
    tokio::spawn(async move {
        match upgrade.await {
            Ok(upgraded) => {
                if let Err(error) = interpreter
                    .serve_provider_ticket(store, ticket, upgraded)
                    .await
                {
                    tracing::debug!(%error, "browser Provider connection ended");
                }
            }
            Err(error) => tracing::debug!(%error, "browser Provider upgrade failed"),
        }
    });
    let upgraded = crate::interpreter::switching_protocols(&key);
    let (parts, _) = upgraded.into_parts();
    Ok(Response::from_parts(parts, Full::new(Bytes::new())))
}

fn connect_failure(error: Error) -> Response<Body> {
    use harmion_node::Error as N;
    let (status, code) = match error {
        Error::Node(N::ConnectionNotFound) => (StatusCode::NOT_FOUND, "connection_not_found"),
        Error::Node(N::ConnectionExpired) => (StatusCode::GONE, "connection_expired"),
        Error::Node(N::ConnectionCompleted) => (StatusCode::CONFLICT, "connection_completed"),
        Error::Node(N::ConnectionPending) => (StatusCode::CONFLICT, "connection_pending"),
        Error::Node(N::ConnectionDenied) => (StatusCode::FORBIDDEN, "connection_denied"),
        Error::Node(N::ConnectionCapacity) => {
            (StatusCode::TOO_MANY_REQUESTS, "connection_capacity")
        }
        Error::Node(N::InterpreterNameConflict) => {
            (StatusCode::CONFLICT, "interpreter_name_conflict")
        }
        Error::Node(N::Forbidden | N::Unauthenticated) => (StatusCode::FORBIDDEN, "forbidden"),
        Error::Node(N::InvalidRequest) => (StatusCode::BAD_REQUEST, "invalid_request"),
        Error::Node(N::Storage(_) | N::CorruptLog | N::Entropy(_)) => {
            (StatusCode::INTERNAL_SERVER_ERROR, "server_error")
        }
        _ => (StatusCode::BAD_REQUEST, "invalid_request"),
    };
    json_response(status, json!({"error": code}))
}

fn bearer_credential(request: &Request<Incoming>) -> Option<Credential> {
    provided(request, "authorization")?
        .strip_prefix("Bearer ")
        .and_then(|value| unhex(value).ok())
        .map(Credential::from_bytes)
}

fn cors_origin(request: &Request<Incoming>) -> Result<Option<String>, Error> {
    let Some(value) = provided(request, "origin") else {
        return Ok(None);
    };
    let url = Url::parse(value).map_err(|_| invalid())?;
    let loopback = match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        _ => false,
    };
    if !matches!(url.scheme(), "http" | "https")
        || !loopback
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || url.port() == Some(0)
    {
        return Err(invalid().into());
    }
    Ok(Some(url.origin().ascii_serialization()))
}

fn add_cors(response: &mut Response<Body>, origin: &str) {
    if let Ok(value) = hyper::header::HeaderValue::from_str(origin) {
        response
            .headers_mut()
            .insert(hyper::header::ACCESS_CONTROL_ALLOW_ORIGIN, value);
    }
    response.headers_mut().insert(
        hyper::header::VARY,
        hyper::header::HeaderValue::from_static("Origin"),
    );
    response.headers_mut().insert(
        hyper::header::ACCESS_CONTROL_ALLOW_METHODS,
        hyper::header::HeaderValue::from_static("POST, OPTIONS"),
    );
    response.headers_mut().insert(
        hyper::header::ACCESS_CONTROL_ALLOW_HEADERS,
        hyper::header::HeaderValue::from_static("content-type, authorization"),
    );
    response.headers_mut().insert(
        hyper::header::ACCESS_CONTROL_MAX_AGE,
        hyper::header::HeaderValue::from_static("600"),
    );
}

const CONNECT_PAGE: &str = r#"<!doctype html>
<meta charset="utf-8"><title>Harmion connection</title>
<h1>Harmion connection</h1><p id="status">Waiting for owner approval…</p>
<script>
(async () => {
  const status = document.getElementById("status");
  try {
    const encoded = location.hash.slice(1);
    history.replaceState(null, "", location.pathname);
    let normalized = encoded.replace(/-/g, "+").replace(/_/g, "/");
    while (normalized.length % 4) normalized += "=";
    const request = JSON.parse(new TextDecoder().decode(
      Uint8Array.from(atob(normalized), c => c.charCodeAt(0))
    ));
    if (!/^[a-f0-9]{64}$/.test(request.connection_id) || !/^[a-f0-9]{64}$/.test(request.request_secret)) throw new Error();
    for (let attempt = 0; attempt < 300; attempt++) {
      const response = await fetch("/connect/status", {
        method: "POST", headers: {"content-type": "application/json"},
        body: JSON.stringify({connection_id: request.connection_id, request_secret: request.request_secret})
      });
      if (!response.ok) throw new Error();
      const view = await response.json();
      status.textContent = "Status: " + view.status;
      if (view.status === "approved" || view.status === "denied" ||
          view.status === "expired" || view.status === "completed") {
        if (window.opener && view.web_origin) {
          window.opener.postMessage({
            type: "harmion-connect", status: view.status,
            connection_id: view.connection_id, state: request.state
          }, view.web_origin);
          status.textContent = view.status === "approved"
            ? "Approved. You can close this window."
            : view.status === "completed"
              ? "This connection is already complete. You can close this window."
              : "The owner " + view.status + " the request.";
        }
        return;
      }
      await new Promise(resolve => setTimeout(resolve, 1000));
    }
    status.textContent = "The connection request expired. Return to the app and try again.";
  } catch (_) {
    status.textContent = "This connection link is invalid. Return to the app and request a new connection.";
  }
})();
</script>"#;

fn connect_page() -> Response<Body> {
    let mut response = response(StatusCode::OK, "text/html; charset=utf-8", CONNECT_PAGE);
    response.headers_mut().insert(
        hyper::header::CONTENT_SECURITY_POLICY,
        hyper::header::HeaderValue::from_static(
            "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'; frame-ancestors 'none'",
        ),
    );
    response.headers_mut().insert(
        hyper::header::CACHE_CONTROL,
        hyper::header::HeaderValue::from_static("no-store"),
    );
    response.headers_mut().insert(
        hyper::header::REFERRER_POLICY,
        hyper::header::HeaderValue::from_static("no-referrer"),
    );
    response.headers_mut().insert(
        hyper::header::X_CONTENT_TYPE_OPTIONS,
        hyper::header::HeaderValue::from_static("nosniff"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpreter_scopes_round_trip() {
        let scope = format!(
            "read:all call provide:{}",
            URL_SAFE_NO_PAD.encode("ledger/balance")
        );
        let grants = parse_scope(&scope).unwrap();
        assert!(grants.read == Some(Filter::All));
        assert!(grants.call);
        assert!(grants.provide);
        assert_eq!(grants.provide_names, ["ledger/balance".to_string()].into());
        assert_eq!(parse_scope(&format_scope(&grants)).unwrap(), grants);
        assert_eq!(
            format_scope(&parse_scope("provide:all").unwrap()),
            "provide:all"
        );
    }

    #[test]
    fn interpreter_scopes_reject_unknown_and_duplicate_values() {
        assert!(parse_scope("call call").is_ok());
        assert!(parse_scope("provide:").is_err());
        assert!(parse_scope("call:").is_err());
        assert!(parse_scope("admin").is_err());
        assert!(
            parse_scope(&format!(
                "provide:{} provide:{}",
                URL_SAFE_NO_PAD.encode("a/b"),
                URL_SAFE_NO_PAD.encode("a/b")
            ))
            .is_err()
        );
        assert!(parse_scope(&format!("provide:{}", URL_SAFE_NO_PAD.encode("bad name"))).is_err());
    }
}

#[test]
fn provider_tickets_are_single_use_and_bounded_per_app() {
    let service = Service {
        origin: "https://127.0.0.1:8443".into(),
        tickets: Mutex::new(BTreeMap::new()),
        provider_tickets: Mutex::new(BTreeMap::new()),
    };
    let app = AppId([1; 32]);
    let other = AppId([2; 32]);
    let provider = harmion_protocol::interpreter::ProviderId([3; 32]);
    let (ticket, expires_at) = service.issue_provider_ticket(app, provider, 100).unwrap();
    assert!(expires_at > 100);
    assert_eq!(
        service.provider_websocket_url(&ticket),
        format!("wss://127.0.0.1:8443/provider/connect?ticket={ticket}")
    );
    service.consume_provider_ticket(&ticket, 100).unwrap();
    assert!(service.consume_provider_ticket(&ticket, 100).is_err());
    for _ in 0..8 {
        service.issue_provider_ticket(app, provider, 100).unwrap();
    }
    assert!(service.issue_provider_ticket(app, provider, 100).is_err());
    assert!(service.issue_provider_ticket(other, provider, 100).is_ok());
    let (expired, expires_at) = service.issue_provider_ticket(other, provider, 100).unwrap();
    assert!(
        service
            .consume_provider_ticket(&expired, expires_at)
            .is_err()
    );
}
