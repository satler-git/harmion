//! Shared synchronous transaction and replication semantics. No networking runtime.

pub mod access;
pub mod application;
pub mod export;
pub mod https_peer;
pub mod identity;
pub mod membership;
pub mod membership_log;
pub mod membership_state;
pub mod oauth;
pub mod provisioning;
pub mod query;
pub mod replication;
pub mod storage;
pub mod subscription;
pub mod sync_status;

use harmion_domain as domain;
use harmion_protocol as protocol;

#[derive(Debug, thiserror::Error)]
#[error("{self:?}")]
pub enum Error {
    Storage(String),
    Codec(#[from] protocol::CodecError),
    Domain(#[from] domain::Error),
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
    OAuthInvalidGrant,
    OAuthCapacity,
    QueryExpired,
    QueryCapacity,
    QueryItemTooLarge,
    ResponseTooLarge,
    ExportBusy,
    SubscriptionCapacity,
    Entropy(String),
    PeerRetired,
    MembershipConflict,
    InvalidSignature,
    /// Observed wall time is behind durable authentication state.
    ClockReversal,
    RecordRejected {
        update_id: domain::UpdateId,
        #[source]
        reason: domain::Error,
    },
}
