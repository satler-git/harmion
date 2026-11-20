//! Native SQLite adapter for the node transaction port.

mod changes;
mod oauth;
mod query;
mod query_session;

use harmion_domain::{ClusterId, ContentId, ContentSnapshot, NodeId, UpdateId};
use harmion_node::membership::{Peer, PeerKey};
use harmion_node::{
    Error,
    access::{App, Filter, Grants},
    application::{RequestResult, TokenPurpose, TokenState},
    query::{QueryPosition, QueryRevision},
    storage::{ChangeWindow, ContentChange, Identity, Store, Transaction},
};
use harmion_protocol::membership::Certificate;
use harmion_protocol::{
    AppId, EditToken, LogEntry, LogEpoch, Record, RequestId,
    query::{Condition, QueryCursor},
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::{collections::BTreeSet, path::Path, time::Duration};

fn db(e: rusqlite::Error) -> Error {
    Error::Storage(e.to_string())
}
fn fixed(bytes: Vec<u8>) -> Result<[u8; 32], Error> {
    bytes.try_into().map_err(|_| Error::CorruptLog)
}
fn seq(value: u64) -> Result<i64, Error> {
    value.try_into().map_err(|_| Error::InvalidPage)
}

fn verify_identity(sql: &Connection, identity: Identity) -> Result<(), Error> {
    let (cluster, node, epoch) = sql
        .query_row(
            "SELECT cluster,node,epoch FROM identity WHERE singleton=1",
            [],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            },
        )
        .map_err(db)?;
    let stored = Identity {
        cluster: ClusterId(fixed(cluster)?),
        node: NodeId(fixed(node)?),
        epoch: LogEpoch(fixed(epoch)?),
    };
    if stored != identity {
        return Err(Error::IdentityMismatch);
    }
    Ok(())
}

pub struct SqliteStore {
    connection: Connection,
    identity: Identity,
}

