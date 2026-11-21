//! Signed-HTTPS peer v2 control plane: typed signer, trustworthy verification,
//! bounded durable challenges and at-most-one dispatch per challenge.
//!
//! Signatures are verified with the durably pinned key by the caller before any
//! state is allocated or consumed. Nothing here decides membership current
//! authority; `membership::handle` keeps its own transaction and checks.
use crate::{Error, membership_state::MembershipSigner, storage::Identity};
use ed25519_dalek::{Signature, Signer, VerifyingKey};
use harmion_domain::NodeId;
use harmion_protocol::https_peer::{
    Body, Commitment, Common, ErrorCode, ExchangeContext, Frame, Hash, Nonce, PayloadClass,
    ResponseContext, ResponseResult, TransportError,
};
use harmion_protocol::{PeerRequest, decode_peer_request};
use sha2::{Digest, Sha256};

/// Provisional transport policy. Changing these values is not a wire/format
/// change, but deployment tuning still needs multi-page and four-peer tests.
pub const CHALLENGE_TTL_MS: u32 = 60_000;
pub const MAX_CHALLENGES_PER_REQUESTER: u64 = 8;
pub const MAX_CHALLENGES_PER_STORE: u64 = 256;
pub const RECLAIM_BATCH: u32 = 32;
pub const REQUESTER_BURST: i64 = 16;
pub const REQUESTER_REFILL_PER_SECOND: i64 = 2;
pub const STORE_BURST: i64 = 64;
pub const STORE_REFILL_PER_SECOND: i64 = 8;
/// Rate/quota scope for the whole replica, distinct from any node ID.
pub const STORE_SCOPE: NodeId = NodeId([0; 32]);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// Canonical owner-approved HTTPS base origin, e.g. `https://replica.example`.
    pub origin: String,
}

/// A signed-HTTPS request that was already authenticated by the caller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Authenticated {
    pub requester: NodeId,
    pub class: PayloadClass,
}

impl MembershipSigner {
    /// Constrained signer: only the five typed HTTPS peer domains are exposed,
    /// never an arbitrary message oracle.
    pub fn sign_challenge_request(
        &self,
        common: Common,
        client_nonce: Nonce,
        commitment: Commitment,
    ) -> Result<Frame, Error> {
        self.sign_frame(
            common,
            Body::ChallengeRequest {
                client_nonce,
                commitment,
            },
        )
    }
    pub fn sign_challenge(&self, common: Common, issue: ChallengeIssue) -> Result<Frame, Error> {
        self.sign_frame(
            common,
            Body::Challenge {
                challenge_request_hash: issue.request_hash,
                client_nonce: issue.client_nonce,
                generation: issue.generation,
                challenge_nonce: issue.challenge_nonce,
                ttl_ms: issue.ttl_ms,
                commitment: issue.commitment,
            },
        )
    }
    pub fn sign_exchange(
        &self,
        common: Common,
        context: ExchangeContext,
        class: PayloadClass,
        request: Vec<u8>,
    ) -> Result<Frame, Error> {
        self.sign_frame(
            common,
            Body::Exchange {
                context,
                class,
                request,
            },
        )
    }
    pub fn sign_response(
        &self,
        common: Common,
        context: ResponseContext,
        result: ResponseResult,
    ) -> Result<Frame, Error> {
        self.sign_frame(common, Body::Response { context, result })
    }
    pub fn sign_challenge_error(
        &self,
        common: Common,
        request_hash: Hash,
        client_nonce: Nonce,
        error: TransportError,
    ) -> Result<Frame, Error> {
        self.sign_frame(
            common,
            Body::ChallengeError {
                challenge_request_hash: request_hash,
                client_nonce,
                error,
            },
        )
    }
    fn sign_frame(&self, common: Common, body: Body) -> Result<Frame, Error> {
        let unsigned = Frame::new(common.clone(), body.clone(), [0; 64])?;
        let signature = self.key().sign(&unsigned.signing_bytes()?).to_bytes();
        Ok(Frame::new(common, body, signature)?)
    }
}

