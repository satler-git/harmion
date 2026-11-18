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
            let _ = timeout(Duration::from_secs(30), async {
                let stream = acceptor.accept(tcp).await.map_err(transport)?;
                hyper::server::conn::http1::Builder::new()
                    .max_headers(32)
                    .max_buf_size(24 * 1024)
                    .serve_connection(
                        TokioIo::new(stream),
                        service_fn(move |request| handle(store.clone(), state.clone(), request)),
                    )
                    .await
                    .map_err(transport)
            })
            .await;
        });
    }
}

async fn handle<S: Store + Send + 'static>(
    store: SharedStore<S>,
    state: Arc<Service>,
    request: Request<Incoming>,
) -> Result<Response<Body>, Infallible> {
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
                <h1>Harmion authorization</h1><p>Review this request with the Harmion owner CLI.</p>\
                <pre>harmion --node NODE_DIRECTORY oauth-consent {}</pre>\
                <p>Open the consent link returned by the command in this browser.</p>",
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
        let (permission, filter) = scope.split_once(':').ok_or_else(invalid)?;
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

fn format_scope(grants: &Grants) -> String {
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
