//! Embeddable Harmion node runtime.
//!
//! The daemon and the Android node share this crate: it owns one node directory,
//! one database connection and one set of listeners for the lifetime of a
//! process, and it exposes lifecycle, status and sync control instead of a
//! process-wide signal handler.
//!
//! Nothing in this crate changes the data model, the peer protocol or the
//! authorization rules; those stay in `harmion-node`/`harmion-net-native`.
pub mod config;
pub mod initialize;
pub mod runtime;
pub mod scheduler;
pub mod status;
pub mod storage;

pub use config::{ConfigOverrides, NodeConfig};
pub use initialize::{InitializeOptions, InitializedNode, initialize, is_initialized, restore};
pub use runtime::{NodeRuntime, ReadyInfo, RuntimeOptions, SyncReportSummary};
pub use scheduler::{BlockReason, SchedulerState, SyncGate, SyncTrigger};
pub use status::{RuntimeStatus, StorageStatus, SyncStatusView};

/// Node lifecycle and host-integration errors. They are intentionally coarse:
/// callers are Android/Kotlin and the daemon, not untrusted clients.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unsupported node config version: {0}")]
    UnsupportedConfig(u32),
    #[error("App must be loopback; Peer needs a concrete IP; sync seconds must be 0..3600")]
    InvalidListenerConfig,
    #[error("OAuth must listen on 127.0.0.1")]
    InvalidOAuthListener,
    #[error("discovery none requires at least one explicit relay URL")]
    MissingRelay,
    #[error("invalid {0} key length")]
    InvalidKeyLength(&'static str),
    #[error("snapshot directory cannot be run; restore it to a new node first")]
    SnapshotDirectory,
    #[error("invalid signed-HTTPS trust root")]
    InvalidTrustRoot,
    #[error("node runtime is already shut down")]
    Stopped,
    /// The host sync policy currently blocks syncing (`code` names the reason).
    #[error("sync blocked by policy: {0}")]
    SyncBlocked(String),
    #[error("the node runtime failed to start: {0}")]
    Start(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Node(#[from] harmion_node::Error),
    #[error("{0}")]
    Transport(#[from] harmion_net_native::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Codec(#[from] harmion_protocol::CodecError),
}

pub(crate) type Result<T> = std::result::Result<T, Error>;
