//! Reference semantics for evaluating a query predicate on one Content snapshot.

use crate::{
    Error,
    access::{Credential, Grants, authenticate},
    application::{Host, TokenPurpose, mint_token, view},
    storage::{Store, Transaction, read_content_graph},
};
use harmion_domain::{AttributeValue, ContentId, ContentSnapshot, UpdateId};
use harmion_protocol::{
    AppId, EditToken, MAX_APP_RESPONSE_BYTES, ReadView,
    query::{
        AttributePredicate, Condition, MAX_QUERY_PAGE_ITEMS, OrderOperator, OrderedValue,
        QueryCursor,
    },
    read_view_size,
};
use std::cmp::Ordering;
use std::collections::BTreeSet;

pub const QUERY_TTL_MICROS: i64 = 5 * 60 * 1_000_000;
pub const MAX_ACTIVE_QUERIES: u32 = 64;
pub const MAX_QUERY_VERSION_REFS: u32 = 1_000_000;

pub struct QueryPosition {
    pub session: QueryCursor,
    pub app: AppId,
    pub after: Option<ContentId>,
    pub expires_at: i64,
}

pub struct QueryRevision {
    pub content: ContentId,
    pub basis: BTreeSet<UpdateId>,
}

pub struct QueryPage {
    pub items: Vec<ReadView>,
    pub next: Option<QueryCursor>,
}

pub fn start(
    store: &mut impl Store,
    host: &mut impl Host,
    credential: &Credential,
    condition: &Condition,
    limit: u32,
) -> Result<QueryPage, Error> {
    validate_limit(limit)?;
    store.transaction(|tx| {
        let now = host.now_micros();
        let app = authenticate(tx, credential, now)?.app;
        let read = app.grants.read.as_ref().ok_or(Error::Forbidden)?;
        let session = QueryCursor(host.random_bytes()?);
        let expires_at = now.saturating_add(QUERY_TTL_MICROS).min(app.expires_at);
        tx.freeze_query(session, app.id, condition, read, now, expires_at)?;
        let result = page(
            tx,
            host,
            &app.grants,
            QueryPosition {
                session,
                app: app.id,
                after: None,
                expires_at,
            },
            limit,
        )?;
        if result.next.is_none() {
            tx.discard_query(session)?;
        }
        Ok(result)
    })
}

pub fn next(
    store: &mut impl Store,
    host: &mut impl Host,
    credential: &Credential,
    cursor: QueryCursor,
    limit: u32,
) -> Result<QueryPage, Error> {
    validate_limit(limit)?;
    store.transaction(|tx| {
        let now = host.now_micros();
        let app = authenticate(tx, credential, now)?.app;
        let position = tx
            .query_position(cursor)?
            .filter(|position| position.app == app.id && position.expires_at > now)
            .ok_or(Error::QueryExpired)?;
        if app.grants.read.is_none() {
            return Err(Error::Forbidden);
        }
        page(tx, host, &app.grants, position, limit)
    })
}

fn validate_limit(limit: u32) -> Result<(), Error> {
    if (1..=MAX_QUERY_PAGE_ITEMS).contains(&limit) {
        Ok(())
    } else {
        Err(Error::InvalidRequest)
    }
}

pub(crate) fn page(
    tx: &mut dyn Transaction,
    host: &mut impl Host,
    grants: &Grants,
    position: QueryPosition,
    limit: u32,
) -> Result<QueryPage, Error> {
    // Bound work per request even when a grant change hides every candidate.
    let revisions = tx.query_revisions(&position, limit + 1)?;
    let mut more = revisions.len() > limit as usize;
    let mut items = Vec::new();
    let mut after = position.after;
    // Reserve the envelope, next cursor, and commas between items.
    let mut remaining = MAX_APP_RESPONSE_BYTES - 256;
    for revision in revisions.into_iter().take(limit as usize) {
        let graph = read_content_graph(tx, revision.content)?;
        let current = graph.snapshot(revision.content).ok_or(Error::CorruptLog)?;
        if grants.can_read_snapshot(&current) {
            let frozen = graph.snapshot_at(revision.content, &revision.basis)?;
            if grants.can_read_snapshot(&frozen) {
                let mut item = view(&frozen, EditToken([0; 32]), grants);
                let Some(size) = read_view_size(&item, remaining) else {
                    if items.is_empty() {
                        return Err(Error::QueryItemTooLarge);
                    }
                    more = true;
                    break;
                };
                remaining -= size;
                item.edit_token = mint_token(tx, host, &frozen, TokenPurpose::Edit)?;
                items.push(item);
            }
        }
        after = Some(revision.content);
    }
    let next = if more {
        let after = after.ok_or(Error::CorruptLog)?;
        Some(tx.put_query_cursor(position.session, after, QueryCursor(host.random_bytes()?))?)
    } else {
        None
    };
    Ok(QueryPage { items, next })
}

/// The caller obtains the snapshot and current grants in the same transaction.
/// Storage adapters execute collection queries and must match these semantics.
pub fn matches(snapshot: &ContentSnapshot, grants: &Grants, condition: &Condition) -> bool {
    grants.can_read_snapshot(snapshot) && evaluate(snapshot, condition)
}

fn evaluate(snapshot: &ContentSnapshot, condition: &Condition) -> bool {
    match condition {
        Condition::And(conditions) => conditions.iter().all(|c| evaluate(snapshot, c)),
        Condition::Or(conditions) => conditions.iter().any(|c| evaluate(snapshot, c)),
        Condition::Not(condition) => !evaluate(snapshot, condition),
        Condition::Kind(kind) => snapshot.content.kinds.contains(kind),
        Condition::LastEditedAt {
            operator,
            utc_micros,
        } => matches_order(*operator, snapshot.last_edited_at.cmp(utc_micros)),
        Condition::Attribute { key, predicate } => snapshot
            .content
            .attributes
            .get(key)
            .is_some_and(|value| evaluate_attribute(value, predicate)),
    }
}

fn matches_order(operator: OrderOperator, order: Ordering) -> bool {
    match operator {
        OrderOperator::Less => order == Ordering::Less,
        OrderOperator::LessOrEqual => order != Ordering::Greater,
        OrderOperator::Greater => order == Ordering::Greater,
        OrderOperator::GreaterOrEqual => order != Ordering::Less,
    }
}

fn evaluate_attribute(value: &AttributeValue, predicate: &AttributePredicate) -> bool {
    match (value, predicate) {
        (_, AttributePredicate::Exists) => true,
        (_, AttributePredicate::Eq(expected)) => value == expected,
        (
            _,
            AttributePredicate::Compare {
                operator,
                value: expected,
            },
        ) => {
            let order = match (value, expected) {
                (AttributeValue::Number(value), OrderedValue::Number(expected)) => {
                    value.cmp(expected)
                }
                (AttributeValue::String(value), OrderedValue::String(expected)) => {
                    value.cmp(expected)
                }
                _ => return false,
            };
            matches_order(*operator, order)
        }
        (AttributeValue::Array(values), AttributePredicate::Contains(expected)) => {
            values.contains(expected)
        }
        (AttributeValue::Array(values), AttributePredicate::ContainsAny(expected)) => {
            expected.iter().any(|item| values.contains(item))
        }
        (AttributeValue::Array(values), AttributePredicate::ContainsAll(expected)) => {
            expected.iter().all(|item| values.contains(item))
        }
        _ => false,
    }
}
