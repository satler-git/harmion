//! Read-only status views. They are serializable so the daemon and the Android
//! FFI can expose the same shape without a second schema.
use crate::scheduler::BlockReason;
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Serialize)]
pub struct SyncStatusView {
    /// False when automatic sync is disabled by configuration (`sync_seconds=0`).
    pub enabled: bool,
    /// False while the host policy (Wi-Fi, metered, battery, roaming) blocks sync.
    pub permitted: bool,
    pub blocked: Option<BlockReason>,
    pub inflight: Vec<String>,
    pub retrying: BTreeMap<String, u32>,
    pub last_trigger: Option<String>,
    pub last_round_at: Option<i64>,
    pub last_success_at: Option<i64>,
    pub last_error: Option<String>,
    pub rounds_completed: u64,
    pub rounds_failed: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct StorageStatus {
    pub database_bytes: u64,
    /// Bytes available to the app's private storage, when the host reports it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub free_bytes: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RuntimeStatus {
    pub running: bool,
    pub started_at: i64,
    pub cluster_id: String,
    pub node_id: String,
    pub founder: bool,
    /// Loopback address of the App HTTPS API.
    pub app_endpoint: String,
    pub peer: harmion_protocol::PeerRegistration,
    pub oauth_origin: Option<String>,
    /// Membership status of this node (`active`, `unconfirmed`, `retired`, `conflict`).
    pub membership: String,
    /// Number of peers with an approved membership record.
    pub peers: usize,
    pub sync: SyncStatusView,
    pub storage: StorageStatus,
}
