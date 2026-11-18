//! App subscription cache and resume state.

use crate::{Client, Error, protocol};
use protocol::{
    domain::ContentId,
    query::{Condition, MAX_QUERY_PAGE_ITEMS, QueryRequest, encode_request},
    subscription::{ResumeCursor, SubscriptionId, encode_resume},
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

type Contents = BTreeMap<ContentId, Value>;

pub struct Subscription {
    id: SubscriptionId,
    cursor: ResumeCursor,
    sequence: u64,
    contents: Contents,
    state: State,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Initial,
    Live,
    Ended,
}

pub struct SubscriptionCheckpoint {
    pub id: SubscriptionId,
    pub cursor: ResumeCursor,
    pub sequence: u64,
    pub contents: BTreeMap<ContentId, Value>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SubscriptionNotification {
    SnapshotChunk,
    SnapshotEnd,
    Changes {
        upserted: Vec<ContentId>,
        removed: Vec<ContentId>,
    },
    Idle,
    ResetRequired,
}

impl Subscription {
    /// The initial cache is made visible only after snapshot_end.
    pub fn contents(&self) -> Option<&BTreeMap<ContentId, Value>> {
        (self.state == State::Live).then_some(&self.contents)
    }

    /// Save the items together with the resume position after a notification is applied.
    pub fn checkpoint(&self) -> Option<SubscriptionCheckpoint> {
        self.contents().map(|contents| SubscriptionCheckpoint {
            id: self.id,
            cursor: self.cursor,
            sequence: self.sequence,
            contents: contents.clone(),
        })
    }

    pub fn from_checkpoint(checkpoint: SubscriptionCheckpoint) -> Result<Self, Error> {
        for (id, item) in &checkpoint.contents {
            if content_id(item)? != *id {
                return Err(Error::InvalidResponse);
            }
        }
        Ok(Self {
            id: checkpoint.id,
            cursor: checkpoint.cursor,
            sequence: checkpoint.sequence,
            contents: checkpoint.contents,
            state: State::Live,
        })
    }

    fn end(&mut self) {
        self.contents.clear();
        self.state = State::Ended;
    }

    fn apply(&mut self, mut value: Value) -> Result<SubscriptionNotification, Error> {
        use SubscriptionNotification as N;
        match value.get("type").and_then(Value::as_str) {
            Some("snapshot_chunk") if self.state == State::Initial => {
                let cursor = ResumeCursor(hex_field(&value, "cursor")?);
                let items = take_contents(&mut value, "items")?;
                if items.keys().any(|id| self.contents.contains_key(id)) {
                    return Err(Error::InvalidResponse);
                }
                self.contents.extend(items);
                self.cursor = cursor;
                Ok(N::SnapshotChunk)
            }
            Some("snapshot_end") if self.state == State::Initial => {
                self.cursor = ResumeCursor(hex_field(&value, "cursor")?);
                self.state = State::Live;
                Ok(N::SnapshotEnd)
            }
            Some("changes") if self.state == State::Live => {
                let sequence: u64 = value
                    .get("sequence")
                    .and_then(Value::as_str)
                    .ok_or(Error::InvalidResponse)?
                    .parse()
                    .map_err(|_| Error::InvalidResponse)?;
                if sequence <= self.sequence {
                    return Ok(N::Idle);
                }
                let cursor = ResumeCursor(hex_field(&value, "cursor")?);
                let upserts = take_contents(&mut value, "upserts")?;
                let removes = value
                    .get("removes")
                    .and_then(Value::as_array)
                    .ok_or(Error::InvalidResponse)?;
                let mut removed = BTreeSet::new();
                for id in removes {
                    let id = ContentId(decode_hex(id)?);
                    if upserts.contains_key(&id) || !removed.insert(id) {
                        return Err(Error::InvalidResponse);
                    }
                }
                let upserted = upserts.keys().copied().collect();
                self.contents.extend(upserts);
                for id in &removed {
                    self.contents.remove(id);
                }
                self.sequence = sequence;
                self.cursor = cursor;
                Ok(N::Changes {
                    upserted,
                    removed: removed.into_iter().collect(),
                })
            }
            Some("idle") if self.state == State::Live => Ok(N::Idle),
            Some("reset_required") => {
                self.end();
                Ok(N::ResetRequired)
            }
            _ => Err(Error::InvalidResponse),
        }
    }
}

impl Client {
    pub async fn subscribe(&self, condition: Condition) -> Result<Subscription, Error> {
        let request = encode_request(&QueryRequest::Start {
            condition,
            limit: MAX_QUERY_PAGE_ITEMS,
        })
        .map_err(|_| Error::InvalidRequest)?;
        let response = self.call("subscribe", &request).await?;
        if response.get("type").and_then(Value::as_str) != Some("snapshot_begin") {
            return Err(Error::InvalidResponse);
        }
        Ok(Subscription {
            id: SubscriptionId(hex_field(&response, "subscription")?),
            cursor: ResumeCursor(hex_field(&response, "cursor")?),
            sequence: 0,
            contents: BTreeMap::new(),
            state: State::Initial,
        })
    }

    pub async fn subscription_next(
        &self,
        subscription: &mut Subscription,
    ) -> Result<SubscriptionNotification, Error> {
        if subscription.state == State::Ended {
            return Ok(SubscriptionNotification::ResetRequired);
        }
        struct InitialRequest<'a>(&'a mut Subscription, bool);
        impl Drop for InitialRequest<'_> {
            fn drop(&mut self) {
                if self.1 && self.0.state == State::Initial {
                    self.0.end();
                }
            }
        }
        let mut request = InitialRequest(subscription, true);
        let response = self
            .call("resume", &encode_resume(request.0.id, request.0.cursor))
            .await;
        let result = response.and_then(|value| request.0.apply(value));
        if matches!(result, Err(Error::Http(401))) {
            request.0.end();
        }
        if result.is_ok() {
            request.1 = false;
        }
        result
    }

    pub async fn unsubscribe(&self, subscription: &mut Subscription) -> Result<(), Error> {
        self.call("unsubscribe", &subscription.id.0).await?;
        subscription.end();
        Ok(())
    }
}