impl SqliteStore {
    /// Host supplies cryptographically random IDs for a new DB and the same IDs
    /// when reopening. Existing metadata is never silently replaced.
    pub fn open(path: impl AsRef<Path>, identity: Identity) -> Result<Self, Error> {
        let mut connection = Connection::open(path).map_err(db)?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(db)?;
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;",
            )
            .map_err(db)?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db)?;
        let version: i64 = tx
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(db)?;
        if !(0..=9).contains(&version) {
            return Err(Error::Storage(format!(
                "unsupported schema version {version}"
            )));
        }
        if version == 0 {
            tx.execute_batch("
                CREATE TABLE identity (singleton INTEGER PRIMARY KEY CHECK(singleton=1),
                    cluster BLOB NOT NULL CHECK(length(cluster)=32), node BLOB NOT NULL CHECK(length(node)=32),
                    epoch BLOB NOT NULL CHECK(length(epoch)=32));
                CREATE TABLE accept_log (seq INTEGER PRIMARY KEY AUTOINCREMENT,
                    update_id BLOB NOT NULL UNIQUE CHECK(length(update_id)=32),
                    bytes BLOB NOT NULL, digest BLOB NOT NULL CHECK(length(digest)=32));
                CREATE TABLE peer_cursors (sender BLOB NOT NULL CHECK(length(sender)=32),
                    epoch BLOB NOT NULL CHECK(length(epoch)=32), through INTEGER NOT NULL CHECK(through>=0),
                    PRIMARY KEY(sender,epoch));
                PRAGMA user_version=1;
            ").map_err(db)?;
            tx.execute(
                "INSERT INTO identity VALUES(1,?1,?2,?3)",
                params![identity.cluster.0, identity.node.0, identity.epoch.0],
            )
            .map_err(db)?;
        }
        if version < 2 {
            tx.execute_batch("
                CREATE TABLE apps (app_id BLOB PRIMARY KEY CHECK(length(app_id)=32),
                    verifier BLOB UNIQUE NOT NULL CHECK(length(verifier)=32), expires_at INTEGER NOT NULL,
                    revoked INTEGER NOT NULL CHECK(revoked IN (0,1)),
                    read_all INTEGER CHECK(read_all IN (0,1)), write_all INTEGER CHECK(write_all IN (0,1)),
                    manage_apps INTEGER NOT NULL CHECK(manage_apps IN (0,1)),
                    manage_nodes INTEGER NOT NULL CHECK(manage_nodes IN (0,1)));
                CREATE TABLE app_kinds (app_id BLOB NOT NULL REFERENCES apps(app_id),
                    permission INTEGER NOT NULL CHECK(permission IN (0,1)), kind TEXT NOT NULL,
                    PRIMARY KEY(app_id,permission,kind));
                CREATE TABLE edit_tokens (token BLOB PRIMARY KEY CHECK(length(token)=32),
                    content BLOB NOT NULL CHECK(length(content)=32),
                    purpose INTEGER NOT NULL CHECK(purpose IN (0,1,2)));
                CREATE TABLE token_basis (token BLOB NOT NULL REFERENCES edit_tokens(token),
                    update_id BLOB NOT NULL REFERENCES accept_log(update_id), PRIMARY KEY(token,update_id));
                CREATE TABLE request_results (app_id BLOB NOT NULL REFERENCES apps(app_id),
                    request_id BLOB NOT NULL CHECK(length(request_id)=32), request BLOB NOT NULL,
                    update_id BLOB NOT NULL REFERENCES accept_log(update_id),
                    snapshot_token BLOB NOT NULL REFERENCES edit_tokens(token),
                    return_view INTEGER NOT NULL CHECK(return_view IN (0,1)),
                    PRIMARY KEY(app_id,request_id));
                PRAGMA user_version=2;
            ").map_err(db)?;
        }
        if version < 3 {
            tx.execute_batch(
                "CREATE TABLE peers (node BLOB PRIMARY KEY CHECK(length(node)=32),
                certificate BLOB NOT NULL UNIQUE, endpoint TEXT NOT NULL,
                retired INTEGER NOT NULL CHECK(retired IN (0,1))); PRAGMA user_version=3;",
            )
            .map_err(db)?;
        }
        if version < 4 {
            tx.execute_batch(
                "ALTER TABLE peers RENAME TO peers_v3;
                CREATE TABLE peers (node BLOB PRIMARY KEY CHECK(length(node)=32),
                    key_kind INTEGER NOT NULL CHECK(key_kind IN (0,1)), key_bytes BLOB NOT NULL,
                    endpoint TEXT NOT NULL, retired INTEGER NOT NULL CHECK(retired IN (0,1)),
                    UNIQUE(key_kind,key_bytes), CHECK(key_kind=0 OR length(key_bytes)=32));
                INSERT INTO peers SELECT node,0,certificate,endpoint,retired FROM peers_v3;
                DROP TABLE peers_v3; PRAGMA user_version=4;",
            )
            .map_err(db)?;
        }
        if version < 5 {
            tx.execute_batch("CREATE TABLE membership_root (singleton INTEGER PRIMARY KEY CHECK(singleton=1), bytes BLOB NOT NULL);
                CREATE TABLE membership_log (seq INTEGER PRIMARY KEY AUTOINCREMENT, id BLOB NOT NULL UNIQUE CHECK(length(id)=32), bytes BLOB NOT NULL);
                CREATE TABLE membership_cursors (sender BLOB NOT NULL CHECK(length(sender)=32), epoch BLOB NOT NULL CHECK(length(epoch)=32), through INTEGER NOT NULL CHECK(through>=0), PRIMARY KEY(sender,epoch));
                CREATE TABLE membership_recovery_roots (id BLOB PRIMARY KEY CHECK(length(id)=32), bytes BLOB NOT NULL);
                CREATE TABLE membership_identity (singleton INTEGER PRIMARY KEY CHECK(singleton=1), bytes BLOB NOT NULL);
                PRAGMA user_version=5;").map_err(db)?;
        }
        verify_identity(&tx, identity)?;
        // Reject broken bytes/dependencies at startup; never salvage only the normal value.
        let mut storage = SqlTransaction { sql: &tx, identity };
        let graph = harmion_node::storage::read_graph(&mut storage)?;
        harmion_node::membership_state::state(&mut storage)?;
        if version < 6 {
            query::initialize(&tx)?;
        }
        if version < 7 {
            query::initialize_version_refs(&tx, &graph)?;
            query_session::initialize(&tx)?;
        }
        if version < 8 {
            changes::initialize(&tx)?;
        }
        if version < 9 {
            oauth::initialize(&tx)?;
            tx.pragma_update(None, "user_version", 9).map_err(db)?;
        }
        tx.commit().map_err(db)?;
        Ok(Self {
            connection,
            identity,
        })
    }

    pub fn identity(&self) -> Identity {
        self.identity
    }

    /// Restore Content records into a fresh identity. The caller must acquire
    /// the owner-protected backup's daemon lock before opening it.
    pub fn restore_backup(
        &mut self,
        path: impl AsRef<Path>,
        identity: Identity,
    ) -> Result<(), Error> {
        if self.identity.cluster != identity.cluster || self.identity.node == identity.node {
            return Err(Error::IdentityMismatch);
        }
        let mut connection =
            Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(db)?;
        let tx = connection.transaction().map_err(db)?;
        let version: i64 = tx
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(db)?;
        if !(1..=9).contains(&version) {
            return Err(Error::Storage(format!(
                "unsupported schema version {version}"
            )));
        }
        verify_identity(&tx, identity)?;
        let mut source = SqlTransaction { sql: &tx, identity };
        let high = source.accepted_high()?;
        self.transaction(|destination| {
            if destination.accepted_high()? != 0 {
                return Err(Error::InvalidRequest);
            }
            let mut after = 0;
            while after < high {
                let entries = source.accepted_page(after, high)?;
                after = entries.last().ok_or(Error::CorruptLog)?.seq;
                let records: Vec<_> = entries.into_iter().map(|entry| entry.record).collect();
                harmion_node::storage::accept_records(destination, &records)?;
            }
            Ok(())
        })
    }
}

