//! Typed identifiers and events for the App subscription protocol.

use crate::{ReadView, hex, read_view_json};
use harmion_domain::ContentId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SubscriptionId(pub [u8; 32]);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResumeCursor(pub [u8; 32]);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    SnapshotBegin {
        subscription: SubscriptionId,
        cursor: ResumeCursor,
    },
    SnapshotChunk {
        items: Vec<ReadView>,
        cursor: ResumeCursor,
    },
    SnapshotEnd {
        cursor: ResumeCursor,
    },
    Changes {
        sequence: u64,
        upserts: Vec<ReadView>,
        removes: Vec<ContentId>,
        cursor: ResumeCursor,
    },
    ResetRequired,
}

pub fn encode_resume(id: SubscriptionId, cursor: ResumeCursor) -> [u8; 64] {
    let mut bytes = [0; 64];
    bytes[..32].copy_from_slice(&id.0);
    bytes[32..].copy_from_slice(&cursor.0);
    bytes
}

pub fn event_json(event: &Event) -> serde_json::Value {
    use serde_json::json;
    match event {
        Event::SnapshotBegin {
            subscription,
            cursor,
        } => json!({
            "type":"snapshot_begin", "subscription":hex(&subscription.0), "cursor":hex(&cursor.0)
        }),
        Event::SnapshotChunk { items, cursor } => json!({
            "type":"snapshot_chunk", "items":items.iter().map(read_view_json).collect::<Vec<_>>(),
            "cursor":hex(&cursor.0)
        }),
        Event::SnapshotEnd { cursor } => json!({"type":"snapshot_end","cursor":hex(&cursor.0)}),
        Event::Changes {
            sequence,
            upserts,
            removes,
            cursor,
        } => json!({
            "type":"changes", "sequence":sequence.to_string(), "cursor":hex(&cursor.0),
            "upserts":upserts.iter().map(read_view_json).collect::<Vec<_>>(),
            "removes":removes.iter().map(|id|hex(&id.0)).collect::<Vec<_>>()
        }),
        Event::ResetRequired => json!({"type":"reset_required"}),
    }
}
