use crate::{api, approval::Confirmer};
use bytes::Bytes;
use harmion_client::{
    Client, ProtectedDir, SshSigner,
    protocol::{MAX_APP_RESPONSE_BYTES, hex, unhex},
    random_id,
};
use http_body_util::{BodyExt, Full, Limited};
use hyper::{Method, Request, Response, StatusCode, body::Incoming, service::service_fn};
use hyper_util::rt::TokioIo;
use std::{
    convert::Infallible,
    io,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use tokio::{net::TcpListener, sync::Semaphore, task::JoinSet, time::timeout};
use tokio_rustls::{TlsAcceptor, rustls::ServerConfig};

type Body = Full<Bytes>;
mod assets {
    include!(concat!(env!("OUT_DIR"), "/assets.rs"));
}
const BOOTSTRAP_TTL: Duration = Duration::from_secs(120);
const SESSION_TTL: Duration = Duration::from_secs(30 * 60);
const SESSION_IDLE: Duration = Duration::from_secs(10 * 60);

pub(crate) struct State {
    client: Client,
    node: ProtectedDir,
    host: String,
    origin: String,
    access: Mutex<Access>,
    confirmations: crate::transfer::Confirmations,
    signer: Option<SshSigner>,
    confirmer: Confirmer,
    enforce: bool,
}

struct Access {
    launch: Option<Secret>,
    session: Option<Secret>,
}

struct Secret {
    value: [u8; 32],
    expires: Instant,
    last_used: Instant,
}

impl Secret {
    fn matches(&self, candidate: &[u8; 32]) -> bool {
        self.expires > Instant::now()
            && self.last_used.elapsed() < SESSION_IDLE
            && bool::from(self.value.ct_eq(candidate))
    }
    fn touch(&mut self) {
        self.last_used = Instant::now();
    }
}

impl State {
    pub fn new(
        client: Client,
        node: ProtectedDir,
        address: SocketAddr,
        content: Option<&str>,
        signer: Option<SshSigner>,
        confirmer: Confirmer,
        enforce: bool,
    ) -> Result<(Self, String), harmion_client::Error> {
        let launch = random_id()?;
        let host = address.to_string();
        let origin = format!("https://{host}");
        let path = content.map_or_else(|| "/".into(), |id| format!("/?content={id}"));
        let link = format!("{origin}{path}#{}", hex(&launch));
        Ok((
            Self {
                client,
                node,
                host,
                origin,
                access: Mutex::new(Access {
                    launch: Some(Secret {
                        value: launch,
                        expires: Instant::now() + BOOTSTRAP_TTL,
                        last_used: Instant::now(),
                    }),
                    session: None,
                }),
                confirmations: Default::default(),
                signer,
                confirmer,
                enforce,
            },
            link,
        ))
    }

    fn establish(&self, launch: [u8; 32]) -> Result<[u8; 32], StatusCode> {
        let mut access = self
            .access
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if !access
            .launch
            .as_ref()
            .is_some_and(|secret| secret.matches(&launch))
        {
            return Err(StatusCode::UNAUTHORIZED);
        }
        let value = random_id().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let now = Instant::now();
        access.launch = None;
        access.session = Some(Secret {
            value,
            expires: now + SESSION_TTL,
            last_used: now,
        });
        Ok(value)
    }

    fn authorize(&self, request: &Request<Incoming>) -> Result<[u8; 32], StatusCode> {
        let token = header(request, "authorization")
            .and_then(|header| header.strip_prefix("Bearer "))
            .and_then(|token| unhex(token).ok())
            .ok_or(StatusCode::UNAUTHORIZED)?;
        let mut access = self
            .access
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let secret = access
            .session
            .as_mut()
            .filter(|secret| secret.matches(&token))
            .ok_or(StatusCode::UNAUTHORIZED)?;
        secret.touch();
        Ok(secret.value)
    }

    fn enforcement(&self) -> (bool, Option<&SshSigner>) {
        (self.enforce, self.signer.as_ref())
    }
}

pub(crate) async fn run(
    listener: TcpListener,
    state: Arc<State>,
    config: ServerConfig,
) -> io::Result<()> {
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let slots = Arc::new(Semaphore::new(16));
    let mut connections = JoinSet::new();
    let shutdown = tokio::signal::ctrl_c();
    tokio::pin!(shutdown);
    loop {
        let socket = tokio::select! {
            result = &mut shutdown => { connections.shutdown().await; return result; },
            Some(_) = connections.join_next(), if !connections.is_empty() => continue,
            incoming = listener.accept() => incoming?.0,
        };
        let Ok(permit) = slots.clone().try_acquire_owned() else {
            continue;
        };
        let (acceptor, state) = (acceptor.clone(), state.clone());
        connections.spawn(async move {
            let _permit = permit;
            let _ = timeout(Duration::from_secs(150), async move {
                if let Ok(stream) = acceptor.accept(socket).await {
                    let _ = hyper::server::conn::http1::Builder::new()
                        .max_headers(24)
                        .max_buf_size(16 * 1024)
                        .serve_connection(
                            TokioIo::new(stream),
                            service_fn(move |request| {
                                let state = state.clone();
                                async move {
                                    Ok::<_, Infallible>(match dispatch(&state, request).await {
                                        Ok(response) => response,
                                        Err(status) => json(
                                            status,
                                            serde_json::json!({"error":status.as_u16()}),
                                        ),
                                    })
                                }
                            }),
                        )
                        .await;
                }
            })
            .await;
        });
    }
}

fn header<'a>(request: &'a Request<Incoming>, name: &str) -> Option<&'a str> {
    if request.headers().get_all(name).iter().count() != 1 {
        return None;
    }
    request.headers().get(name)?.to_str().ok()
}

