//! Experimental signed-HTTPS transport-v2 codec building block, not an exposed server.
//!
//! Signatures are opaque bytes: successful construction/decoding proves only canonical
//! syntax, bounds and inner identity/class consistency. This module does NOT verify
//! signatures, trust, response freshness, challenge consumption, membership or WebPKI.
//! See contracts/https-peer-v2-draft.md. No HMC v2 support is implied.
use crate::{
    CodecError, MAX_PEER_BYTES, PeerCommand, decode_peer_request, decode_peer_response,
    record::{Decoder, Encoder},
};
use harmion_domain::{ClusterId, NodeId};
use sha2::{Digest, Sha256};

pub const MAX_FRAME_BYTES: usize = MAX_PEER_BYTES + 8192;
pub const MAX_METADATA_BYTES: usize = 8192;
pub const MAX_ORIGIN_BYTES: usize = 512;
pub const CHALLENGE_PATH: &str = "/peer/v2/challenge";
pub const EXCHANGE_PATH: &str = "/peer/v2/exchange";
const DOMAIN: &[u8] = b"harmion/https-peer/v2/";

pub type Hash = [u8; 32];
pub type Nonce = [u8; 32];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Common {
    pub cluster: ClusterId,
    pub sender: NodeId,
    pub audience: NodeId,
    /// Canonical server origin in both directions, not a node identity.
    pub origin: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PayloadClass {
    Data = 0,
    Evidence = 1,
}
impl PayloadClass {
    fn parse(tag: u8) -> Result<Self, CodecError> {
        match tag {
            0 => Ok(Self::Data),
            1 => Ok(Self::Evidence),
            _ => Err(CodecError::InvalidTag),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commitment {
    pub class: PayloadClass,
    pub inner_length: u32,
    pub inner_digest: Hash,
}
impl Commitment {
    /// Computes a commitment to valid HPQ v1 bytes. Does not authenticate them.
    pub fn from_request(request: &[u8]) -> Result<Self, CodecError> {
        let decoded = decode_peer_request(request)?;
        Ok(Self {
            class: command_class(&decoded.command),
            inner_length: request.len() as u32,
            inner_digest: Sha256::digest(request).into(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExchangeContext {
    pub challenge_request_hash: Hash,
    pub challenge_hash: Hash,
    pub client_nonce: Nonce,
    pub generation: Nonce,
    pub challenge_nonce: Nonce,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResponseContext {
    pub exchange_hash: Hash,
    pub challenge_hash: Hash,
    pub client_nonce: Nonce,
    pub generation: Nonce,
    pub challenge_nonce: Nonce,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum ErrorCode {
    InvalidEnvelope = 1,
    NotAuthorized = 2,
    ChallengeUnavailable = 3,
    BindingMismatch = 4,
    UnsupportedVersion = 5,
    Capacity = 6,
    StoreUnavailable = 7,
    InternalFailure = 8,
}
impl ErrorCode {
    pub fn http_status(self) -> u16 {
        match self {
            Self::InvalidEnvelope | Self::BindingMismatch | Self::UnsupportedVersion => 400,
            Self::NotAuthorized => 403,
            Self::ChallengeUnavailable => 409,
            Self::Capacity => 429,
            Self::StoreUnavailable => 503,
            Self::InternalFailure => 500,
        }
    }
    fn parse(code: u16) -> Result<Self, CodecError> {
        match code {
            1 => Ok(Self::InvalidEnvelope),
            2 => Ok(Self::NotAuthorized),
            3 => Ok(Self::ChallengeUnavailable),
            4 => Ok(Self::BindingMismatch),
            5 => Ok(Self::UnsupportedVersion),
            6 => Ok(Self::Capacity),
            7 => Ok(Self::StoreUnavailable),
            8 => Ok(Self::InternalFailure),
            _ => Err(CodecError::InvalidTag),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransportError {
    pub code: ErrorCode,
    /// Numeric wire value only; host policy must bound actual retry delays.
    pub retry_after_ms: u32,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResponseResult {
    /// Original HPS v1 bytes, never reconstructed from a decoded object.
    Peer(Vec<u8>),
    Error(TransportError),
}
impl ResponseResult {
    pub fn http_status(&self) -> u16 {
        match self {
            Self::Peer(_) => 200,
            Self::Error(error) => error.code.http_status(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Body {
    ChallengeRequest {
        client_nonce: Nonce,
        commitment: Commitment,
    },
    Challenge {
        challenge_request_hash: Hash,
        client_nonce: Nonce,
        generation: Nonce,
        challenge_nonce: Nonce,
        /// Nonzero numeric value; deployment TTL policy is NOT fixed here.
        ttl_ms: u32,
        commitment: Commitment,
    },
    Exchange {
        context: ExchangeContext,
        class: PayloadClass,
        /// Original HPQ v1 bytes, including original record/certificate bytes.
        request: Vec<u8>,
    },
    Response {
        context: ResponseContext,
        result: ResponseResult,
    },
    ChallengeError {
        challenge_request_hash: Hash,
        client_nonce: Nonce,
        error: TransportError,
    },
}
impl Body {
    fn kind(&self) -> u8 {
        match self {
            Self::ChallengeRequest { .. } => 0,
            Self::Challenge { .. } => 1,
            Self::Exchange { .. } => 2,
            Self::Response { .. } => 3,
            Self::ChallengeError { .. } => 4,
        }
    }
    fn domain(&self) -> &'static [u8] {
        match self {
            Self::ChallengeRequest { .. } => b"challenge-request",
            Self::Challenge { .. } => b"challenge",
            Self::Exchange { .. } => b"exchange",
            Self::Response { .. } => b"response",
            Self::ChallengeError { .. } => b"challenge-error",
        }
    }
    fn path(&self) -> &'static str {
        match self {
            Self::Exchange { .. } | Self::Response { .. } => EXCHANGE_PATH,
            _ => CHALLENGE_PATH,
        }
    }
    fn limit(&self) -> usize {
        match self {
            Self::Exchange { .. } | Self::Response { .. } => MAX_FRAME_BYTES,
            _ => MAX_METADATA_BYTES,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub common: Common,
    pub body: Body,
    /// Opaque Ed25519 wire bytes; even zero bytes are syntactically accepted.
    pub signature: [u8; 64],
}
impl Frame {
    pub fn new(common: Common, body: Body, signature: [u8; 64]) -> Result<Self, CodecError> {
        let frame = Self {
            common,
            body,
            signature,
        };
        frame.validate()?;
        Ok(frame)
    }
    fn validate(&self) -> Result<(), CodecError> {
        if self.common.sender == self.common.audience {
            return Err(CodecError::InvalidFieldValue);
        }
        validate_origin(&self.common.origin)?;
        match &self.body {
            Body::ChallengeRequest { commitment, .. } => validate_commitment(commitment)?,
            Body::Challenge {
                commitment, ttl_ms, ..
            } => {
                validate_commitment(commitment)?;
                if *ttl_ms == 0 {
                    return Err(CodecError::InvalidNumber);
                }
            }
            Body::Exchange { class, request, .. } => {
                let inner = decode_peer_request(request)?;
                if inner.cluster != self.common.cluster
                    || inner.node != self.common.sender
                    || *class != command_class(&inner.command)
                {
                    return Err(CodecError::InvalidFieldValue);
                }
            }
            Body::Response {
                result: ResponseResult::Peer(bytes),
                ..
            } => {
                let inner = decode_peer_response(bytes)?;
                if inner.cluster != self.common.cluster || inner.node != self.common.sender {
                    return Err(CodecError::InvalidFieldValue);
                }
            }
            _ => {}
        }
        Ok(())
    }
    fn unsigned(&self) -> Result<Vec<u8>, CodecError> {
        self.validate()?;
        let mut e = Encoder::with_limit(self.body.limit() - 64);
        e.raw(b"HHT\0")?;
        e.raw(&2u32.to_be_bytes())?;
        e.tag(self.body.kind())?;
        e.raw(&self.common.cluster.0)?;
        e.raw(&self.common.sender.0)?;
        e.raw(&self.common.audience.0)?;
        blob(&mut e, self.common.origin.as_bytes())?;
        route(&mut e, self.body.path())?;
        match &self.body {
            Body::ChallengeRequest {
                client_nonce,
                commitment,
            } => {
                e.raw(client_nonce)?;
                route(&mut e, EXCHANGE_PATH)?;
                put_commitment(&mut e, commitment)?;
            }
            Body::Challenge {
                challenge_request_hash,
                client_nonce,
                generation,
                challenge_nonce,
                ttl_ms,
                commitment,
            } => {
                for value in [
                    challenge_request_hash,
                    client_nonce,
                    generation,
                    challenge_nonce,
                ] {
                    e.raw(value)?;
                }
                e.raw(&ttl_ms.to_be_bytes())?;
                route(&mut e, EXCHANGE_PATH)?;
                put_commitment(&mut e, commitment)?;
            }
            Body::Exchange {
                context,
                class,
                request,
            } => {
                for value in [
                    &context.challenge_request_hash,
                    &context.challenge_hash,
                    &context.client_nonce,
                    &context.generation,
                    &context.challenge_nonce,
                ] {
                    e.raw(value)?;
                }
                e.tag(*class as u8)?;
                blob(&mut e, request)?;
            }
            Body::Response { context, result } => {
                for value in [
                    &context.exchange_hash,
                    &context.challenge_hash,
                    &context.client_nonce,
                    &context.generation,
                    &context.challenge_nonce,
                ] {
                    e.raw(value)?;
                }
                e.raw(&result.http_status().to_be_bytes())?;
                match result {
                    ResponseResult::Peer(bytes) => {
                        e.tag(0)?;
                        blob(&mut e, bytes)?;
                    }
                    ResponseResult::Error(error) => {
                        e.tag(1)?;
                        e.raw(&6u32.to_be_bytes())?;
                        put_error(&mut e, error)?;
                    }
                }
            }
            Body::ChallengeError {
                challenge_request_hash,
                client_nonce,
                error,
            } => {
                e.raw(challenge_request_hash)?;
                e.raw(client_nonce)?;
                e.raw(&error.code.http_status().to_be_bytes())?;
                put_error(&mut e, error)?;
            }
        }
        Ok(e.finish())
    }
    pub fn encode(&self) -> Result<Vec<u8>, CodecError> {
        let mut bytes = self.unsigned()?;
        bytes.extend_from_slice(&self.signature);
        Ok(bytes)
    }
    /// Domain || kind-name || NUL || canonical unsigned frame, for ordinary Ed25519.
    pub fn signing_bytes(&self) -> Result<Vec<u8>, CodecError> {
        let unsigned = self.unsigned()?;
        let mut bytes =
            Vec::with_capacity(DOMAIN.len() + self.body.domain().len() + 1 + unsigned.len());
        bytes.extend_from_slice(DOMAIN);
        bytes.extend_from_slice(self.body.domain());
        bytes.push(0);
        bytes.extend_from_slice(&unsigned);
        Ok(bytes)
    }
    /// SHA-256 of the complete canonical frame, INCLUDING opaque signature bytes.
    pub fn hash(&self) -> Result<Hash, CodecError> {
        Ok(Sha256::digest(self.encode()?).into())
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        let mut d = Decoder::with_limit(bytes, MAX_FRAME_BYTES)?;
        if d.raw(4)? != b"HHT\0" {
            return Err(CodecError::BadMagic);
        }
        let version = d.u32()?;
        if version != 2 {
            return Err(CodecError::UnsupportedVersion(version));
        }
        let kind = d.tag()?;
        if kind > 4 {
            return Err(CodecError::InvalidTag);
        }
        if !matches!(kind, 2 | 3) && bytes.len() > MAX_METADATA_BYTES {
            return Err(CodecError::TooLarge);
        }
        let common = Common {
            cluster: ClusterId(d.fixed()?),
            sender: NodeId(d.fixed()?),
            audience: NodeId(d.fixed()?),
            origin: std::str::from_utf8(get_blob(&mut d, MAX_ORIGIN_BYTES)?)
                .map_err(|_| CodecError::InvalidUtf8)?
                .to_owned(),
        };
        get_route(
            &mut d,
            if matches!(kind, 2 | 3) {
                EXCHANGE_PATH
            } else {
                CHALLENGE_PATH
            },
        )?;
        let body = match kind {
            0 => {
                let client_nonce = d.fixed()?;
                get_route(&mut d, EXCHANGE_PATH)?;
                Body::ChallengeRequest {
                    client_nonce,
                    commitment: get_commitment(&mut d)?,
                }
            }
            1 => {
                let challenge_request_hash = d.fixed()?;
                let client_nonce = d.fixed()?;
                let generation = d.fixed()?;
                let challenge_nonce = d.fixed()?;
                let ttl_ms = d.u32()?;
                get_route(&mut d, EXCHANGE_PATH)?;
                Body::Challenge {
                    challenge_request_hash,
                    client_nonce,
                    generation,
                    challenge_nonce,
                    ttl_ms,
                    commitment: get_commitment(&mut d)?,
                }
            }
            2 => {
                let context = ExchangeContext {
                    challenge_request_hash: d.fixed()?,
                    challenge_hash: d.fixed()?,
                    client_nonce: d.fixed()?,
                    generation: d.fixed()?,
                    challenge_nonce: d.fixed()?,
                };
                let class = PayloadClass::parse(d.tag()?)?;
                Body::Exchange {
                    context,
                    class,
                    request: get_blob(&mut d, MAX_PEER_BYTES)?.to_vec(),
                }
            }
            3 => {
                let context = ResponseContext {
                    exchange_hash: d.fixed()?,
                    challenge_hash: d.fixed()?,
                    client_nonce: d.fixed()?,
                    generation: d.fixed()?,
                    challenge_nonce: d.fixed()?,
                };
                let status = u16::from_be_bytes(d.fixed()?);
                let tag = d.tag()?;
                let result = match tag {
                    0 => ResponseResult::Peer(get_blob(&mut d, MAX_PEER_BYTES)?.to_vec()),
                    1 => {
                        let mut error = Decoder::with_limit(get_blob(&mut d, 6)?, 6)?;
                        let result = ResponseResult::Error(get_error(&mut error)?);
                        error.finish()?;
                        result
                    }
                    _ => return Err(CodecError::InvalidTag),
                };
                if status != result.http_status() {
                    return Err(CodecError::InvalidFieldValue);
                }
                Body::Response { context, result }
            }
            4 => {
                let challenge_request_hash = d.fixed()?;
                let client_nonce = d.fixed()?;
                let status = u16::from_be_bytes(d.fixed()?);
                let error = get_error(&mut d)?;
                if status != error.code.http_status() {
                    return Err(CodecError::InvalidFieldValue);
                }
                Body::ChallengeError {
                    challenge_request_hash,
                    client_nonce,
                    error,
                }
            }
            _ => return Err(CodecError::InvalidTag),
        };
        let signature = d.fixed()?;
        d.finish()?;
        Self::new(common, body, signature)
    }
}

fn command_class(command: &PeerCommand) -> PayloadClass {
    match command {
        PeerCommand::MembershipStatus
        | PeerCommand::MembershipCursor { .. }
        | PeerCommand::MembershipPage { .. }
        | PeerCommand::MembershipAccept(_) => PayloadClass::Evidence,
        _ => PayloadClass::Data,
    }
}
fn validate_commitment(commitment: &Commitment) -> Result<(), CodecError> {
    // Smallest canonical HPQ v1 frame: magic/version + cluster/node + tag.
    if commitment.inner_length < 73 {
        return Err(CodecError::InvalidFieldValue);
    }
    if commitment.inner_length as usize > MAX_PEER_BYTES {
        return Err(CodecError::TooLarge);
    }
    Ok(())
}
fn blob(e: &mut Encoder, bytes: &[u8]) -> Result<(), CodecError> {
    let length = u32::try_from(bytes.len()).map_err(|_| CodecError::TooLarge)?;
    e.raw(&length.to_be_bytes())?;
    e.raw(bytes)
}
fn get_blob<'a>(d: &mut Decoder<'a>, limit: usize) -> Result<&'a [u8], CodecError> {
    let length = d.u32()? as usize;
    if length > limit {
        return Err(CodecError::TooLarge);
    }
    d.raw(length)
}
fn route(e: &mut Encoder, path: &str) -> Result<(), CodecError> {
    e.tag(0)?; // Only POST.
    blob(e, path.as_bytes())
}
fn get_route(d: &mut Decoder<'_>, path: &str) -> Result<(), CodecError> {
    if d.tag()? != 0 {
        return Err(CodecError::InvalidTag);
    }
    if get_blob(d, 64)? != path.as_bytes() {
        return Err(CodecError::NonCanonical);
    }
    Ok(())
}
fn put_commitment(e: &mut Encoder, value: &Commitment) -> Result<(), CodecError> {
    e.tag(value.class as u8)?;
    e.raw(&value.inner_length.to_be_bytes())?;
    e.raw(&value.inner_digest)
}
fn get_commitment(d: &mut Decoder<'_>) -> Result<Commitment, CodecError> {
    Ok(Commitment {
        class: PayloadClass::parse(d.tag()?)?,
        inner_length: d.u32()?,
        inner_digest: d.fixed()?,
    })
}
fn put_error(e: &mut Encoder, error: &TransportError) -> Result<(), CodecError> {
    e.raw(&(error.code as u16).to_be_bytes())?;
    e.raw(&error.retry_after_ms.to_be_bytes())
}
fn get_error(d: &mut Decoder<'_>) -> Result<TransportError, CodecError> {
    Ok(TransportError {
        code: ErrorCode::parse(u16::from_be_bytes(d.fixed()?))?,
        retry_after_ms: d.u32()?,
    })
}

/// Syntactic ASCII canonical-origin subset only. Host must independently perform
/// normal URL/WebPKI validation, owner allowlisting and DNS/egress policy checks.
/// DNS names (including lowercase IDNA A-labels) only; no numeric URL-host aliases.
pub fn validate_origin(origin: &str) -> Result<(), CodecError> {
    if origin.len() > MAX_ORIGIN_BYTES {
        return Err(CodecError::TooLarge);
    }
    let invalid = || CodecError::NonCanonical;
    let parsed = url::Url::parse(origin).map_err(|_| invalid())?;
    if parsed.scheme() != "https"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.origin().ascii_serialization() != origin
    {
        return Err(invalid());
    }
    let host = match parsed.host() {
        Some(url::Host::Domain(host)) => host,
        _ => return Err(invalid()),
    };
    if host.is_empty() || host.len() > 253 {
        return Err(invalid());
    }
    for label in host.split('.') {
        if label.is_empty()
            || label.len() > 63
            || label.starts_with('-')
            || label.ends_with('-')
            || !label
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(invalid());
        }
    }
    if parsed.port() == Some(0) {
        return Err(invalid());
    }
    Ok(())
}
