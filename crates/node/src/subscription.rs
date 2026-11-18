//! Subscription sessions and acknowledgement logic, independent of transport.

use crate::{
    Error,
    access::{Credential, Grants, authenticate},
    application::{Host, TokenPurpose, mint_token, view},
    query::{self, QUERY_TTL_MICROS, QueryPosition},
    storage::{Store, Transaction, read_content_graph},
};
use harmion_domain::ContentId;
use harmion_protocol::{
    AppId, EditToken, MAX_APP_RESPONSE_BYTES, ReadView,
    query::{Condition, MAX_QUERY_PAGE_ITEMS, QueryCursor},
    read_view_size,
    subscription::{Event, ResumeCursor, SubscriptionId},
};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_SUBSCRIPTIONS: usize = 16;
pub const MAX_VISIBLE_CONTENTS: usize = 100_000;
pub const SUBSCRIPTION_IDLE_MICROS: i64 = 5 * 60 * 1_000_000;

#[derive(Default)]
pub struct Subscriptions {
    sessions: BTreeMap<SubscriptionId, Session>,
}

struct Session {
    app: AppId,
    grant_generation: u64,
    condition: Condition,
    expiry: i64,
    snapshot: QueryCursor,
    visible: BTreeSet<ContentId>,
    sequence: u64,
    phase: Phase,
}

enum Phase {
    Snapshot {
        position: QueryPosition,
        through: u64,
        cursor: ResumeCursor,
    },
    End {
        through: u64,
        cursor: ResumeCursor,
    },
    Live {
        acknowledged: Position,
        pending: Option<Delivery>,
    },
}

#[derive(Clone, Copy)]
struct Position {
    through: u64,
    cursor: ResumeCursor,
}

struct Delivery {
    position: Position,
    changes: BTreeMap<ContentId, bool>,
}

enum Step {
    Chunk {
        items: Vec<ReadView>,
        cursor: ResumeCursor,
        position: Option<QueryPosition>,
        through: u64,
    },
    End(Position),
    Changes {
        acknowledged_pending: bool,
        delivery: Delivery,
        upserts: Vec<ReadView>,
        removes: Vec<ContentId>,
    },
    Idle {
        acknowledged_pending: bool,
    },
    Reset,
    UnknownCursor,
}

impl Subscriptions {
    pub fn start(
        &mut self,
        store: &mut impl Store,
        host: &mut impl Host,
        credential: &Credential,
        condition: Condition,
    ) -> Result<Event, Error> {
        let now = host.now_micros();
        self.sessions.retain(|_, session| session.expiry > now);
        let (id, session, cursor) = store.transaction(|tx| {
            let app = authenticate(tx, credential, now)?.app;
            let read = app.grants.read.as_ref().ok_or(Error::Forbidden)?;
            if self.sessions.len() >= MAX_SUBSCRIPTIONS {
                return Err(Error::SubscriptionCapacity);
            }
            let id = SubscriptionId(host.random_bytes()?);
            if self.sessions.contains_key(&id) {
                return Err(Error::Entropy("subscription ID collision".into()));
            }
            let cursor = ResumeCursor(host.random_bytes()?);
            let snapshot = QueryCursor(host.random_bytes()?);
            let expiry = now.saturating_add(QUERY_TTL_MICROS).min(app.expires_at);
            let through = tx.change_window()?.high;
            let grant_generation = tx.grant_generation(app.id)?;
            tx.freeze_query(snapshot, app.id, &condition, read, now, expiry)?;
            Ok((
                id,
                Session {
                    app: app.id,
                    grant_generation,
                    condition,
                    expiry,
                    snapshot,
                    visible: BTreeSet::new(),
                    sequence: 0,
                    phase: Phase::Snapshot {
                        position: QueryPosition {
                            session: snapshot,
                            app: app.id,
                            after: None,
                            expires_at: expiry,
                        },
                        through,
                        cursor,
                    },
                },
                cursor,
            ))
        })?;
        self.sessions.insert(id, session);
        Ok(Event::SnapshotBegin {
            subscription: id,
            cursor,
        })
    }