/// Strict ordinary Ed25519 verification over the exact canonical signing input
/// with the caller's already-trusted key. Never reads a key from the frame.
pub fn verify_frame(frame: &Frame, key: &[u8; 32]) -> Result<(), Error> {
    VerifyingKey::from_bytes(key)
        .map_err(|_| Error::InvalidSignature)?
        .verify_strict(
            &frame.signing_bytes()?,
            &Signature::from_bytes(&frame.signature),
        )
        .map_err(|_| Error::InvalidSignature)
}

/// Route/cluster/audience/origin checks that do not depend on any secret.
pub fn check_common(frame: &Frame, local: Identity, origin: &str) -> Result<(), Error> {
    if frame.common.cluster != local.cluster
        || frame.common.audience != local.node
        || frame.common.origin != origin
        || frame.common.sender == frame.common.audience
    {
        return Err(Error::IdentityMismatch);
    }
    Ok(())
}

/// The class of a request's inner HPQ command, checked before allocation.
pub fn request_class(request: &Frame) -> Result<PayloadClass, Error> {
    Ok(match &request.body {
        Body::ChallengeRequest { commitment, .. } => commitment.class,
        Body::Exchange { class, .. } => *class,
        _ => return Err(Error::InvalidRequest),
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChallengeClock {
    pub generation: Nonce,
    pub now: i64,
}

/// Fields a server signs into a challenge. Grouped so the typed signer keeps a
/// single narrow call shape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChallengeIssue {
    pub request_hash: Hash,
    pub client_nonce: Nonce,
    pub generation: Nonce,
    pub challenge_nonce: Nonce,
    pub ttl_ms: u32,
    pub commitment: Commitment,
}

/// Per-request authenticated context for the challenge/exchange service.
pub struct Session<'a> {
    pub signer: &'a MembershipSigner,
    pub config: &'a Config,
    pub local: Identity,
    pub requester: NodeId,
    pub now: i64,
}

/// Bounded durable challenge row. Inner record bytes are never stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChallengeRecord {
    pub requester: NodeId,
    pub client_nonce: Nonce,
    pub generation: Nonce,
    pub challenge_nonce: Nonce,
    pub challenge_request_hash: Hash,
    pub challenge_hash: Hash,
    pub commitment: Commitment,
    pub ttl_ms: u32,
    pub expires_at: i64,
    pub used: bool,
}

/// Durable transport state, separate from the node transaction. All methods run
/// inside one synchronous transaction; no network await and no data log writes.
pub trait ChallengeStore {
    fn challenge_transaction<T>(
        &mut self,
        f: impl FnOnce(&mut dyn ChallengeTransaction) -> Result<T, Error>,
    ) -> Result<T, Error>;
}

