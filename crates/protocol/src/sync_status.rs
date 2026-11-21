//! Owner-only, node-local evidence from completed replication rounds.
//! Positions and counts are decimal strings; identifiers and epochs are hex.
//! This is historical evidence, not online status or a guarantee of future
//! remote durability.

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SyncStatus {
    pub node: String,
    pub local_epoch: String,
    pub local_high: String,
    pub update: Option<String>,
    pub peers: Vec<PeerSyncStatus>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PeerSyncStatus {
    pub node: String,
    /// Membership retirement, never transport liveness.
    pub retired: bool,
    pub last_success_at: Option<String>,
    pub last_failure: Option<SyncFailure>,
    pub local_epoch: Option<String>,
    pub acknowledged_through: Option<String>,
    pub remote_epoch: Option<String>,
    pub remote_high: Option<String>,
    pub received_through: Option<String>,
    /// Number of accepted local records not covered by the observed ACK prefix.
    pub unconfirmed_local: String,
    /// None means no applicable evidence (or no update requested). False means
    /// unconfirmed, NOT proof that the peer lacks the update: relay receipt and
    /// ACK loss are possible. True confirms receipt in the observed round only.
    pub update_confirmed: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SyncFailure {
    pub attempted_at: String,
    /// Stable, non-sensitive classification; never an error message or endpoint.
    pub code: String,
}
