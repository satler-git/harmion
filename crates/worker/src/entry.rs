//! Transport-agnostic signed-HTTPS peer v2 entry for a Worker/DO host.
//!
//! The host supplies the already-read request, a wall-clock reading that its
//! runtime advances monotonically, randomness, and the durable KV boundary. The
//! same node-store and challenge-store semantics as native apply; this module
//! performs no network I/O and does not implement TLS or routing policy.

use harmion_node::{
    Error as NodeError,
    https_peer::{self, ChallengeStore, Config, ExchangeOutcome, request_class},
    identity::KnownIdentity,
    membership::{self, PeerKey},
    membership_state::{self, MembershipSigner},
    storage::{Identity, Store},
};
use harmion_protocol::{
    encode_peer_response,
    https_peer::{
        Body, CHALLENGE_PATH, EXCHANGE_PATH, ErrorCode, Frame, MAX_FRAME_BYTES, PayloadClass,
        ResponseResult,
    },
};

pub const CONTENT_TYPE: &str = "application/vnd.harmion.https-peer-v2";

pub struct Request<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub host: Option<&'a str>,
    /// Whether an Origin header was present. Native clients never send one.
    pub has_origin: bool,
    pub body: &'a [u8],
}

/// Authenticated host context. The wall clock must be advanced by the host's
/// monotonic source; this module never reads a clock or entropy itself.
pub struct HostContext<'a> {
    pub signer: &'a MembershipSigner,
    pub config: &'a Config,
    pub local: Identity,
    pub now: i64,
}

pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Response {
    fn empty(status: u16) -> Self {
        Self {
            status,
            body: Vec::new(),
        }
    }
    fn frame(outcome: &harmion_node::https_peer::PeerV2Response) -> Self {
        match outcome.encode() {
            Ok(body) => Self {
                status: outcome.http_status(),
                body,
            },
            Err(_) => Self::empty(500),
        }
    }
}

fn authority(origin: &str) -> Option<String> {
    let parsed = url::Url::parse(origin).ok()?;
    let host = parsed.host_str()?;
    Some(match parsed.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    })
}

fn status_for(error: &harmion_node::Error) -> u16 {
    match error {
        harmion_node::Error::ClockReversal => 503,
        harmion_node::Error::Storage(_) | harmion_node::Error::CorruptLog => 503,
        _ => 400,
    }
}

/// Decode, authenticate, authorize at the transport class level, then issue or
/// exchange. Signed errors are bounded codes; unsigned errors carry no detail.
///
/// Ordering gate: the host must complete its physical durability flush after the
/// challenge-store transaction committed by `begin_exchange` and before running
/// the node handler, and again after the node transaction commits and before
/// releasing the returned response. This function cannot await that gate.
pub fn handle<S: Store, C: ChallengeStore + ?Sized>(
    node: &mut S,
    challenges: &mut C,
    host: &HostContext<'_>,
    request: Request<'_>,
    random: &mut dyn FnMut() -> Result<[u8; 32], NodeError>,
) -> Response {
    let signer = host.signer;
    let config = host.config;
    let local = host.local;
    let now = host.now;
    if request.method != "POST"
        || (request.path != CHALLENGE_PATH && request.path != EXCHANGE_PATH)
        || request.has_origin
        || request.host != authority(&config.origin).as_deref()
    {
        return Response::empty(400);
    }
    let limit = if request.path == CHALLENGE_PATH {
        harmion_protocol::https_peer::MAX_METADATA_BYTES
    } else {
        MAX_FRAME_BYTES
    };
    if request.body.len() > limit {
        return Response::empty(413);
    }
    let Ok(frame) = Frame::decode(request.body) else {
        return Response::empty(400);
    };
    if https_peer::check_common(&frame, local, &config.origin).is_err() {
        return Response::empty(400);
    }
    let requester = frame.common.sender;
    let pin: Option<KnownIdentity> = match node.transaction(|tx| tx.known_peer_identity(requester))
    {
        Ok(pin) => pin,
        Err(_) => return Response::empty(503),
    };
    let Some(pin) = pin else {
        return Response::empty(403);
    };
    if https_peer::verify_frame(&frame, &pin.signing_key).is_err() {
        return Response::empty(403);
    }
    if matches!(request_class(&frame), Ok(PayloadClass::Data)) {
        match node.transaction(|tx| membership_state::active_member(tx, requester).map(|_| ())) {
            Ok(()) => {}
            Err(harmion_node::Error::Storage(_) | harmion_node::Error::CorruptLog) => {
                return Response::empty(503);
            }
            Err(_) => return Response::empty(403),
        }
    }
    let session = https_peer::Session {
        signer,
        config,
        local,
        requester,
        now,
    };
    if request.path == CHALLENGE_PATH {
        return match https_peer::issue_challenge(challenges, &session, &frame, random) {
            Ok(outcome) => Response::frame(&outcome),
            Err(error) => Response::empty(status_for(&error)),
        };
    }
    let consumed = match https_peer::begin_exchange(challenges, &session, &frame, random) {
        Ok(ExchangeOutcome::Consumed(consumed)) => consumed,
        Ok(ExchangeOutcome::Rejected(rejected)) => return Response::frame(&rejected),
        Err(error) => return Response::empty(status_for(&error)),
    };
    let key = PeerKey::SignedHttps(pin.signing_key);
    // `handle` owns its own node transaction; the challenge was already consumed.
    let dispatched = membership::handle(node, &key, &consumed.request)
        .and_then(|reply| Ok(encode_peer_response(&reply)?));
    match https_peer::complete_exchange(&session, &consumed, dispatched) {
        Ok(outcome) => Response::frame(&outcome),
        Err(_) => Response::empty(500),
    }
}

/// A signed transport error carries no message, payload or key material.
pub fn is_signed_error(body: &[u8]) -> Option<ErrorCode> {
    let frame = Frame::decode(body).ok()?;
    match &frame.body {
        Body::Response {
            result: ResponseResult::Error(error),
            ..
        }
        | Body::ChallengeError { error, .. } => Some(error.code),
        _ => None,
    }
}