pub trait ChallengeTransaction {
    /// Fails closed on wall-clock reversal. The durable high-water is advanced
    /// to the returned value with monotonic elapsed time within one runtime.
    fn clock(
        &mut self,
        observed_wall_micros: i64,
        random: &mut dyn FnMut() -> Result<[u8; 32], Error>,
    ) -> Result<ChallengeClock, Error>;
    /// Bounded reclaim of spent rows and expired rows, then a bounded count.
    fn reclaim(&mut self, now: i64, limit: u32) -> Result<u32, Error>;
    fn challenge(
        &mut self,
        requester: NodeId,
        client_nonce: Nonce,
    ) -> Result<Option<ChallengeRecord>, Error>;
    fn insert_challenge(&mut self, record: &ChallengeRecord) -> Result<(), Error>;
    /// Compare-and-swap unused -> used. False means absent, spent, expired or
    /// generation-mismatched; the caller must not dispatch.
    fn consume_challenge(
        &mut self,
        requester: NodeId,
        client_nonce: Nonce,
        now: i64,
    ) -> Result<bool, Error>;
    fn count_challenges(&mut self, requester: Option<NodeId>) -> Result<u64, Error>;
    /// Durable integer token bucket. `scope` is a Node ID or `STORE_SCOPE`.
    fn take_rate(
        &mut self,
        scope: NodeId,
        now: i64,
        burst: i64,
        refill_per_second: i64,
    ) -> Result<bool, Error>;
    /// Explicit identity/replay-store recovery only. Fences prior challenges.
    fn reset_generation(
        &mut self,
        random: &mut dyn FnMut() -> Result<[u8; 32], Error>,
    ) -> Result<Nonce, Error>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PeerV2Response {
    Challenge(Frame),
    Exchange(Frame),
    /// Signed, bounded challenge-route error. Never used as a success.
    ChallengeError(Frame),
}

impl PeerV2Response {
    pub fn http_status(&self) -> u16 {
        match self {
            Self::Challenge(_) => 200,
            Self::Exchange(frame) => {
                let Body::Response { result, .. } = &frame.body else {
                    return 500;
                };
                result.http_status()
            }
            Self::ChallengeError(frame) => {
                let Body::ChallengeError { error, .. } = &frame.body else {
                    return 500;
                };
                error.code.http_status()
            }
        }
    }
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        Ok(match self {
            Self::Challenge(frame) | Self::Exchange(frame) | Self::ChallengeError(frame) => {
                frame.encode()?
            }
        })
    }
}

fn common(origin: &str, local: Identity, requester: NodeId) -> Common {
    Common {
        cluster: local.cluster,
        sender: local.node,
        audience: requester,
        origin: origin.to_owned(),
    }
}

fn challenge_error(
    session: &Session<'_>,
    request_hash: Hash,
    client_nonce: Nonce,
    error: TransportError,
) -> Result<PeerV2Response, Error> {
    Ok(PeerV2Response::ChallengeError(
        session.signer.sign_challenge_error(
            common(&session.config.origin, session.local, session.requester),
            request_hash,
            client_nonce,
            error,
        )?,
    ))
}

fn signed_challenge(session: &Session<'_>, record: &ChallengeRecord) -> Result<Frame, Error> {
    session.signer.sign_challenge(
        common(&session.config.origin, session.local, record.requester),
        ChallengeIssue {
            request_hash: record.challenge_request_hash,
            client_nonce: record.client_nonce,
            generation: record.generation,
            challenge_nonce: record.challenge_nonce,
            ttl_ms: record.ttl_ms,
            commitment: record.commitment.clone(),
        },
    )
}