async fn dispatch(state: &State, request: Request<Incoming>) -> Result<Response<Body>, StatusCode> {
    if header(&request, "host") != Some(state.host.as_str()) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let path = request.uri().path().to_owned();
    if request.method() == Method::GET {
        let (mime, bytes) = assets::asset(&path).ok_or(StatusCode::NOT_FOUND)?;
        return Ok(response(StatusCode::OK, mime, Bytes::from_static(bytes)));
    }
    if request.method() != Method::POST || request.uri().query().is_some() {
        return Err(StatusCode::NOT_FOUND);
    }
    if header(&request, "origin") != Some(state.origin.as_str()) {
        return Err(StatusCode::FORBIDDEN);
    }
    if header(&request, "content-type") != Some("application/json") {
        return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    let session = if path != "/session" {
        Some(state.authorize(&request)?)
    } else {
        None
    };
    let body = Limited::new(request.into_body(), MAX_APP_RESPONSE_BYTES)
        .collect()
        .await
        .map_err(|_| StatusCode::PAYLOAD_TOO_LARGE)?
        .to_bytes();
    if path == "/session" {
        let launch: String = serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST)?;
        let launch = unhex(&launch).map_err(|_| StatusCode::BAD_REQUEST)?;
        let session = state.establish(launch)?;
        return Ok(json(
            StatusCode::OK,
            serde_json::json!({"session":hex(&session),"expires_in":SESSION_TTL.as_secs()}),
        ));
    }
    if path == "/session/end" {
        state
            .access
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .session = None;
        return Ok(json(StatusCode::OK, serde_json::json!({})));
    }
    if let Some(summary) = api::approval_summary(&path, &body, &state.confirmations)
        && !state.confirmer.confirm(summary).await
    {
        return Err(StatusCode::FORBIDDEN);
    }
    let (enforce, signer) = state.enforcement();
    let caller = api::Caller::new(&state.client, signer, enforce, session.unwrap_or([0; 32]));
    let result = api::dispatch(&caller, &state.node, &state.confirmations, &path, &body).await?;
    Ok(json(StatusCode::OK, result))
}

fn json(status: StatusCode, value: serde_json::Value) -> Response<Body> {
    response(status, "application/json", value.to_string())
}

fn response(status: StatusCode, content_type: &str, body: impl Into<Bytes>) -> Response<Body> {
    Response::builder().status(status)
        .header("content-type", content_type)
        .header("cache-control", "no-store")
        .header("referrer-policy", "no-referrer")
        .header("x-content-type-options", "nosniff")
        .header("permissions-policy", "camera=(self), microphone=(), geolocation=()")
        .header("content-security-policy", "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data: blob:; media-src 'self' blob:; frame-ancestors 'none'; base-uri 'none'; form-action 'self'")
        .body(Full::new(body.into())).expect("static headers")
}
