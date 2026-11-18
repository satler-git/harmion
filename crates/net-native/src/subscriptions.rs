//! App HTTPS subscription requests with bounded long polling.

use crate::{Error, SharedStore, SystemHost};
use harmion_node::{access::Credential, storage::Store, subscription::Subscriptions};
use harmion_protocol::{
    query::{MAX_QUERY_PAGE_ITEMS, QueryRequest, decode_request},
    subscription::{ResumeCursor, SubscriptionId, event_json},
};
use serde_json::{Value, json};
use std::{sync::Mutex, time::Duration};
use tokio::{
    sync::Semaphore,
    time::{Instant, timeout_at},
};

pub(crate) struct Service {
    sessions: Mutex<Subscriptions>,
    waiters: Semaphore,
}

impl Service {
    pub(crate) fn new() -> Self {
        Self {
            sessions: Mutex::new(Subscriptions::default()),
            // Keep some App connection slots available for writes and management.
            waiters: Semaphore::new(8),
        }
    }

    fn sessions(&self) -> Result<std::sync::MutexGuard<'_, Subscriptions>, harmion_node::Error> {
        self.sessions
            .lock()
            .map_err(|_| harmion_node::Error::Storage("subscription sessions poisoned".into()))
    }

    pub(crate) async fn handle<S: Store + Send + 'static>(
        self: std::sync::Arc<Self>,
        store: &SharedStore<S>,
        credential: Credential,
        method: &str,
        body: &[u8],
    ) -> Result<Value, Error> {
        let invalid = || harmion_node::Error::InvalidRequest;
        match method {
            "subscribe" => {
                let QueryRequest::Start {
                    condition,
                    limit: MAX_QUERY_PAGE_ITEMS,
                } = decode_request(body)?
                else {
                    return Err(invalid().into());
                };
                let event = store
                    .run(move |db| {
                        self.sessions()?
                            .start(db, &mut SystemHost, &credential, condition)
                    })
                    .await?;
                Ok(event_json(&event))
            }
            "unsubscribe" => {
                let id = SubscriptionId(body.try_into().map_err(|_| invalid())?);
                store
                    .run(move |db| {
                        self.sessions()?
                            .unsubscribe(db, &SystemHost, &credential, id)
                    })
                    .await?;
                Ok(json!({"unsubscribed":true}))
            }
            "resume" => {
                let (id, cursor) = body.split_at_checked(32).ok_or_else(invalid)?;
                let id = SubscriptionId(id.try_into().map_err(|_| invalid())?);
                let cursor = ResumeCursor(cursor.try_into().map_err(|_| invalid())?);
                self.resume(store, credential, id, cursor).await
            }
            _ => Err(invalid().into()),
        }
    }

    async fn resume<S: Store + Send + 'static>(
        self: std::sync::Arc<Self>,
        store: &SharedStore<S>,
        credential: Credential,
        id: SubscriptionId,
        cursor: ResumeCursor,
    ) -> Result<Value, Error> {
        let mut changes = store.changes.subscribe();
        let deadline = Instant::now() + Duration::from_secs(20);
        let slot = self.waiters.try_acquire().ok();
        loop {
            let service = self.clone();
            let credential = credential.clone();
            let event = store
                .run(move |db| {
                    service
                        .sessions()?
                        .resume(db, &mut SystemHost, &credential, id, cursor)
                })
                .await?;
            if let Some(event) = event {
                return Ok(event_json(&event));
            }
            if slot.is_none() || Instant::now() >= deadline {
                return Ok(json!({"type":"idle"}));
            }
            let _ = timeout_at(deadline, changes.changed()).await;
        }
    }
}