/// Issue or replay the identical challenge for an authenticated challenge
/// request. Metadata only: no record or certificate bytes are read or stored.
pub fn issue_challenge<C: ChallengeStore + ?Sized>(
    store: &mut C,
    session: &Session<'_>,
    request: &Frame,
    random: &mut dyn FnMut() -> Result<[u8; 32], Error>,
) -> Result<PeerV2Response, Error> {
    let Body::ChallengeRequest {
        client_nonce,
        commitment,
    } = &request.body
    else {
        return Err(Error::InvalidRequest);
    };
    let client_nonce = *client_nonce;
    let commitment = commitment.clone();
    let request_hash = request.hash()?;
    let requester = session.requester;
    let outcome = store.challenge_transaction(|tx| {
        let clock = tx.clock(session.now, random)?;
        tx.reclaim(clock.now, RECLAIM_BATCH)?;
        if let Some(existing) = tx.challenge(requester, client_nonce)? {
            if existing.challenge_request_hash != request_hash || existing.commitment != commitment
            {
                return Ok(Err((ErrorCode::BindingMismatch, 0)));
            }
            if existing.used || existing.expires_at <= clock.now {
                return Ok(Err((ErrorCode::ChallengeUnavailable, 0)));
            }
            // Ed25519 is deterministic, so re-signing the same stored fields
            // reproduces the exact challenge bytes and stored hash.
            let frame = signed_challenge(session, &existing)?;
            if frame.hash()? != existing.challenge_hash {
                return Err(Error::CorruptLog);
            }
            return Ok(Ok(frame));
        }
        let requester_ok = tx.take_rate(
            requester,
            clock.now,
            REQUESTER_BURST,
            REQUESTER_REFILL_PER_SECOND,
        )?;
        let store_ok = requester_ok
            && tx.take_rate(STORE_SCOPE, clock.now, STORE_BURST, STORE_REFILL_PER_SECOND)?;
        if !requester_ok || !store_ok {
            return Ok(Err((ErrorCode::Capacity, 1_000)));
        }
        if tx.count_challenges(Some(requester))? >= MAX_CHALLENGES_PER_REQUESTER
            || tx.count_challenges(None)? >= MAX_CHALLENGES_PER_STORE
        {
            return Ok(Err((ErrorCode::Capacity, 1_000)));
        }
        let mut record = ChallengeRecord {
            requester,
            client_nonce,
            generation: clock.generation,
            challenge_nonce: random()?,
            challenge_request_hash: request_hash,
            // Filled after signing so the exact stored hash is returned.
            challenge_hash: [0; 32],
            commitment,
            ttl_ms: CHALLENGE_TTL_MS,
            expires_at: clock
                .now
                .saturating_add(i64::from(CHALLENGE_TTL_MS) * 1_000),
            used: false,
        };
        let frame = signed_challenge(session, &record)?;
        record.challenge_hash = frame.hash()?;
        tx.insert_challenge(&record)?;
        Ok(Ok(frame))
    })?;
    match outcome {
        Ok(frame) => Ok(PeerV2Response::Challenge(frame)),
        Err((code, retry_after_ms)) => challenge_error(
            session,
            request_hash,
            client_nonce,
            TransportError {
                code,
                retry_after_ms,
            },
        ),
    }
}

/// A challenge that this exchange proof consumed. The transport-state commit
/// already happened; the node handler runs after `begin_exchange` returns and
/// its result is signed by `complete_exchange`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsumedExchange {
    pub context: ResponseContext,
    pub exchange_hash: Hash,
    pub request: PeerRequest,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExchangeOutcome {
    /// Nothing was dispatched; the challenge was absent, spent, expired,
    /// generation-mismatched or bound to other commitments.
    Rejected(PeerV2Response),
    Consumed(ConsumedExchange),
}

/// Consume the exact challenge. This commits before the node handler may run.
/// At most one concurrent caller wins; a failed handler still leaves it used.
pub fn begin_exchange<C: ChallengeStore + ?Sized>(
    store: &mut C,
    session: &Session<'_>,
    request: &Frame,
    random: &mut dyn FnMut() -> Result<[u8; 32], Error>,
) -> Result<ExchangeOutcome, Error> {
    let Body::Exchange {
        context,
        class,
        request: inner_bytes,
    } = &request.body
    else {
        return Err(Error::InvalidRequest);
    };
    let exchange_hash = request.hash()?;
    let context = context.clone();
    let class = *class;
    let inner = decode_peer_request(inner_bytes)?;
    let requester = session.requester;
    if class != request_class(request)? {
        return Err(Error::InvalidRequest);
    }
    let (consumed, challenge_hash) = store.challenge_transaction(|tx| {
        let clock = tx.clock(session.now, random)?;
        tx.reclaim(clock.now, RECLAIM_BATCH)?;
        let Some(record) = tx.challenge(requester, context.client_nonce)? else {
            return Ok((false, context.challenge_hash));
        };
        let digest: Hash = Sha256::digest(inner_bytes).into();
        if record.generation != context.generation
            || record.challenge_request_hash != context.challenge_request_hash
            || record.challenge_hash != context.challenge_hash
            || record.challenge_nonce != context.challenge_nonce
            || record.commitment.class != class
            || record.commitment.inner_length as usize != inner_bytes.len()
            || record.commitment.inner_digest != digest
            || record.used
            || record.expires_at <= clock.now
        {
            return Ok((false, record.challenge_hash));
        }
        let consumed = tx.consume_challenge(requester, context.client_nonce, clock.now)?;
        Ok((consumed, record.challenge_hash))
    })?;
    if !consumed {
        return Ok(ExchangeOutcome::Rejected(PeerV2Response::Exchange(
            session.signer.sign_response(
                common(&session.config.origin, session.local, session.requester),
                ResponseContext {
                    exchange_hash,
                    challenge_hash,
                    client_nonce: context.client_nonce,
                    generation: context.generation,
                    challenge_nonce: context.challenge_nonce,
                },
                ResponseResult::Error(TransportError {
                    code: ErrorCode::ChallengeUnavailable,
                    retry_after_ms: 0,
                }),
            )?,
        )));
    }
    Ok(ExchangeOutcome::Consumed(ConsumedExchange {
        context: ResponseContext {
            exchange_hash,
            challenge_hash,
            client_nonce: context.client_nonce,
            generation: context.generation,
            challenge_nonce: context.challenge_nonce,
        },
        exchange_hash,
        request: inner,
    }))
}

