//! Bounded node-local window of committed projection changes.

use crate::{db, fixed, seq};
use harmion_domain::ContentId;
use harmion_node::{
    Error,
    storage::{ChangeWindow, ContentChange, MAX_RETAINED_CHANGES},
};
use harmion_protocol::AppId;
use rusqlite::{Connection, OptionalExtension, params};

pub(super) fn initialize(sql: &Connection) -> Result<(), Error> {
    sql.execute_batch(
        "CREATE TABLE content_changes (
            seq INTEGER PRIMARY KEY AUTOINCREMENT,
            content BLOB NOT NULL CHECK(length(content)=32));
        CREATE TABLE grant_generations (
            app BLOB PRIMARY KEY NOT NULL REFERENCES apps(app_id),
            generation INTEGER NOT NULL CHECK(typeof(generation)='integer' AND generation>0));",
    )
    .map_err(db)
}

pub(super) fn record(sql: &Connection, content: ContentId) -> Result<(), Error> {
    sql.execute(
        "INSERT INTO content_changes(content) VALUES (?1)",
        [content.0],
    )
    .map_err(db)?;
    let high = u64::try_from(sql.last_insert_rowid()).map_err(|_| Error::CorruptLog)?;
    if high > MAX_RETAINED_CHANGES {
        sql.execute(
            "DELETE FROM content_changes WHERE seq<=?1",
            [seq(high - MAX_RETAINED_CHANGES)?],
        )
        .map_err(db)?;
    }
    Ok(())
}

pub(super) fn window(sql: &Connection) -> Result<ChangeWindow, Error> {
    let high: i64 = sql
        .query_row(
            "SELECT COALESCE(MAX(seq),0) FROM content_changes",
            [],
            |row| row.get(0),
        )
        .map_err(db)?;
    let high = u64::try_from(high).map_err(|_| Error::CorruptLog)?;
    Ok(ChangeWindow {
        discarded_through: high.saturating_sub(MAX_RETAINED_CHANGES),
        high,
    })
}

pub(super) fn after(sql: &Connection, after: u64, limit: u32) -> Result<Vec<ContentChange>, Error> {
    let mut statement = sql
        .prepare_cached(
            "SELECT seq,content FROM content_changes WHERE seq>?1 ORDER BY seq LIMIT ?2",
        )
        .map_err(db)?;
    let mut rows = statement.query(params![seq(after)?, limit]).map_err(db)?;
    let mut changes = Vec::new();
    while let Some(row) = rows.next().map_err(db)? {
        changes.push(ContentChange {
            sequence: u64::try_from(row.get::<_, i64>(0).map_err(db)?)
                .map_err(|_| Error::CorruptLog)?,
            content: ContentId(fixed(row.get(1).map_err(db)?)?),
        });
    }
    Ok(changes)
}

pub(super) fn advance_grants(sql: &Connection, app: AppId) -> Result<(), Error> {
    sql.execute(
        "INSERT INTO grant_generations VALUES (?1,1)
         ON CONFLICT(app) DO UPDATE SET generation=generation+1",
        [app.0],
    )
    .map_err(db)?;
    Ok(())
}

pub(super) fn grant_generation(sql: &Connection, app: AppId) -> Result<u64, Error> {
    let generation: i64 = sql
        .query_row(
            "SELECT generation FROM grant_generations WHERE app=?1",
            [app.0],
            |row| row.get(0),
        )
        .optional()
        .map_err(db)?
        .unwrap_or(0);
    u64::try_from(generation).map_err(|_| Error::CorruptLog)
}
