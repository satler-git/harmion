/// Owner-only, node-local counters sampled in a single storage transaction.
/// Decimal strings preserve counters above JavaScript's exact integer range.
#[derive(serde::Serialize)]
pub struct NodeMetrics {
    pub contents: String,
    pub visible_contents: String,
    pub accepted_updates: String,
    pub record_bytes: String,
    pub active_apps: String,
}