    /// `None` means there is no newer change to deliver yet.
    pub fn resume(
        &mut self,
        store: &mut impl Store,
        host: &mut impl Host,
        credential: &Credential,
        id: SubscriptionId,
        cursor: ResumeCursor,
    ) -> Result<Option<Event>, Error> {
        let now = host.now_micros();
        let session = self.sessions.get_mut(&id);
        let (step, expires_at) = store.transaction(|tx| {
            let app = authenticate(tx, credential, now)?.app;
            let Some(session) = session.as_deref().filter(|session| session.app == app.id) else {
                return Ok((Step::UnknownCursor, app.expires_at));
            };
            if session.expiry <= now || session.grant_generation != tx.grant_generation(app.id)? {
                tx.discard_query(session.snapshot)?;
                return Ok((Step::Reset, app.expires_at));
            }
            let step = session.step(tx, host, &app.grants, cursor)?;
            if matches!(step, Step::Reset) {
                tx.discard_query(session.snapshot)?;
            }
            Ok((step, app.expires_at))
        })?;
        if matches!(step, Step::UnknownCursor) {
            return Ok(Some(Event::ResetRequired));
        }
        if matches!(step, Step::Reset) {
            self.sessions.remove(&id);
            return Ok(Some(Event::ResetRequired));
        }
        let session = session.ok_or(Error::CorruptLog)?;
        let event = session.apply(step);
        if matches!(session.phase, Phase::Live { .. }) {
            session.expiry = now.saturating_add(SUBSCRIPTION_IDLE_MICROS).min(expires_at);
        }
        Ok(event)
    }

    pub fn unsubscribe(
        &mut self,
        store: &mut impl Store,
        host: &impl Host,
        credential: &Credential,
        id: SubscriptionId,
    ) -> Result<(), Error> {
        let remove = store.transaction(|tx| {
            let app = authenticate(tx, credential, host.now_micros())?.app;
            if let Some(session) = self
                .sessions
                .get(&id)
                .filter(|session| session.app == app.id)
            {
                tx.discard_query(session.snapshot)?;
                Ok(true)
            } else {
                Ok(false)
            }
        })?;
        if remove {
            self.sessions.remove(&id);
        }
        Ok(())
    }
}

impl Session {
    fn step(
        &self,
        tx: &mut dyn Transaction,
        host: &mut impl Host,
        grants: &Grants,
        requested: ResumeCursor,
    ) -> Result<Step, Error> {
        let window = tx.change_window()?;
        match &self.phase {
            Phase::Snapshot {
                position,
                through,
                cursor,
            } if *cursor == requested => {
                if *through < window.discarded_through {
                    return Ok(Step::Reset);
                }
                let position = QueryPosition { ..*position };
                let page = query::page(tx, host, grants, position, MAX_QUERY_PAGE_ITEMS)?;
                if self.visible.len() + page.items.len() > MAX_VISIBLE_CONTENTS {
                    return Ok(Step::Reset);
                }
                let position = match page.next {
                    Some(next) => Some(tx.query_position(next)?.ok_or(Error::CorruptLog)?),
                    None => {
                        tx.discard_query(self.snapshot)?;
                        None
                    }
                };
                Ok(Step::Chunk {
                    items: page.items,
                    cursor: ResumeCursor(host.random_bytes()?),
                    position,
                    through: *through,
                })
            }
            Phase::End { through, cursor } if *cursor == requested => {
                if *through < window.discarded_through {
                    return Ok(Step::Reset);
                }
                Ok(Step::End(Position {
                    through: *through,
                    cursor: ResumeCursor(host.random_bytes()?),
                }))
            }
            Phase::Live {
                acknowledged,
                pending,
            } => {
                let acknowledgement = pending.as_ref().filter(|p| p.position.cursor == requested);
                let position = match acknowledgement {
                    Some(pending) => pending.position,
                    None if acknowledged.cursor == requested => *acknowledged,
                    None => return Ok(Step::UnknownCursor),
                };
                if position.through < window.discarded_through || position.through > window.high {
                    return Ok(Step::Reset);
                }
                self.collect_changes(tx, host, grants, position, acknowledgement)
            }
            _ => Ok(Step::UnknownCursor),
        }
    }