struct SqlTransaction<'a> {
    sql: &'a rusqlite::Transaction<'a>,
    identity: Identity,
}

impl SqlTransaction<'_> {
    fn read_entries(
        &self,
        sql: &str,
        params: impl rusqlite::Params,
    ) -> Result<Vec<LogEntry>, Error> {
        let mut statement = self.sql.prepare_cached(sql).map_err(db)?;
        let mut rows = statement.query(params).map_err(db)?;
        let mut entries = Vec::new();
        while let Some(row) = rows.next().map_err(db)? {
            let sequence: i64 = row.get(0).map_err(db)?;
            let id: Vec<u8> = row.get(1).map_err(db)?;
            let bytes: Vec<u8> = row.get(2).map_err(db)?;
            let digest: Vec<u8> = row.get(3).map_err(db)?;
            let record = Record::from_bytes(&bytes)?;
            if sequence <= 0
                || fixed(id)? != record.update().update_id.0
                || fixed(digest)? != *record.digest()
            {
                return Err(Error::CorruptLog);
            }
            entries.push(LogEntry {
                seq: sequence as u64,
                record,
            });
        }
        Ok(entries)
    }

    fn filter(
        &self,
        app: AppId,
        permission: i64,
        all: Option<i64>,
    ) -> Result<Option<Filter>, Error> {
        match all {
            None => Ok(None),
            Some(1) => Ok(Some(Filter::All)),
            Some(0) => {
                let mut statement = self.sql.prepare("SELECT kind FROM app_kinds WHERE app_id=?1 AND permission=?2 ORDER BY kind").map_err(db)?;
                let kinds = statement
                    .query_map(params![app.0, permission], |r| r.get::<_, String>(0))
                    .map_err(db)?
                    .collect::<Result<BTreeSet<_>, _>>()
                    .map_err(db)?;
                Ok(Some(Filter::Kinds(kinds)))
            }
            _ => Err(Error::CorruptLog),
        }
    }
    fn write_kinds(&self, app: &App) -> Result<(), Error> {
        self.sql
            .execute("DELETE FROM app_kinds WHERE app_id=?1", [app.id.0])
            .map_err(db)?;
        for (permission, filter) in [(0, &app.grants.read), (1, &app.grants.write)] {
            if let Some(Filter::Kinds(kinds)) = filter {
                for kind in kinds {
                    self.sql
                        .execute(
                            "INSERT INTO app_kinds VALUES(?1,?2,?3)",
                            params![app.id.0, permission, kind],
                        )
                        .map_err(db)?;
                }
            }
        }
        Ok(())
    }
}
fn all_flag(filter: &Option<Filter>) -> Option<i64> {
    filter.as_ref().map(|f| i64::from(matches!(f, Filter::All)))
}