fn decode_hex(value: &Value) -> Result<[u8; 32], Error> {
    protocol::unhex(value.as_str().ok_or(Error::InvalidResponse)?)
        .map_err(|_| Error::InvalidResponse)
}

fn hex_field(value: &Value, name: &str) -> Result<[u8; 32], Error> {
    decode_hex(value.get(name).ok_or(Error::InvalidResponse)?)
}

fn content_id(item: &Value) -> Result<ContentId, Error> {
    Ok(ContentId(hex_field(
        item.get("content").ok_or(Error::InvalidResponse)?,
        "id",
    )?))
}

fn take_contents(value: &mut Value, name: &str) -> Result<Contents, Error> {
    let Some(Value::Array(items)) = value.get_mut(name).map(Value::take) else {
        return Err(Error::InvalidResponse);
    };
    let mut contents = BTreeMap::new();
    for item in items {
        if contents.insert(content_id(&item)?, item).is_some() {
            return Err(Error::InvalidResponse);
        }
    }
    Ok(contents)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn late_notification_cannot_rewind_contents_or_checkpoint() {
        let id = ContentId([4; 32]);
        let view = |body: &str| json!({"content":{"id":protocol::hex(&id.0),"body":body}});
        let mut subscription = Subscription::from_checkpoint(SubscriptionCheckpoint {
            id: SubscriptionId([1; 32]),
            cursor: ResumeCursor([2; 32]),
            sequence: 0,
            contents: [(id, view("initial"))].into(),
        })
        .unwrap();
        for (sequence, cursor, body) in [("2", [9; 32], "new"), ("1", [8; 32], "old")] {
            subscription
                .apply(json!({
                    "type":"changes", "sequence":sequence, "cursor":protocol::hex(&cursor),
                    "upserts":[view(body)], "removes":[]
                }))
                .unwrap();
        }
        let checkpoint = subscription.checkpoint().unwrap();
        assert_eq!(checkpoint.sequence, 2);
        assert_eq!(checkpoint.cursor, ResumeCursor([9; 32]));
        assert_eq!(checkpoint.contents[&id], view("new"));
    }
}