    fn collect_changes(
        &self,
        tx: &mut dyn Transaction,
        host: &mut impl Host,
        grants: &Grants,
        position: Position,
        acknowledgement: Option<&Delivery>,
    ) -> Result<Step, Error> {
        let batch = tx.changes_after(position.through, MAX_QUERY_PAGE_ITEMS)?;
        let acknowledged_pending = acknowledgement.is_some();
        if batch.is_empty() {
            return Ok(Step::Idle {
                acknowledged_pending,
            });
        }
        if self.sequence == u64::MAX {
            return Ok(Step::Reset);
        }
        let mut seen = BTreeSet::new();
        let mut changes = BTreeMap::new();
        let mut upserts = Vec::new();
        let mut removes = Vec::new();
        let mut remaining = MAX_APP_RESPONSE_BYTES - 512;
        let mut through = position.through;
        let mut visible_count = self.visible.len();
        if let Some(acknowledgement) = acknowledgement {
            for (id, visible) in &acknowledgement.changes {
                visible_count = count_visible(visible_count, self.visible.contains(id), *visible);
            }
        }
        for change in batch {
            if !seen.insert(change.content) {
                through = change.sequence;
                continue;
            }
            let previous = acknowledgement
                .and_then(|delivery| delivery.changes.get(&change.content).copied())
                .unwrap_or_else(|| self.visible.contains(&change.content));
            let graph = read_content_graph(tx, change.content)?;
            let snapshot = graph.snapshot(change.content).ok_or(Error::CorruptLog)?;
            let visible = query::matches(&snapshot, grants, &self.condition);
            if visible {
                let mut item = view(&snapshot, EditToken([0; 32]), grants);
                let Some(size) = read_view_size(&item, remaining) else {
                    if upserts.is_empty() && removes.is_empty() {
                        return Err(Error::QueryItemTooLarge);
                    }
                    break;
                };
                remaining -= size;
                item.edit_token = mint_token(tx, host, &snapshot, TokenPurpose::Edit)?;
                upserts.push(item);
            } else if previous {
                // A remove is only meaningful for a Content the client could already see.
                if remaining < 68 {
                    break;
                }
                remaining -= 68;
                removes.push(change.content);
            }
            if visible != previous {
                changes.insert(change.content, visible);
                visible_count = count_visible(visible_count, previous, visible);
            }
            through = change.sequence;
        }
        if visible_count > MAX_VISIBLE_CONTENTS {
            return Ok(Step::Reset);
        }
        Ok(Step::Changes {
            acknowledged_pending,
            delivery: Delivery {
                position: Position {
                    through,
                    cursor: ResumeCursor(host.random_bytes()?),
                },
                changes,
            },
            upserts,
            removes,
        })
    }

    fn acknowledge(&mut self, accept_pending: bool) {
        if let Phase::Live {
            acknowledged,
            pending,
        } = &mut self.phase
            && accept_pending
            && let Some(delivery) = pending.take()
        {
            for (id, visible) in delivery.changes {
                if visible {
                    self.visible.insert(id);
                } else {
                    self.visible.remove(&id);
                }
            }
            *acknowledged = delivery.position;
        }
    }

    fn apply(&mut self, step: Step) -> Option<Event> {
        match step {
            Step::Chunk {
                items,
                cursor,
                position,
                through,
            } => {
                self.visible
                    .extend(items.iter().map(|item| item.content.id));
                self.phase = match position {
                    Some(position) => Phase::Snapshot {
                        position,
                        cursor,
                        through,
                    },
                    None => Phase::End { through, cursor },
                };
                Some(Event::SnapshotChunk { items, cursor })
            }
            Step::End(position) => {
                self.phase = Phase::Live {
                    acknowledged: position,
                    pending: None,
                };
                Some(Event::SnapshotEnd {
                    cursor: position.cursor,
                })
            }
            Step::Changes {
                acknowledged_pending,
                delivery,
                upserts,
                removes,
            } => {
                self.acknowledge(acknowledged_pending);
                let cursor = delivery.position.cursor;
                if let Phase::Live { pending, .. } = &mut self.phase {
                    *pending = Some(delivery);
                }
                self.sequence += 1;
                Some(Event::Changes {
                    sequence: self.sequence,
                    upserts,
                    removes,
                    cursor,
                })
            }
            Step::Idle {
                acknowledged_pending,
            } => {
                self.acknowledge(acknowledged_pending);
                None
            }
            Step::Reset | Step::UnknownCursor => Some(Event::ResetRequired),
        }
    }
}

fn count_visible(count: usize, before: bool, after: bool) -> usize {
    count + usize::from(after) - usize::from(before)
}