/// Sign the outcome of the dispatched handler, bound to the exact exchange.
/// Transport errors expose bounded codes and never a message or payload.
pub fn complete_exchange(
    session: &Session<'_>,
    consumed: &ConsumedExchange,
    result: Result<Vec<u8>, Error>,
) -> Result<PeerV2Response, Error> {
    let response = match result {
        Ok(hps) => ResponseResult::Peer(hps),
        Err(
            Error::Unauthenticated
            | Error::PeerRetired
            | Error::MembershipConflict
            | Error::IdentityMismatch
            | Error::InvalidSignature,
        ) => ResponseResult::Error(TransportError {
            code: ErrorCode::NotAuthorized,
            retry_after_ms: 0,
        }),
        Err(Error::Storage(_) | Error::CorruptLog) => ResponseResult::Error(TransportError {
            code: ErrorCode::StoreUnavailable,
            retry_after_ms: 1_000,
        }),
        Err(_) => ResponseResult::Error(TransportError {
            code: ErrorCode::InternalFailure,
            retry_after_ms: 0,
        }),
    };
    Ok(PeerV2Response::Exchange(session.signer.sign_response(
        common(&session.config.origin, session.local, session.requester),
        consumed.context.clone(),
        response,
    )?))
}

/// The frame kind a client expects for its live attempt. Used by the transport
/// to reject a response that is bound to another request.
pub fn challenge_kind(frame: &Frame) -> Result<&'static str, Error> {
    Ok(match &frame.body {
        Body::Challenge { .. } => "challenge",
        Body::Response { .. } => "response",
        Body::ChallengeError { .. } => "challenge-error",
        _ => return Err(Error::InvalidRequest),
    })
}

/// Runtime monotonic clamp applied on top of the durable high-water.
#[derive(Debug, Default)]
pub struct AuthClock {
    last: Option<(i64, std::time::Instant)>,
}

impl AuthClock {
    /// Wall time must never move backwards relative to durable authentication
    /// state. A frozen or stalled clock is clamped forward with runtime elapsed
    /// time so challenge expiry still advances; the durable high-water itself
    /// only follows observed wall time.
    pub fn adjust(&mut self, observed: i64, high_water: i64) -> Result<i64, Error> {
        if observed < high_water {
            return Err(Error::ClockReversal);
        }
        let now = match self.last {
            Some((wall, instant)) => {
                let elapsed = instant
                    .elapsed()
                    .as_micros()
                    .min(i64::MAX as u128)
                    .try_into()
                    .map_err(|_| Error::ClockReversal)?;
                observed.max(wall.saturating_add(elapsed))
            }
            None => observed,
        };
        self.last = Some((now, std::time::Instant::now()));
        Ok(now)
    }
}
