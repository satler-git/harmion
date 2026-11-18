//! Shared synchronous transaction and replication semantics. No networking runtime.

pub mod access;
pub mod application;
pub mod membership;
pub mod membership_log;
pub mod membership_state;
pub mod replication;
pub mod storage;

use harmion_domain as domain;
use harmion_protocol as protocol;

#[derive(Debug)]
pub enum Error {
    Storage(String),
    Codec(protocol::CodecError),
    Domain(domain::Error),
    IdentityMismatch,
    CorruptLog,
    InvalidPage,
    CursorMismatch {
        current: u64,
    },
    Unauthenticated,
    Forbidden,
    NotFound,
    InvalidToken,
    RequestIdReuse,
    AlreadyBootstrapped,
    InvalidRequest,
    Entropy(String),
    PeerRetired,
    MembershipConflict,
    InvalidSignature,
    RecordRejected {
        update_id: domain::UpdateId,
        reason: domain::Error,
    },
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}
impl From<protocol::CodecError> for Error {
    fn from(e: protocol::CodecError) -> Self {
        Self::Codec(e)
    }
}
impl From<domain::Error> for Error {
    fn from(e: domain::Error) -> Self {
        Self::Domain(e)
    }
}
