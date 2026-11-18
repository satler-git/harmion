//! Materialized fixed Query results and their continuation positions.

use crate::{db, fixed, query};
use harmion_domain::{ContentId, UpdateId};
use harmion_node::{
    Error,
    access::Filter,
    query::{MAX_ACTIVE_QUERIES, MAX_QUERY_VERSION_REFS, QueryPosition, QueryRevision},
};
use harmion_protocol::{
    AppId,
    query::{Condition, QueryCursor},
};
use rusqlite::{Connection, OptionalExtension, params, params_from_iter, types::Value};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn initialize(sql: &Connection) -> Result<(), Error> {
    sql.execute_batch(
        "CREATE TABLE query_sessions (
            session BLOB PRIMARY KEY NOT NULL CHECK(length(session)=32),
            app BLOB NOT NULL REFERENCES apps(app_id), expires_at INTEGER NOT NULL,
            version_refs INTEGER NOT NULL DEFAULT 0 CHECK(version_refs>=0));
        CREATE INDEX query_expiry ON query_sessions(expires_at);
        CREATE TABLE query_basis (
            session BLOB NOT NULL REFERENCES query_sessions(session) ON DELETE CASCADE,
            content BLOB NOT NULL CHECK(length(content)=32),
            update_id BLOB NOT NULL REFERENCES accept_log(update_id),
            PRIMARY KEY(session,content,update_id));
        CREATE TABLE query_cursors (
            cursor BLOB PRIMARY KEY NOT NULL CHECK(length(cursor)=32),
            session BLOB NOT NULL REFERENCES query_sessions(session) ON DELETE CASCADE,
            after_content BLOB NOT NULL CHECK(length(after_content)=32),
            UNIQUE(session,after_content));",
    )
    .map_err(db)
}

pub(super) fn freeze(
    sql: &Connection,
    session: QueryCursor,
    app: AppId,
    condition: &Condition,
    read: &Filter,
    now: i64,
    expires_at: i64,
) -> Result<(), Error> {
    sql.execute("DELETE FROM query_sessions WHERE expires_at<=?1", [now])
        .map_err(db)?;
    let (count, occupied): (u32, u32) = sql
        .query_row(
            "SELECT COUNT(*),COALESCE(SUM(version_refs),0) FROM query_sessions",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(db)?;
    if count >= MAX_ACTIVE_QUERIES {
        return Err(Error::QueryCapacity);
    }
    let available = MAX_QUERY_VERSION_REFS.saturating_sub(occupied);
    let (constraints, bindings) = query::search_constraints(condition, read, Some(app))?;
    sql.execute(
        "INSERT INTO query_sessions(session,app,expires_at) VALUES (?1,?2,?3)",
        params![session.0, app.0, expires_at],
    )
    .map_err(db)?;
    let used = sql.execute(
        &format!(
            "INSERT INTO query_basis SELECT ?,content,update_id FROM search_basis
                  WHERE content IN (SELECT content FROM search_contents WHERE {constraints}) LIMIT ?"
        ),
        params_from_iter(std::iter::once(Value::Blob(session.0.to_vec())).chain(bindings)
            .chain(std::iter::once(Value::Integer(i64::from(available) + 1)))),
    )
    .map_err(db)?;
    if used > available as usize {
        return Err(Error::QueryCapacity);
    }
    sql.execute(
        "UPDATE query_sessions SET version_refs=?1 WHERE session=?2",
        params![
            i64::try_from(used).map_err(|_| Error::QueryCapacity)?,
            session.0
        ],
    )
    .map_err(db)?;
    Ok(())
}

pub(super) fn position(
    sql: &Connection,
    cursor: QueryCursor,
) -> Result<Option<QueryPosition>, Error> {
    type CursorRow = (Vec<u8>, Vec<u8>, Vec<u8>, i64);
    let row: Option<CursorRow> = sql
        .query_row(
            "SELECT c.session,s.app,c.after_content,s.expires_at
         FROM query_cursors c JOIN query_sessions s ON s.session=c.session WHERE c.cursor=?1",
            [cursor.0],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(db)?;
    row.map(|(session, app, after, expires_at)| {
        Ok(QueryPosition {
            session: QueryCursor(fixed(session)?),
            app: AppId(fixed(app)?),
            after: Some(ContentId(fixed(after)?)),
            expires_at,
        })
    })
    .transpose()
}

pub(super) fn discard(sql: &Connection, session: QueryCursor) -> Result<(), Error> {
    sql.execute("DELETE FROM query_sessions WHERE session=?1", [session.0])
        .map_err(db)?;
    Ok(())
}

pub(super) fn revisions(
    sql: &Connection,
    position: &QueryPosition,
    limit: u32,
) -> Result<Vec<QueryRevision>, Error> {
    const FIRST: &str = "WITH page AS (
            SELECT DISTINCT content FROM query_basis WHERE session=?1
            ORDER BY content LIMIT ?2
        )
        SELECT p.content,b.update_id FROM page p JOIN query_basis b ON b.session=?1 AND b.content=p.content
        ORDER BY p.content,b.update_id";
    const AFTER: &str = "WITH page AS (
            SELECT DISTINCT content FROM query_basis WHERE session=?1 AND content>?3
            ORDER BY content LIMIT ?2
        )
        SELECT p.content,b.update_id FROM page p JOIN query_basis b ON b.session=?1 AND b.content=p.content
        ORDER BY p.content,b.update_id";
    let mut statement;
    let mut rows = if let Some(after) = position.after {
        statement = sql.prepare_cached(AFTER).map_err(db)?;
        statement
            .query(params![position.session.0, limit, after.0])
            .map_err(db)?
    } else {
        statement = sql.prepare_cached(FIRST).map_err(db)?;
        statement
            .query(params![position.session.0, limit])
            .map_err(db)?
    };
    let mut page: BTreeMap<ContentId, BTreeSet<UpdateId>> = BTreeMap::new();
    while let Some(row) = rows.next().map_err(db)? {
        let content = ContentId(fixed(row.get(0).map_err(db)?)?);
        let update = UpdateId(fixed(row.get(1).map_err(db)?)?);
        page.entry(content).or_default().insert(update);
    }
    Ok(page
        .into_iter()
        .map(|(content, basis)| QueryRevision { content, basis })
        .collect())
}

pub(super) fn put_cursor(
    sql: &Connection,
    session: QueryCursor,
    after: ContentId,
    cursor: QueryCursor,
) -> Result<QueryCursor, Error> {
    sql.execute(
        "INSERT INTO query_cursors VALUES (?1,?2,?3) ON CONFLICT(session,after_content) DO NOTHING",
        params![cursor.0, session.0, after.0],
    )
    .map_err(db)?;
    let stored = sql
        .query_row(
            "SELECT cursor FROM query_cursors WHERE session=?1 AND after_content=?2",
            params![session.0, after.0],
            |row| row.get(0),
        )
        .map_err(db)?;
    Ok(QueryCursor(fixed(stored)?))
}
