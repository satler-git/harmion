//! Shared synchronous transaction and replication semantics. No networking runtime.

pub mod access;
pub mod application;
pub mod connect;
pub mod export;
pub mod https_peer;
pub mod identity;
pub mod interpreter;
pub mod membership;
pub mod membership_log;
pub mod membership_state;
pub mod oauth;
pub mod owner;
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
    /// The edit basis is absent or was reclaimed by retention. Get the Content
    /// again; retrying the same basis cannot succeed.
    TokenExpired,
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
    /// The operation requires a fresh owner approval, but none was supplied.
    OwnerAssertionRequired,
    /// The owner signature, challenge binding, or credential is invalid.
    OwnerAssertionInvalid,
    /// The owner challenge exists but has expired.
    OwnerChallengeExpired,
    /// The owner challenge was already consumed.
    OwnerChallengeUsed,
    /// The recovery code does not match the node's recovery policy.
    OwnerRecoveryInvalid,
    /// Too many pending owner challenges.
    OwnerCapacity,
    /// The host cannot run the configured owner-signature verifier.
    OwnerVerifierUnavailable,
    /// The host does not provide this optional storage capability.
    Unsupported,
    /// Another approved Provider already owns the requested name.
    InterpreterNameConflict,
    /// The registration is pending, superseded, or owned by another app.
    InterpreterNotApproved,
    /// The authenticated app does not own this Provider registration.
    InterpreterProviderMismatch,
    /// The approved Provider's app is no longer allowed or connected.
    InterpreterUnavailable,
    /// The node's bounded active-invocation capacity is full.
    InterpreterCapacity,
    /// The caller canceled its invocation.
    InterpreterCancelled,
    /// The materialized Provider input exceeds the contract limit.
    InterpreterInputTooLarge,
    /// The Provider's response did not satisfy the contract.
    InterpreterInvalidResponse,
    /// The Provider did not answer before the invocation deadline.
    InterpreterTimeout,
    /// No connection request matches this id or secret.
    ConnectionNotFound,
    /// The connection request expired before completion.
    ConnectionExpired,
    /// The connection request was already completed.
    ConnectionCompleted,
    /// The connection request is awaiting owner approval.
    ConnectionPending,
    /// The owner denied the connection request.
    ConnectionDenied,
    /// Too many pending connection requests on this node.
    ConnectionCapacity,
}
