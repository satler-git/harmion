//! Staged streaming interpreter input. An upload is caller-scoped, ordered, and
//! bounded by a logical byte limit before any Provider invocation starts.
use harmion_domain::ContentId;
use harmion_protocol::AppId;
use harmion_protocol::interpreter::{
    ContentSelection, MAX_ACTIVE_INPUT_STREAMS, MAX_ACTIVE_INPUT_STREAMS_PER_APP,
    MAX_STREAM_INPUT_BYTES, ProviderId, StreamId,
};
use serde_json::Value as Json;
use std::collections::{BTreeMap, BTreeSet};

/// One open upload. `contents` preserves the caller's chunk and item order;
/// `seen` only enforces the contract's no-duplicate Content rule.
#[derive(Debug)]
pub struct StagedStream {
    pub id: StreamId,
    pub app: AppId,
    pub provider: ProviderId,
    pub provider_name: String,
    pub contract_version: u32,
    pub input: Json,
    pub timeout_ms: u64,
    pub started_at: i64,
    pub deadline: i64,
    pub next_seq: u64,
    pub contents: Vec<ContentSelection>,
    pub seen: BTreeSet<ContentId>,
    pub accepted_bytes: u64,
    pub accepted_contents: u64,
}
impl StagedStream {
    pub fn expired(&self, now: i64) -> bool {
        self.deadline <= now
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenError {
    NodeCapacity,
    AppCapacity,
    Duplicate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChunkError {
    NotFound,
    Forbidden,
    Expired {
        accepted_contents: u64,
        accepted_bytes: u64,
    },
    Sequence {
        expected: u64,
        received: u64,
    },
    Duplicate,
    Limit {
        accepted_contents: u64,
        accepted_bytes: u64,
        limit_bytes: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CancelOutcome {
    Canceled(StreamId),
    NotFound,
    Forbidden,
}

#[derive(Debug)]
pub struct StreamTable {
    max_node: usize,
    max_app: usize,
    limit_bytes: u64,
    active: BTreeMap<StreamId, StagedStream>,
    per_app: BTreeMap<AppId, usize>,
}
impl StreamTable {
    pub fn new(max_node: usize, max_app: usize, limit_bytes: u64) -> Self {
        Self {
            max_node,
            max_app,
            limit_bytes,
            active: BTreeMap::new(),
            per_app: BTreeMap::new(),
        }
    }
    pub fn default_limits() -> Self {
        Self::new(
            MAX_ACTIVE_INPUT_STREAMS,
            MAX_ACTIVE_INPUT_STREAMS_PER_APP,
            MAX_STREAM_INPUT_BYTES as u64,
        )
    }
    pub fn open(&mut self, stream: StagedStream) -> Result<(), OpenError> {
        if self.active.contains_key(&stream.id) {
            return Err(OpenError::Duplicate);
        }
        if self.active.len() >= self.max_node {
            return Err(OpenError::NodeCapacity);
        }
        let count = self.per_app.get(&stream.app).copied().unwrap_or(0);
        if count >= self.max_app {
            return Err(OpenError::AppCapacity);
        }
        self.per_app.insert(stream.app, count + 1);
        self.active.insert(stream.id, stream);
        Ok(())
    }
    /// Append one ordered chunk. `chunk_bytes` is the encoded size of the
    /// caller's Content selection frame and is charged before the chunk is
    /// accepted, so an oversized upload never leaves a partially applied chunk.
    pub fn chunk(
        &mut self,
        id: StreamId,
        app: AppId,
        seq: u64,
        contents: Vec<ContentSelection>,
        chunk_bytes: u64,
        now: i64,
    ) -> Result<(u64, u64, u64), ChunkError> {
        let Some(stream) = self.active.get(&id) else {
            return Err(ChunkError::NotFound);
        };
        if stream.app != app {
            return Err(ChunkError::Forbidden);
        }
        if stream.expired(now) {
            let stream = self.remove(id).expect("checked present");
            return Err(ChunkError::Expired {
                accepted_contents: stream.accepted_contents,
                accepted_bytes: stream.accepted_bytes,
            });
        }
        if stream.next_seq != seq {
            return Err(ChunkError::Sequence {
                expected: stream.next_seq,
                received: seq,
            });
        }
        let mut chunk_seen = BTreeSet::new();
        for selection in &contents {
            if stream.seen.contains(&selection.content) || !chunk_seen.insert(selection.content) {
                return Err(ChunkError::Duplicate);
            }
        }
        let accepted_bytes = stream
            .accepted_bytes
            .saturating_add(chunk_bytes)
            .min(self.limit_bytes.saturating_add(1));
        if accepted_bytes > self.limit_bytes {
            return Err(ChunkError::Limit {
                accepted_contents: stream.accepted_contents,
                accepted_bytes: stream.accepted_bytes,
                limit_bytes: self.limit_bytes,
            });
        }
        let stream = self.active.get_mut(&id).expect("checked present");
        for selection in contents {
            stream.seen.insert(selection.content);
            stream.contents.push(selection);
        }
        stream.accepted_bytes = accepted_bytes;
        stream.accepted_contents = stream.contents.len() as u64;
        stream.next_seq = seq.saturating_add(1);
        Ok((
            stream.next_seq,
            stream.accepted_contents,
            stream.accepted_bytes,
        ))
    }
    /// Remove and return a completed upload. Ending is single-use; an expired
    /// stream is removed and reported with its accepted partial state.
    pub fn take(&mut self, id: StreamId, app: AppId, now: i64) -> Result<StagedStream, ChunkError> {
        let Some(stream) = self.active.get(&id) else {
            return Err(ChunkError::NotFound);
        };
        if stream.app != app {
            return Err(ChunkError::Forbidden);
        }
        let stream = self.remove(id).expect("checked present");
        if stream.expired(now) {
            return Err(ChunkError::Expired {
                accepted_contents: stream.accepted_contents,
                accepted_bytes: stream.accepted_bytes,
            });
        }
        Ok(stream)
    }
    pub fn cancel(&mut self, id: StreamId, app: AppId) -> CancelOutcome {
        let Some(stream) = self.active.get(&id) else {
            return CancelOutcome::NotFound;
        };
        if stream.app != app {
            return CancelOutcome::Forbidden;
        }
        self.remove(id);
        CancelOutcome::Canceled(id)
    }
    /// Drop streams whose upload deadline has passed. No Provider invocation
    /// has started for these, so no cancel notification is needed.
    pub fn expire(&mut self, now: i64) -> Vec<StagedStream> {
        let expired: Vec<_> = self
            .active
            .values()
            .filter(|stream| stream.expired(now))
            .map(|stream| stream.id)
            .collect();
        expired
            .into_iter()
            .filter_map(|id| self.remove(id))
            .collect()
    }
    pub fn remove_app(&mut self, app: AppId) -> Vec<StagedStream> {
        let ids: Vec<_> = self
            .active
            .values()
            .filter(|stream| stream.app == app)
            .map(|stream| stream.id)
            .collect();
        ids.into_iter().filter_map(|id| self.remove(id)).collect()
    }
    pub fn get(&self, id: StreamId) -> Option<&StagedStream> {
        self.active.get(&id)
    }
    pub fn len(&self) -> usize {
        self.active.len()
    }
    pub fn is_empty(&self) -> bool {
        self.active.is_empty()
    }
    pub fn limit_bytes(&self) -> u64 {
        self.limit_bytes
    }
    fn remove(&mut self, id: StreamId) -> Option<StagedStream> {
        let stream = self.active.remove(&id)?;
        if let Some(count) = self.per_app.get_mut(&stream.app) {
            *count -= 1;
            if *count == 0 {
                self.per_app.remove(&stream.app);
            }
        }
        Some(stream)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use harmion_protocol::EditToken;

    fn stream(id: u8, app: u8, provider: u8, deadline: i64) -> StagedStream {
        StagedStream {
            id: StreamId([id; 32]),
            app: AppId([app; 32]),
            provider: ProviderId([provider; 32]),
            provider_name: "ledger/balance".into(),
            contract_version: 1,
            input: Json::Null,
            timeout_ms: 5_000,
            started_at: 0,
            deadline,
            next_seq: 0,
            contents: Vec::new(),
            seen: BTreeSet::new(),
            accepted_bytes: 0,
            accepted_contents: 0,
        }
    }

    fn selection(id: u8) -> ContentSelection {
        ContentSelection {
            content: ContentId([id; 32]),
            token: EditToken([id.wrapping_add(1); 32]),
        }
    }

    #[test]
    fn more_than_64_items_across_ordered_chunks_are_preserved() {
        let mut table = StreamTable::default_limits();
        let app = AppId([1; 32]);
        table.open(stream(1, 1, 9, 1_000)).unwrap();
        let first: Vec<_> = (1..=40).map(selection).collect();
        let second: Vec<_> = (41..=70).map(selection).collect();
        assert_eq!(
            table.chunk(StreamId([1; 32]), app, 0, first, 40, 10),
            Ok((1, 40, 40))
        );
        assert_eq!(
            table.chunk(StreamId([1; 32]), app, 1, second, 30, 11),
            Ok((2, 70, 70))
        );
        let staged = table.take(StreamId([1; 32]), app, 12).unwrap();
        assert_eq!(staged.contents.len(), 70);
        assert_eq!(staged.contents[0].content, ContentId([1; 32]));
        assert_eq!(staged.contents[69].content, ContentId([70; 32]));
        assert!(table.is_empty());
    }

    #[test]
    fn sequence_duplicate_ownership_and_expiry_are_explicit() {
        let mut table = StreamTable::default_limits();
        let app = AppId([1; 32]);
        table.open(stream(1, 1, 9, 100)).unwrap();
        assert_eq!(
            table.chunk(StreamId([1; 32]), app, 1, vec![selection(1)], 1, 10),
            Err(ChunkError::Sequence {
                expected: 0,
                received: 1
            })
        );
        table
            .chunk(StreamId([1; 32]), app, 0, vec![selection(1)], 1, 10)
            .unwrap();
        assert_eq!(
            table.chunk(StreamId([1; 32]), app, 1, vec![selection(1)], 1, 10),
            Err(ChunkError::Duplicate)
        );
        assert_eq!(
            table.chunk(
                StreamId([1; 32]),
                AppId([2; 32]),
                1,
                vec![selection(2)],
                1,
                10
            ),
            Err(ChunkError::Forbidden)
        );
        assert!(matches!(
            table.take(StreamId([1; 32]), app, 100),
            Err(ChunkError::Expired {
                accepted_contents: 1,
                accepted_bytes: 1
            })
        ));
        assert!(table.is_empty());
    }

    #[test]
    fn logical_byte_limit_reports_partial_state_without_applying_the_chunk() {
        let mut table = StreamTable::new(8, 8, 100);
        let app = AppId([1; 32]);
        table.open(stream(1, 1, 9, 1_000)).unwrap();
        table
            .chunk(StreamId([1; 32]), app, 0, vec![selection(1)], 60, 10)
            .unwrap();
        assert_eq!(
            table.chunk(
                StreamId([1; 32]),
                app,
                1,
                vec![selection(2), selection(3)],
                60,
                11
            ),
            Err(ChunkError::Limit {
                accepted_contents: 1,
                accepted_bytes: 60,
                limit_bytes: 100
            })
        );
        let staged = table.get(StreamId([1; 32])).unwrap();
        assert_eq!(staged.contents.len(), 1);
        assert_eq!(staged.next_seq, 1);
    }

    #[test]
    fn capacity_and_app_removal_release_slots() {
        let mut table = StreamTable::new(2, 1, 1_000);
        table.open(stream(1, 1, 9, 100)).unwrap();
        assert_eq!(
            table.open(stream(2, 1, 9, 100)),
            Err(OpenError::AppCapacity)
        );
        table.open(stream(3, 2, 9, 100)).unwrap();
        assert_eq!(
            table.open(stream(4, 3, 9, 100)),
            Err(OpenError::NodeCapacity)
        );
        assert_eq!(table.remove_app(AppId([1; 32])).len(), 1);
        table.open(stream(5, 1, 9, 100)).unwrap();
        assert_eq!(table.len(), 2);
        assert_eq!(table.expire(100).len(), 2);
        assert!(table.is_empty());
    }
}