impl Transaction for SqlTransaction<'_> {
    fn membership_identity(&mut self) -> Result<Option<Certificate>, Error> {
        let bytes: Option<Vec<u8>> = self
            .sql
            .query_row(
                "SELECT bytes FROM membership_identity WHERE singleton=1",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(db)?;
        bytes
            .map(|b| Certificate::from_bytes(&b).map_err(Into::into))
            .transpose()
    }
    fn set_membership_identity(&mut self, identity: &Certificate) -> Result<(), Error> {
        if let Some(previous) = self.membership_identity()? {
            if previous != *identity {
                return Err(Error::IdentityMismatch);
            }
            return Ok(());
        }
        self.sql
            .execute(
                "INSERT INTO membership_identity VALUES(1,?1)",
                [identity.bytes()],
            )
            .map_err(db)?;
        Ok(())
    }
    fn recovery_roots(&mut self) -> Result<Vec<Certificate>, Error> {
        let mut query = self
            .sql
            .prepare("SELECT id,bytes FROM membership_recovery_roots ORDER BY id")
            .map_err(db)?;
        let mut rows = query.query([]).map_err(db)?;
        let mut result = Vec::new();
        while let Some(row) = rows.next().map_err(db)? {
            let id: Vec<u8> = row.get(0).map_err(db)?;
            let bytes: Vec<u8> = row.get(1).map_err(db)?;
            let certificate = Certificate::from_bytes(&bytes)?;
            if fixed(id)? != certificate.id() {
                return Err(Error::CorruptLog);
            }
            result.push(certificate);
        }
        Ok(result)
    }
    fn insert_recovery_root(&mut self, root: &Certificate) -> Result<(), Error> {
        self.sql
            .execute(
                "INSERT INTO membership_recovery_roots VALUES(?1,?2)",
                params![root.id(), root.bytes()],
            )
            .map_err(db)?;
        Ok(())
    }
    fn membership_root(&mut self) -> Result<Option<Certificate>, Error> {
        let bytes: Option<Vec<u8>> = self
            .sql
            .query_row(
                "SELECT bytes FROM membership_root WHERE singleton=1",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(db)?;
        bytes
            .map(|b| Certificate::from_bytes(&b).map_err(Into::into))
            .transpose()
    }
    fn set_membership_root(&mut self, root: &Certificate) -> Result<(), Error> {
        if let Some(existing) = self.membership_root()? {
            if existing != *root {
                return Err(Error::IdentityMismatch);
            }
            return Ok(());
        }
        self.sql
            .execute("INSERT INTO membership_root VALUES(1,?1)", [root.bytes()])
            .map_err(db)?;
        Ok(())
    }
    fn membership_entries(&mut self) -> Result<Vec<LogEntry<Certificate>>, Error> {
        let mut query = self
            .sql
            .prepare("SELECT seq,id,bytes FROM membership_log ORDER BY seq")
            .map_err(db)?;
        let mut rows = query.query([]).map_err(db)?;
        let mut entries = Vec::new();
        while let Some(row) = rows.next().map_err(db)? {
            let number: i64 = row.get(0).map_err(db)?;
            let id: Vec<u8> = row.get(1).map_err(db)?;
            let bytes: Vec<u8> = row.get(2).map_err(db)?;
            let record = Certificate::from_bytes(&bytes)?;
            if number <= 0 || fixed(id)? != record.id() {
                return Err(Error::CorruptLog);
            }
            entries.push(LogEntry {
                seq: number as u64,
                record,
            });
        }
        Ok(entries)
    }
    fn append_membership(&mut self, certificate: &Certificate) -> Result<u64, Error> {
        let existing: Option<(i64, Vec<u8>)> = self
            .sql
            .query_row(
                "SELECT seq,bytes FROM membership_log WHERE id=?1",
                [certificate.id()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(db)?;
        if let Some((number, bytes)) = existing {
            if bytes != certificate.bytes() {
                return Err(Error::IdentityMismatch);
            }
            return Ok(number as u64);
        }
        self.sql
            .execute(
                "INSERT INTO membership_log(id,bytes) VALUES(?1,?2)",
                params![certificate.id(), certificate.bytes()],
            )
            .map_err(db)?;
        Ok(self.sql.last_insert_rowid() as u64)
    }
    fn membership_cursor(&mut self, sender: NodeId, epoch: LogEpoch) -> Result<u64, Error> {
        let value: Option<i64> = self
            .sql
            .query_row(
                "SELECT through FROM membership_cursors WHERE sender=?1 AND epoch=?2",
                params![sender.0, epoch.0],
                |r| r.get(0),
            )
            .optional()
            .map_err(db)?;
        Ok(value.unwrap_or(0) as u64)
    }
    fn set_membership_cursor(
        &mut self,
        sender: NodeId,
        epoch: LogEpoch,
        through: u64,
    ) -> Result<(), Error> {
        self.sql.execute("INSERT INTO membership_cursors VALUES(?1,?2,?3) ON CONFLICT(sender,epoch) DO UPDATE SET through=excluded.through", params![sender.0,epoch.0,seq(through)?]).map_err(db)?;
        Ok(())
    }
    fn peer(&mut self, node: NodeId) -> Result<Option<Peer>, Error> {
        let row = self
            .sql
            .query_row(
                "SELECT key_kind,key_bytes,endpoint,retired FROM peers WHERE node=?1",
                [node.0],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, Vec<u8>>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, bool>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(db)?;
        row.map(|(kind, bytes, endpoint, retired)| {
            Ok(Peer {
                node,
                key: match kind {
                    0 => PeerKey::TlsCertificate(bytes),
                    1 => PeerKey::Iroh(fixed(bytes)?),
                    _ => return Err(Error::CorruptLog),
                },
                endpoint,
                retired,
            })
        })
        .transpose()
    }
    fn peers(&mut self) -> Result<Vec<Peer>, Error> {
        let mut statement = self
            .sql
            .prepare("SELECT node FROM peers ORDER BY node")
            .map_err(db)?;
        let nodes = statement
            .query_map([], |r| r.get::<_, Vec<u8>>(0))
            .map_err(db)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db)?;
        nodes
            .into_iter()
            .map(|bytes| self.peer(NodeId(fixed(bytes)?))?.ok_or(Error::CorruptLog))
            .collect()
    }
    fn put_peer(&mut self, peer: &Peer) -> Result<(), Error> {
        let (kind, bytes): (i64, &[u8]) = match &peer.key {
            PeerKey::TlsCertificate(bytes) => (0, bytes),
            PeerKey::Iroh(bytes) => (1, bytes),
        };
        self.sql.execute("INSERT INTO peers VALUES(?1,?2,?3,?4,?5) ON CONFLICT(node) DO UPDATE SET
            key_kind=excluded.key_kind,key_bytes=excluded.key_bytes,endpoint=excluded.endpoint,retired=excluded.retired",
            params![peer.node.0, kind, bytes, peer.endpoint, peer.retired]).map_err(db)?;
        Ok(())
    }
    fn identity(&self) -> Identity {
        self.identity
    }
    fn entries(&mut self) -> Result<Vec<LogEntry>, Error> {
        self.read_entries(
            "SELECT seq,update_id,bytes,digest FROM accept_log ORDER BY seq",
            [],
        )
    }
    fn content_entries(
        &mut self,
        content: ContentId,
        through: Option<u64>,
    ) -> Result<Vec<LogEntry>, Error> {
        let entries = self.read_entries(
            "SELECT log.seq,log.update_id,log.bytes,log.digest FROM content_records AS records
             JOIN accept_log AS log ON log.seq=records.seq WHERE records.content=?1 AND records.seq<=?2
             ORDER BY records.seq",
            params![content.0, through.map(seq).transpose()?.unwrap_or(i64::MAX)],
        )?;
        if entries
            .iter()
            .any(|entry| entry.record.update().content_id != content)
        {
            return Err(Error::CorruptLog);
        }
        Ok(entries)
    }
    fn accepted_high(&mut self) -> Result<u64, Error> {
        let high: i64 = self
            .sql
            .query_row("SELECT COALESCE(MAX(seq),0) FROM accept_log", [], |r| {
                r.get(0)
            })
            .map_err(db)?;
        high.try_into().map_err(|_| Error::CorruptLog)
    }
    fn node_metrics(&mut self, now: i64) -> Result<harmion_protocol::metrics::NodeMetrics, Error> {
        self.sql
            .query_row(
                "SELECT (SELECT COUNT(DISTINCT content) FROM content_records),
                    (SELECT COUNT(*) FROM search_contents),
                    (SELECT COUNT(*) FROM accept_log),
                    (SELECT COALESCE(SUM(length(bytes)),0) FROM accept_log),
                    (SELECT COUNT(*) FROM apps WHERE revoked=0 AND expires_at>?1)",
                [now],
                |row| {
                    Ok(harmion_protocol::metrics::NodeMetrics {
                        contents: row.get::<_, i64>(0)?.to_string(),
                        visible_contents: row.get::<_, i64>(1)?.to_string(),
                        accepted_updates: row.get::<_, i64>(2)?.to_string(),
                        record_bytes: row.get::<_, i64>(3)?.to_string(),
                        active_apps: row.get::<_, i64>(4)?.to_string(),
                    })
                },
            )
            .map_err(db)
    }
    fn accepted_page(&mut self, after: u64, high: u64) -> Result<Vec<LogEntry>, Error> {
        self.read_entries(
            "WITH candidates AS (
                SELECT seq,length(bytes) AS bytes FROM accept_log
                WHERE seq>?1 AND seq<=?2 ORDER BY seq LIMIT ?3
            ), budget AS (
                SELECT seq,SUM(bytes) OVER (ORDER BY seq) AS used FROM candidates
            )
            SELECT log.seq,log.update_id,
                   CASE WHEN length(log.bytes)>?5 THEN x'' ELSE log.bytes END,log.digest
            FROM budget JOIN accept_log AS log USING(seq)
            WHERE budget.used<=?4 OR log.seq=(SELECT MIN(seq) FROM candidates)
            ORDER BY log.seq",
            params![
                seq(after)?,
                seq(high)?,
                harmion_protocol::MAX_PAGE_ENTRIES as i64,
                harmion_protocol::MAX_PAGE_RECORD_BYTES as i64,
                harmion_protocol::MAX_RECORD_BYTES as i64,
            ],
        )
    }
    fn accepted_record(&mut self, id: UpdateId) -> Result<Option<Record>, Error> {
        Ok(self
            .read_entries(
                "SELECT seq,update_id,bytes,digest FROM accept_log WHERE update_id=?1",
                [id.0],
            )?
            .pop()
            .map(|entry| entry.record))
    }
    fn export_content_ids(
        &mut self,
        through: u64,
        after: Option<ContentId>,
        limit: u32,
    ) -> Result<Vec<ContentId>, Error> {
        let mut statement;
        let mut rows = if let Some(after) = after {
            statement = self
                .sql
                .prepare_cached(
                    "SELECT DISTINCT content FROM content_records WHERE content>?1 AND seq<=?2
                 ORDER BY content LIMIT ?3",
                )
                .map_err(db)?;
            statement
                .query(params![after.0, seq(through)?, limit])
                .map_err(db)?
        } else {
            statement = self.sql.prepare_cached(
                "SELECT DISTINCT content FROM content_records WHERE seq<=?1 ORDER BY content LIMIT ?2"
            ).map_err(db)?;
            statement.query(params![seq(through)?, limit]).map_err(db)?
        };
        let mut result = Vec::new();
        while let Some(row) = rows.next().map_err(db)? {
            result.push(ContentId(fixed(row.get(0).map_err(db)?)?));
        }
        Ok(result)
    }
    fn append(&mut self, record: &Record) -> Result<u64, Error> {
        self.sql
            .execute(
                "INSERT INTO accept_log(update_id,bytes,digest) VALUES(?1,?2,?3)",
                params![record.update().update_id.0, record.bytes(), record.digest()],
            )
            .map_err(db)?;
        let seq = self.sql.last_insert_rowid();
        self.sql
            .execute(
                "INSERT INTO content_records VALUES(?1,?2)",
                params![record.update().content_id.0, seq],
            )
            .map_err(db)?;
        Ok(seq as u64)
    }
    fn put_search_projection(&mut self, snapshot: &ContentSnapshot) -> Result<(), Error> {
        query::put_projection(self.sql, snapshot)?;
        changes::record(self.sql, snapshot.content.id)
    }
    fn change_window(&mut self) -> Result<ChangeWindow, Error> {
        changes::window(self.sql)
    }
    fn changes_after(&mut self, after: u64, limit: u32) -> Result<Vec<ContentChange>, Error> {
        changes::after(self.sql, after, limit)
    }
    fn search_content_ids(
        &mut self,
        condition: &Condition,
        read: &Filter,
        after: Option<ContentId>,
        limit: u32,
    ) -> Result<Vec<ContentId>, Error> {
        query::search(self.sql, condition, read, after, limit)
    }
    fn freeze_query(
        &mut self,
        start: QueryCursor,
        app: AppId,
        condition: &Condition,
        read: &Filter,
        now: i64,
        expires_at: i64,
    ) -> Result<(), Error> {
        query_session::freeze(self.sql, start, app, condition, read, now, expires_at)
    }
    fn query_position(&mut self, cursor: QueryCursor) -> Result<Option<QueryPosition>, Error> {
        query_session::position(self.sql, cursor)
    }
    fn discard_query(&mut self, session: QueryCursor) -> Result<(), Error> {
        query_session::discard(self.sql, session)
    }
    fn query_revisions(
        &mut self,
        position: &QueryPosition,
        limit: u32,
    ) -> Result<Vec<QueryRevision>, Error> {
        query_session::revisions(self.sql, position, limit)
    }
    fn put_query_cursor(
        &mut self,
        session: QueryCursor,
        after: ContentId,
        cursor: QueryCursor,
    ) -> Result<QueryCursor, Error> {
        query_session::put_cursor(self.sql, session, after, cursor)
    }
    fn cursor(&mut self, sender: NodeId, epoch: LogEpoch) -> Result<u64, Error> {
        let cursor: Option<i64> = self
            .sql
            .query_row(
                "SELECT through FROM peer_cursors WHERE sender=?1 AND epoch=?2",
                params![sender.0, epoch.0],
                |r| r.get(0),
            )
            .optional()
            .map_err(db)?;
        cursor
            .unwrap_or(0)
            .try_into()
            .map_err(|_| Error::CorruptLog)
    }
    fn set_cursor(&mut self, sender: NodeId, epoch: LogEpoch, through: u64) -> Result<(), Error> {
        self.sql
            .execute(
                "INSERT INTO peer_cursors(sender,epoch,through) VALUES(?1,?2,?3)
            ON CONFLICT(sender,epoch) DO UPDATE SET through=excluded.through",
                params![sender.0, epoch.0, seq(through)?],
            )
            .map_err(db)?;
        Ok(())
    }
    fn has_apps(&mut self) -> Result<bool, Error> {
        self.sql
            .query_row("SELECT EXISTS(SELECT 1 FROM apps)", [], |r| r.get(0))
            .map_err(db)
    }
    fn app_by_verifier(&mut self, verifier: [u8; 32]) -> Result<Option<App>, Error> {
        let id: Option<Vec<u8>> = self
            .sql
            .query_row(
                "SELECT app_id FROM apps WHERE verifier=?1",
                [verifier],
                |r| r.get(0),
            )
            .optional()
            .map_err(db)?;
        match id {
            Some(id) => self.app_by_id(AppId(fixed(id)?)),
            None => Ok(None),
        }
    }
    fn app_by_id(&mut self, id: AppId) -> Result<Option<App>, Error> {
        let row = self.sql.query_row("SELECT verifier,expires_at,revoked,read_all,write_all,manage_apps,manage_nodes FROM apps WHERE app_id=?1", [id.0], |r| {
            Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, i64>(1)?, r.get::<_, bool>(2)?,
                r.get::<_, Option<i64>>(3)?, r.get::<_, Option<i64>>(4)?, r.get::<_, bool>(5)?, r.get::<_, bool>(6)?))
        }).optional().map_err(db)?;
        let Some((verifier, expires_at, revoked, read, write, manage_apps, manage_nodes)) = row
        else {
            return Ok(None);
        };
        Ok(Some(App {
            id,
            verifier: fixed(verifier)?,
            expires_at,
            revoked,
            grants: Grants {
                read: self.filter(id, 0, read)?,
                write: self.filter(id, 1, write)?,
                manage_apps,
                manage_nodes,
            },
        }))
    }
    fn insert_app(&mut self, app: &App) -> Result<(), Error> {
        self.sql
            .execute(
                "INSERT INTO apps VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    app.id.0,
                    app.verifier,
                    app.expires_at,
                    app.revoked,
                    all_flag(&app.grants.read),
                    all_flag(&app.grants.write),
                    app.grants.manage_apps,
                    app.grants.manage_nodes
                ],
            )
            .map_err(db)?;
        self.write_kinds(app)
    }
    fn update_app(&mut self, app: &App) -> Result<(), Error> {
        let changed = self.sql.execute("UPDATE apps SET expires_at=?2,revoked=?3,read_all=?4,write_all=?5,manage_apps=?6,manage_nodes=?7 WHERE app_id=?1",
            params![app.id.0, app.expires_at, app.revoked, all_flag(&app.grants.read), all_flag(&app.grants.write), app.grants.manage_apps, app.grants.manage_nodes]).map_err(db)?;
        if changed != 1 {
            return Err(Error::NotFound);
        }
        self.write_kinds(app)?;
        changes::advance_grants(self.sql, app.id)
    }
    fn grant_generation(&mut self, app: AppId) -> Result<u64, Error> {
        changes::grant_generation(self.sql, app)
    }
    fn token(&mut self, token: EditToken) -> Result<Option<TokenState>, Error> {
        let row = self
            .sql
            .query_row(
                "SELECT content,purpose FROM edit_tokens WHERE token=?1",
                [token.0],
                |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, i64>(1)?)),
            )
            .optional()
            .map_err(db)?;
        let Some((content, purpose)) = row else {
            return Ok(None);
        };
        let purpose = match purpose {
            0 => TokenPurpose::Edit,
            1 => TokenPurpose::Resolve,
            2 => TokenPurpose::Manage,
            _ => return Err(Error::CorruptLog),
        };
        let mut statement = self
            .sql
            .prepare("SELECT update_id FROM token_basis WHERE token=?1 ORDER BY update_id")
            .map_err(db)?;
        let mut basis = BTreeSet::new();
        for id in statement
            .query_map([token.0], |r| r.get::<_, Vec<u8>>(0))
            .map_err(db)?
        {
            basis.insert(UpdateId(fixed(id.map_err(db)?)?));
        }
        Ok(Some(TokenState {
            content_id: ContentId(fixed(content)?),
            basis,
            purpose,
        }))
    }
    fn insert_token(&mut self, token: EditToken, state: &TokenState) -> Result<(), Error> {
        let purpose = match state.purpose {
            TokenPurpose::Edit => 0,
            TokenPurpose::Resolve => 1,
            TokenPurpose::Manage => 2,
        };
        self.sql
            .execute(
                "INSERT INTO edit_tokens VALUES(?1,?2,?3)",
                params![token.0, state.content_id.0, purpose],
            )
            .map_err(db)?;
        for id in &state.basis {
            self.sql
                .execute(
                    "INSERT INTO token_basis VALUES(?1,?2)",
                    params![token.0, id.0],
                )
                .map_err(db)?;
        }
        Ok(())
    }
    fn request(&mut self, app: AppId, request: RequestId) -> Result<Option<RequestResult>, Error> {
        let row = self.sql.query_row("SELECT request,update_id,snapshot_token,return_view FROM request_results WHERE app_id=?1 AND request_id=?2",
            params![app.0, request.0], |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, Vec<u8>>(1)?, r.get::<_, Vec<u8>>(2)?, r.get::<_, bool>(3)?))).optional().map_err(db)?;
        row.map(|(request, update, token, return_view)| {
            Ok(RequestResult {
                request,
                update_id: UpdateId(fixed(update)?),
                snapshot_token: EditToken(fixed(token)?),
                return_view,
            })
        })
        .transpose()
    }
    fn insert_request(
        &mut self,
        app: AppId,
        request: RequestId,
        result: &RequestResult,
    ) -> Result<(), Error> {
        self.sql
            .execute(
                "INSERT INTO request_results VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    app.0,
                    request.0,
                    result.request,
                    result.update_id.0,
                    result.snapshot_token.0,
                    result.return_view
                ],
            )
            .map_err(db)?;
        Ok(())
    }
}

impl Store for SqliteStore {
    fn transaction<T>(
        &mut self,
        f: impl FnOnce(&mut dyn Transaction) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db)?;
        let result = f(&mut SqlTransaction {
            sql: &tx,
            identity: self.identity,
        })?;
        tx.commit().map_err(db)?;
        Ok(result)
    }
}
