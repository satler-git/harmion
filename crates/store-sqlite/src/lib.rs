//! Native SQLite adapter for the node transaction port.

mod challenge;
mod changes;
mod oauth;
mod query;
mod query_session;
mod startup;
mod sync_status;

use harmion_domain::{ClusterId, ContentId, ContentSnapshot, NodeId, UpdateId};
use harmion_node::identity::{KnownIdentity, Provenance};
use harmion_node::membership::{Peer, PeerKey};
use harmion_node::{
    Error,
    access::{App, Filter, Grants},
    application::{RequestResult, TokenPurpose, TokenState},
    owner::{OwnerAudit, OwnerChallenge, OwnerCredential, OwnerPolicy},
    query::{QueryPosition, QueryRevision},
    storage::{ChangeWindow, ContentChange, Identity, Store, Transaction},
};
use harmion_protocol::connect::{ConnectionId, ConnectionStatus};
use harmion_protocol::interpreter::{ProviderId, ProviderStatus};
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

type OwnerCredentialRow = (Vec<u8>, i64, String, String, String, i64, Option<i64>, bool);
type OwnerChallengeRow = (
    Vec<u8>,
    Vec<u8>,
    Option<Vec<u8>>,
    String,
    Option<Vec<u8>>,
    Vec<u8>,
    i64,
    i64,
    Option<i64>,
);
type OwnerAuditRow = (
    i64,
    i64,
    String,
    String,
    Option<Vec<u8>>,
    Vec<u8>,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    String,
    Vec<u8>,
    Vec<u8>,
);

struct InterpreterRow {
    provider: Vec<u8>,
    app: Vec<u8>,
    name: String,
    contract_version: i64,
    manifest: Vec<u8>,
    status: i64,
    created_at: i64,
    approved_at: Option<i64>,
}
struct ConnectionRow {
    id: Vec<u8>,
    secret_verifier: Vec<u8>,
    name: String,
    redirect: String,
    web_origin: Option<String>,
    client: Option<Vec<u8>>,
    challenge: Vec<u8>,
    state: String,
    grants: String,
    provider_manifest: Option<Vec<u8>>,
    provider_replace: bool,
    status: i64,
    created_at: i64,
    expires_at: i64,
    approved_at: Option<i64>,
    completed_at: Option<i64>,
    app: Option<Vec<u8>>,
    provider: Option<Vec<u8>>,
}
fn connection_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ConnectionRow> {
    Ok(ConnectionRow {
        id: row.get(0)?,
        secret_verifier: row.get(1)?,
        name: row.get(2)?,
        redirect: row.get(3)?,
        web_origin: row.get(4)?,
        client: row.get(5)?,
        challenge: row.get(6)?,
        state: row.get(7)?,
        grants: row.get(8)?,
        provider_manifest: row.get(9)?,
        provider_replace: row.get(10)?,
        status: row.get(11)?,
        created_at: row.get(12)?,
        expires_at: row.get(13)?,
        approved_at: row.get(14)?,
        completed_at: row.get(15)?,
        app: row.get(16)?,
        provider: row.get(17)?,
    })
}
fn connection_from(row: ConnectionRow) -> Result<harmion_node::connect::ConnectionRequest, Error> {
    let grants: harmion_protocol::GrantSpec =
        serde_json::from_str(&row.grants).map_err(|_| Error::CorruptLog)?;
    Ok(harmion_node::connect::ConnectionRequest {
        id: ConnectionId(fixed(row.id)?),
        secret_verifier: fixed(row.secret_verifier)?,
        name: row.name,
        redirect: row.redirect,
        web_origin: row.web_origin,
        client: row.client.map(fixed).transpose()?.map(AppId),
        challenge: fixed(row.challenge)?,
        state: row.state,
        grants: grants.into(),
        provider_manifest: row.provider_manifest,
        provider_replace: row.provider_replace,
        status: ConnectionStatus::from_storage(row.status).ok_or(Error::CorruptLog)?,
        created_at: row.created_at,
        expires_at: row.expires_at,
        approved_at: row.approved_at,
        completed_at: row.completed_at,
        app: row.app.map(fixed).transpose()?.map(AppId),
        provider_id: row.provider.map(fixed).transpose()?.map(ProviderId),
    })
}
fn interpreter_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<InterpreterRow> {
    Ok(InterpreterRow {
        provider: row.get(0)?,
        app: row.get(1)?,
        name: row.get(2)?,
        contract_version: row.get(3)?,
        manifest: row.get(4)?,
        status: row.get(5)?,
        created_at: row.get(6)?,
        approved_at: row.get(7)?,
    })
}
fn interpreter_from(row: InterpreterRow) -> Result<harmion_node::interpreter::Registration, Error> {
    Ok(harmion_node::interpreter::Registration {
        provider: ProviderId(fixed(row.provider)?),
        app: AppId(fixed(row.app)?),
        name: row.name,
        contract_version: row
            .contract_version
            .try_into()
            .map_err(|_| Error::CorruptLog)?,
        manifest: row.manifest,
        status: ProviderStatus::from_storage(row.status).ok_or(Error::CorruptLog)?,
        created_at: row.created_at,
        approved_at: row.approved_at,
    })
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
    auth_clock: harmion_node::https_peer::AuthClock,
}

/// A partial or simulated upgrade can already carry the retention column.
/// Table and column names here are fixed literals, never user input.
fn has_column(tx: &rusqlite::Transaction<'_>, table: &str, column: &str) -> Result<bool, Error> {
    let mut statement = tx
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(db)?;
    let mut rows = statement.query([]).map_err(db)?;
    while let Some(row) = rows.next().map_err(db)? {
        let name: String = row.get(1).map_err(db)?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Allow `key_kind` 2 (signed HTTPS) in the routing projection. SQLite cannot
/// change a CHECK in place, and the sync-status tables carry a foreign key to
/// `peers`, so the documented 12-step rebuild must run with foreign-key
/// enforcement off, i.e. outside the schema transaction. It is idempotent: it
/// only rebuilds a v4+ `peers` table whose CHECK still excludes kind 2.
fn widen_peer_keys(connection: &mut Connection) -> Result<(), Error> {
    let sql: Option<String> = connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='peers'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(db)?;
    let Some(sql) = sql else {
        return Ok(());
    };
    // A pre-v4 table has no key_kind; the v4 migration creates the widened form.
    if !sql.contains("key_kind") || sql.contains("key_kind IN (0,1,2)") {
        return Ok(());
    }
    connection
        .execute_batch("PRAGMA foreign_keys=OFF;")
        .map_err(db)?;
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db)?;
    tx.execute_batch(
        "CREATE TABLE peers_widened (node BLOB PRIMARY KEY CHECK(length(node)=32),
            key_kind INTEGER NOT NULL CHECK(key_kind IN (0,1,2)), key_bytes BLOB NOT NULL,
            endpoint TEXT NOT NULL, retired INTEGER NOT NULL CHECK(retired IN (0,1)),
            UNIQUE(key_kind,key_bytes), CHECK(key_kind=0 OR length(key_bytes)=32));
        INSERT INTO peers_widened SELECT node,key_kind,key_bytes,endpoint,retired FROM peers;
        DROP TABLE peers;
        ALTER TABLE peers_widened RENAME TO peers;",
    )
    .map_err(db)?;
    tx.commit().map_err(db)?;
    connection
        .execute_batch("PRAGMA foreign_keys=ON;")
        .map_err(db)?;
    Ok(())
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
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(db)?;
        if !(0..=17).contains(&version) {
            return Err(Error::Storage(format!(
                "unsupported schema version {version}"
            )));
        }
        if version < 11 {
            widen_peer_keys(&mut connection)?;
        }
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db)?;
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
                    key_kind INTEGER NOT NULL CHECK(key_kind IN (0,1,2)), key_bytes BLOB NOT NULL,
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
        if version < 8 {
            changes::initialize(&tx)?;
        }
        if version < 9 {
            oauth::initialize(&tx)?;
            tx.pragma_update(None, "user_version", 9).map_err(db)?;
        }
        if version < 10 {
            sync_status::initialize(&tx)?;
            tx.pragma_update(None, "user_version", 10).map_err(db)?;
        }
        if version < 11 {
            // Authentication identity pins are durable and independent of current
            // membership grants. key_kind 2 (signed HTTPS) is the HMC v2 binding.
            // A pin is never rewritten and a key is never reused for another node.
            tx.execute_batch(
                "CREATE TABLE identity_pins (
                    node BLOB PRIMARY KEY CHECK(length(node)=32),
                    signing_key BLOB NOT NULL UNIQUE CHECK(length(signing_key)=32),
                    key_kind INTEGER NOT NULL CHECK(key_kind IN (0,1,2)),
                    key_bytes BLOB NOT NULL,
                    provenance_kind INTEGER NOT NULL CHECK(provenance_kind IN (0,1,2,3)),
                    provenance_id BLOB,
                    CHECK((provenance_kind=2 AND provenance_id IS NOT NULL AND length(provenance_id)=32)
                       OR (provenance_kind IN (0,1,3) AND provenance_id IS NULL)),
                    UNIQUE(key_kind,key_bytes));
                PRAGMA user_version=11;",
            )
            .map_err(db)?;
        }
        if version < 12 {
            challenge::initialize(&tx)?;
        }
        if version < 13 {
            // Retention metadata. Existing rows keep 0 and are grandfathered;
            // only newly created tokens/receipts are reclaimed after 30 days.
            if !has_column(&tx, "edit_tokens", "created_at")? {
                tx.execute_batch(
                    "ALTER TABLE edit_tokens ADD COLUMN created_at INTEGER NOT NULL DEFAULT 0;",
                )
                .map_err(db)?;
            }
            if !has_column(&tx, "request_results", "created_at")? {
                tx.execute_batch(
                    "ALTER TABLE request_results ADD COLUMN created_at INTEGER NOT NULL DEFAULT 0;",
                )
                .map_err(db)?;
            }
            tx.pragma_update(None, "user_version", 13).map_err(db)?;
        }
        if version < 14 {
            // Interpreter is a local-App capability. Peer-only hosts leave these
            // columns false and do not implement the registry methods.
            if !has_column(&tx, "apps", "call")? {
                tx.execute_batch("ALTER TABLE apps ADD COLUMN call INTEGER NOT NULL DEFAULT 0;")
                    .map_err(db)?;
            }
            if !has_column(&tx, "apps", "provide")? {
                tx.execute_batch("ALTER TABLE apps ADD COLUMN provide INTEGER NOT NULL DEFAULT 0;")
                    .map_err(db)?;
            }
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS interpreters (
                    provider_id BLOB PRIMARY KEY CHECK(length(provider_id)=32),
                    app_id BLOB NOT NULL REFERENCES apps(app_id),
                    name TEXT NOT NULL,
                    contract_version INTEGER NOT NULL,
                    manifest BLOB NOT NULL,
                    status INTEGER NOT NULL CHECK(status IN (0,1,2)),
                    created_at INTEGER NOT NULL,
                    approved_at INTEGER,
                    CHECK(status != 1 OR approved_at IS NOT NULL),
                    UNIQUE(app_id, name));
                CREATE UNIQUE INDEX IF NOT EXISTS interpreters_approved_name
                    ON interpreters(name) WHERE status=1;
                PRAGMA user_version=14;",
            )
            .map_err(db)?;
        }
        if version < 15 {
            // OAuth `provide:NAME` pre-approvals are local App grants, not
            // replicated membership or Interpreter state.
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS app_provide_names (
                    app_id BLOB NOT NULL REFERENCES apps(app_id) ON DELETE CASCADE,
                    name TEXT NOT NULL,
                    PRIMARY KEY(app_id, name));
                PRAGMA user_version=15;",
            )
            .map_err(db)?;
        }
        if version < 16 {
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS connection_requests (
                    id BLOB PRIMARY KEY CHECK(length(id)=32),
                    secret_verifier BLOB NOT NULL CHECK(length(secret_verifier)=32),
                    name TEXT NOT NULL,
                    redirect TEXT NOT NULL,
                    web_origin TEXT,
                    client BLOB CHECK(client IS NULL OR length(client)=32),
                    challenge BLOB NOT NULL CHECK(length(challenge)=32),
                    state TEXT NOT NULL,
                    grants TEXT NOT NULL,
                    provider_manifest BLOB,
                    provider_replace INTEGER NOT NULL CHECK(provider_replace IN (0,1)),
                    status INTEGER NOT NULL CHECK(status IN (0,1,2,3,4)),
                    created_at INTEGER NOT NULL,
                    expires_at INTEGER NOT NULL,
                    approved_at INTEGER,
                    completed_at INTEGER,
                    app BLOB CHECK(app IS NULL OR length(app)=32),
                    provider BLOB CHECK(provider IS NULL OR length(provider)=32));
                CREATE INDEX IF NOT EXISTS connection_requests_expiry
                    ON connection_requests(expires_at);
                PRAGMA user_version=16;",
            )
            .map_err(db)?;
        }
        if version < 17 {
            // Owner approvals are node-local. Existing nodes keep enforce=0 and
            // continue to work until the host runs the offline enrollment.
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS owner_credentials (
                    id BLOB PRIMARY KEY CHECK(length(id)=32),
                    mechanism INTEGER NOT NULL CHECK(mechanism=1),
                    public_key TEXT NOT NULL,
                    fingerprint TEXT NOT NULL,
                    label TEXT NOT NULL,
                    created_at INTEGER NOT NULL,
                    last_used_at INTEGER,
                    revoked INTEGER NOT NULL CHECK(revoked IN (0,1)));
                CREATE TABLE IF NOT EXISTS owner_policy (
                    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
                    enforce INTEGER NOT NULL CHECK(enforce IN (0,1)),
                    recovery_hash BLOB CHECK(recovery_hash IS NULL OR length(recovery_hash)=32),
                    updated_at INTEGER NOT NULL);
                CREATE TABLE IF NOT EXISTS owner_challenges (
                    id BLOB PRIMARY KEY CHECK(length(id)=32),
                    app BLOB NOT NULL CHECK(length(app)=32),
                    statement_hash BLOB NOT NULL CHECK(length(statement_hash)=32),
                    session BLOB CHECK(session IS NULL OR length(session)=32),
                    operation TEXT NOT NULL,
                    target BLOB CHECK(target IS NULL OR length(target)=32),
                    args_hash BLOB NOT NULL CHECK(length(args_hash)=32),
                    issued_at INTEGER NOT NULL,
                    expires_at INTEGER NOT NULL,
                    used_at INTEGER);
                CREATE INDEX IF NOT EXISTS owner_challenges_expiry
                    ON owner_challenges(expires_at);
                CREATE INDEX IF NOT EXISTS owner_challenges_app
                    ON owner_challenges(app, used_at, expires_at);
                CREATE TABLE IF NOT EXISTS owner_audit (
                    seq INTEGER PRIMARY KEY AUTOINCREMENT,
                    at INTEGER NOT NULL,
                    actor TEXT NOT NULL,
                    operation TEXT NOT NULL,
                    target BLOB CHECK(target IS NULL OR length(target)=32),
                    args_hash BLOB NOT NULL CHECK(length(args_hash)=32),
                    credential BLOB CHECK(credential IS NULL OR length(credential)=32),
                    challenge BLOB CHECK(challenge IS NULL OR length(challenge)=32),
                    session BLOB CHECK(session IS NULL OR length(session)=32),
                    result TEXT NOT NULL,
                    prev_hash BLOB NOT NULL CHECK(length(prev_hash)=32),
                    entry_hash BLOB NOT NULL UNIQUE CHECK(length(entry_hash)=32));
                PRAGMA user_version=17;",
            )
            .map_err(db)?;
        }
        {
            let mut storage = SqlTransaction { sql: &tx, identity };
            // Reject broken bytes/dependencies at startup; never salvage only the
            // normal value. Legacy migrations still need the complete graph to
            // initialize the per-Content history and projection indexes.
            let membership = if version < 7 {
                let graph = harmion_node::storage::read_graph(&mut storage)?;
                let membership = harmion_node::membership_state::state(&mut storage)?;
                if version < 6 {
                    query::initialize(&tx)?;
                }
                query::initialize_version_refs(&tx, &graph)?;
                query_session::initialize(&tx)?;
                membership
            } else {
                startup::verify_content_history(&tx, identity)?;
                harmion_node::membership_state::state(&mut storage)?
            };
            if version < 11 {
                // Seed only identities whose exact key/binding is validated from
                // effective membership state; raw historical claims are not replayed.
                harmion_node::identity::migrate(&mut storage, membership.as_ref())?;
            }
            if let Some(state) = &membership {
                harmion_node::identity::verify_active_identities(&mut storage, state)?;
            }
        };
        tx.commit().map_err(db)?;
        Ok(Self {
            connection,
            identity,
            auth_clock: Default::default(),
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
        if !(1..=17).contains(&version) {
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
    fn provide_names(&self, app: AppId) -> Result<BTreeSet<String>, Error> {
        let mut statement = self
            .sql
            .prepare("SELECT name FROM app_provide_names WHERE app_id=?1 ORDER BY name")
            .map_err(db)?;
        statement
            .query_map([app.0], |row| row.get::<_, String>(0))
            .map_err(db)?
            .collect::<Result<BTreeSet<_>, _>>()
            .map_err(db)
    }
    fn write_provide_names(&self, app: &App) -> Result<(), Error> {
        self.sql
            .execute("DELETE FROM app_provide_names WHERE app_id=?1", [app.id.0])
            .map_err(db)?;
        for name in &app.grants.provide_names {
            self.sql
                .execute(
                    "INSERT INTO app_provide_names(app_id,name) VALUES(?1,?2)",
                    params![app.id.0, name],
                )
                .map_err(db)?;
        }
        Ok(())
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
                key: PeerKey::from_parts(kind.try_into().map_err(|_| Error::CorruptLog)?, bytes)?,
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
        self.sql.execute("INSERT INTO peers VALUES(?1,?2,?3,?4,?5) ON CONFLICT(node) DO UPDATE SET
            key_kind=excluded.key_kind,key_bytes=excluded.key_bytes,endpoint=excluded.endpoint,retired=excluded.retired",
            params![peer.node.0, peer.key.kind(), peer.key.bytes(), peer.endpoint, peer.retired]).map_err(db)?;
        Ok(())
    }
    fn known_peer_identity(&mut self, node: NodeId) -> Result<Option<KnownIdentity>, Error> {
        let row = self
            .sql
            .query_row(
                "SELECT signing_key,key_kind,key_bytes,provenance_kind,provenance_id
                 FROM identity_pins WHERE node=?1",
                [node.0],
                |r| {
                    Ok((
                        r.get::<_, Vec<u8>>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, Vec<u8>>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, Option<Vec<u8>>>(4)?,
                    ))
                },
            )
            .optional()
            .map_err(db)?;
        row.map(|(signing_key, kind, bytes, provenance, evidence)| {
            let evidence = evidence
                .map(|id| <[u8; 32]>::try_from(id.as_slice()).map_err(|_| Error::CorruptLog))
                .transpose()?;
            Ok(KnownIdentity {
                node,
                signing_key: <[u8; 32]>::try_from(signing_key.as_slice())
                    .map_err(|_| Error::CorruptLog)?,
                key: PeerKey::from_parts(kind.try_into().map_err(|_| Error::CorruptLog)?, bytes)?,
                provenance: Provenance::from_parts(
                    provenance.try_into().map_err(|_| Error::CorruptLog)?,
                    evidence,
                )?,
            })
        })
        .transpose()
    }
    fn known_peer_identities(&mut self) -> Result<Vec<KnownIdentity>, Error> {
        let mut statement = self
            .sql
            .prepare("SELECT node FROM identity_pins ORDER BY node")
            .map_err(db)?;
        let nodes = statement
            .query_map([], |row| row.get::<_, Vec<u8>>(0))
            .map_err(db)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db)?;
        nodes
            .into_iter()
            .map(|bytes| {
                self.known_peer_identity(NodeId(fixed(bytes)?))?
                    .ok_or(Error::CorruptLog)
            })
            .collect()
    }
    fn put_known_identity(&mut self, identity: &KnownIdentity) -> Result<(), Error> {
        if let Some(existing) = self.known_peer_identity(identity.node)? {
            if existing == *identity {
                return Ok(());
            }
            return Err(Error::IdentityMismatch);
        }
        for existing in self.known_peer_identities()? {
            if existing.signing_key == identity.signing_key || existing.key == identity.key {
                return Err(Error::IdentityMismatch);
            }
        }
        self.sql
            .execute(
                "INSERT INTO identity_pins VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    identity.node.0,
                    identity.signing_key,
                    identity.key.kind(),
                    identity.key.bytes(),
                    identity.provenance.kind(),
                    identity.provenance.certificate(),
                ],
            )
            .map_err(db)?;
        Ok(())
    }
    fn owner_credentials(&mut self) -> Result<Vec<OwnerCredential>, Error> {
        let mut statement = self
            .sql
            .prepare(
                "SELECT id,mechanism,public_key,fingerprint,label,created_at,last_used_at,revoked
                 FROM owner_credentials ORDER BY created_at,id",
            )
            .map_err(db)?;
        let rows: Vec<OwnerCredentialRow> = statement
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            })
            .map_err(db)?
            .collect::<Result<_, _>>()
            .map_err(db)?;
        rows.into_iter()
            .map(
                |(
                    id,
                    mechanism,
                    public_key,
                    fingerprint,
                    label,
                    created_at,
                    last_used_at,
                    revoked,
                )| {
                    let mechanism = u8::try_from(mechanism).map_err(|_| Error::CorruptLog)?;
                    if mechanism != 1 {
                        return Err(Error::CorruptLog);
                    }
                    Ok(OwnerCredential {
                        id: fixed(id)?,
                        mechanism,
                        public_key,
                        fingerprint,
                        label,
                        created_at,
                        last_used_at,
                        revoked,
                    })
                },
            )
            .collect()
    }
    fn insert_owner_credential(&mut self, credential: &OwnerCredential) -> Result<(), Error> {
        self.sql
            .execute(
                "INSERT INTO owner_credentials
                 (id,mechanism,public_key,fingerprint,label,created_at,last_used_at,revoked)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    credential.id,
                    credential.mechanism,
                    credential.public_key,
                    credential.fingerprint,
                    credential.label,
                    credential.created_at,
                    credential.last_used_at,
                    credential.revoked,
                ],
            )
            .map_err(db)?;
        Ok(())
    }
    fn update_owner_credential(&mut self, credential: &OwnerCredential) -> Result<(), Error> {
        let changed = self
            .sql
            .execute(
                "UPDATE owner_credentials SET public_key=?2,fingerprint=?3,label=?4,
                    last_used_at=?5,revoked=?6 WHERE id=?1",
                params![
                    credential.id,
                    credential.public_key,
                    credential.fingerprint,
                    credential.label,
                    credential.last_used_at,
                    credential.revoked,
                ],
            )
            .map_err(db)?;
        if changed != 1 {
            return Err(Error::NotFound);
        }
        Ok(())
    }
    fn owner_policy(&mut self) -> Result<OwnerPolicy, Error> {
        let row: Option<(bool, Option<Vec<u8>>, i64)> = self
            .sql
            .query_row(
                "SELECT enforce,recovery_hash,updated_at FROM owner_policy WHERE singleton=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(db)?;
        match row {
            Some((enforce, recovery, updated_at)) => Ok(OwnerPolicy {
                enforce,
                recovery_hash: recovery.map(fixed).transpose()?,
                updated_at,
            }),
            None => Ok(OwnerPolicy::default()),
        }
    }
    fn put_owner_policy(&mut self, policy: &OwnerPolicy) -> Result<(), Error> {
        self.sql
            .execute(
                "INSERT INTO owner_policy(singleton,enforce,recovery_hash,updated_at)
                 VALUES(1,?1,?2,?3)
                 ON CONFLICT(singleton) DO UPDATE SET
                    enforce=excluded.enforce,
                    recovery_hash=excluded.recovery_hash,
                    updated_at=excluded.updated_at",
                params![
                    policy.enforce,
                    policy.recovery_hash.as_ref().map(|hash| hash.as_slice()),
                    policy.updated_at
                ],
            )
            .map_err(db)?;
        Ok(())
    }
    fn insert_owner_challenge(&mut self, challenge: &OwnerChallenge) -> Result<(), Error> {
        self.sql
            .execute(
                "INSERT INTO owner_challenges
                 (id,app,statement_hash,session,operation,target,args_hash,issued_at,expires_at,used_at)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                params![
                    challenge.id,
                    challenge.app.0,
                    challenge.statement_hash,
                    challenge.session.as_ref().map(|id| id.as_slice()),
                    challenge.operation,
                    challenge.target.as_ref().map(|id| id.as_slice()),
                    challenge.args_hash,
                    challenge.issued_at,
                    challenge.expires_at,
                    challenge.used_at,
                ],
            )
            .map_err(db)?;
        Ok(())
    }
    fn owner_challenge(&mut self, id: [u8; 32]) -> Result<Option<OwnerChallenge>, Error> {
        let row: Option<OwnerChallengeRow> = self
            .sql
            .query_row(
                "SELECT app,statement_hash,session,operation,target,args_hash,issued_at,expires_at,used_at
                 FROM owner_challenges WHERE id=?1",
                [id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                    ))
                },
            )
            .optional()
            .map_err(db)?;
        row.map(
            |(
                app,
                statement_hash,
                session,
                operation,
                target,
                args_hash,
                issued_at,
                expires_at,
                used_at,
            )| {
                Ok(OwnerChallenge {
                    id,
                    app: AppId(fixed(app)?),
                    statement_hash: fixed(statement_hash)?,
                    session: session.map(fixed).transpose()?,
                    operation,
                    target: target.map(fixed).transpose()?,
                    args_hash: fixed(args_hash)?,
                    issued_at,
                    expires_at,
                    used_at,
                })
            },
        )
        .transpose()
    }
    fn consume_owner_challenge(&mut self, id: [u8; 32], now: i64) -> Result<bool, Error> {
        let changed = self
            .sql
            .execute(
                "UPDATE owner_challenges SET used_at=?1
                 WHERE id=?2 AND used_at IS NULL AND expires_at>?1",
                params![now, id],
            )
            .map_err(db)?;
        Ok(changed == 1)
    }
    fn purge_owner_challenges(&mut self, now: i64) -> Result<(), Error> {
        self.sql
            .execute(
                "DELETE FROM owner_challenges
                 WHERE expires_at<?1 OR (used_at IS NOT NULL AND used_at<?2)",
                params![now, now.saturating_sub(86_400_000_000)],
            )
            .map_err(db)?;
        Ok(())
    }
    fn owner_pending_challenges(&mut self, app: AppId, now: i64) -> Result<u32, Error> {
        let count: i64 = self
            .sql
            .query_row(
                "SELECT COUNT(*) FROM owner_challenges
                 WHERE app=?1 AND used_at IS NULL AND expires_at>?2",
                params![app.0, now],
                |row| row.get(0),
            )
            .map_err(db)?;
        u32::try_from(count).map_err(|_| Error::CorruptLog)
    }
    fn append_owner_audit(&mut self, audit: &OwnerAudit) -> Result<[u8; 32], Error> {
        let previous: Option<Vec<u8>> = self
            .sql
            .query_row(
                "SELECT entry_hash FROM owner_audit ORDER BY seq DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(db)?;
        let previous = previous.map(fixed).transpose()?.unwrap_or([0; 32]);
        let entry = harmion_node::owner::audit_entry_hash(previous, audit);
        self.sql
            .execute(
                "INSERT INTO owner_audit
                 (at,actor,operation,target,args_hash,credential,challenge,session,result,prev_hash,entry_hash)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                params![
                    audit.at,
                    audit.actor,
                    audit.operation,
                    audit.target.as_ref().map(|id| id.as_slice()),
                    audit.args_hash,
                    audit.credential.as_ref().map(|id| id.as_slice()),
                    audit.challenge.as_ref().map(|id| id.as_slice()),
                    audit.session.as_ref().map(|id| id.as_slice()),
                    audit.result,
                    previous,
                    entry,
                ],
            )
            .map_err(db)?;
        Ok(entry)
    }
    fn owner_audit_entries(&mut self, limit: u32) -> Result<Vec<OwnerAudit>, Error> {
        let mut statement = self
            .sql
            .prepare(
                "SELECT seq,at,actor,operation,target,args_hash,credential,challenge,session,
                    result,prev_hash,entry_hash
                 FROM owner_audit ORDER BY seq DESC LIMIT ?1",
            )
            .map_err(db)?;
        let rows: Vec<OwnerAuditRow> = statement
            .query_map([i64::from(limit)], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                    row.get(11)?,
                ))
            })
            .map_err(db)?
            .collect::<Result<_, _>>()
            .map_err(db)?;
        rows.into_iter()
            .map(
                |(
                    seq,
                    at,
                    actor,
                    operation,
                    target,
                    args_hash,
                    credential,
                    challenge,
                    session,
                    result,
                    prev_hash,
                    entry_hash,
                )| {
                    Ok(OwnerAudit {
                        seq: u64::try_from(seq).map_err(|_| Error::CorruptLog)?,
                        at,
                        actor,
                        operation,
                        target: target.map(fixed).transpose()?,
                        args_hash: fixed(args_hash)?,
                        credential: credential.map(fixed).transpose()?,
                        challenge: challenge.map(fixed).transpose()?,
                        session: session.map(fixed).transpose()?,
                        result,
                        prev_hash: fixed(prev_hash)?,
                        entry_hash: fixed(entry_hash)?,
                    })
                },
            )
            .collect()
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
    fn accepted_sequence(&mut self, id: UpdateId) -> Result<Option<u64>, Error> {
        self.sql
            .query_row(
                "SELECT seq FROM accept_log WHERE update_id=?1",
                [id.0],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(db)?
            .map(|value| value.try_into().map_err(|_| Error::CorruptLog))
            .transpose()
    }
    fn accepted_count_after(&mut self, after: u64) -> Result<u64, Error> {
        self.sql
            .query_row(
                "SELECT COUNT(*) FROM accept_log WHERE seq>?1",
                [seq(after)?],
                |row| row.get::<_, i64>(0),
            )
            .map_err(db)?
            .try_into()
            .map_err(|_| Error::CorruptLog)
    }
    fn sync_observation(
        &mut self,
        node: NodeId,
    ) -> Result<Option<harmion_node::sync_status::Observation>, Error> {
        self.read_sync_observation(node)
    }
    fn put_sync_observation(
        &mut self,
        node: NodeId,
        observation: &harmion_node::sync_status::Observation,
    ) -> Result<(), Error> {
        self.write_sync_observation(node, observation)
    }
    fn sync_failure(
        &mut self,
        node: NodeId,
    ) -> Result<Option<harmion_node::sync_status::Failure>, Error> {
        self.read_sync_failure(node)
    }
    fn put_sync_failure(
        &mut self,
        node: NodeId,
        failure: Option<&harmion_node::sync_status::Failure>,
    ) -> Result<(), Error> {
        self.write_sync_failure(node, failure)
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
        let row = self.sql.query_row("SELECT verifier,expires_at,revoked,read_all,write_all,call,provide,manage_apps,manage_nodes FROM apps WHERE app_id=?1", [id.0], |r| {
            Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, i64>(1)?, r.get::<_, bool>(2)?,
                r.get::<_, Option<i64>>(3)?, r.get::<_, Option<i64>>(4)?, r.get::<_, bool>(5)?,
                r.get::<_, bool>(6)?, r.get::<_, bool>(7)?, r.get::<_, bool>(8)?))
        }).optional().map_err(db)?;
        let Some((
            verifier,
            expires_at,
            revoked,
            read,
            write,
            call,
            provide,
            manage_apps,
            manage_nodes,
        )) = row
        else {
            return Ok(None);
        };
        let provide_names = self.provide_names(id)?;
        Ok(Some(App {
            id,
            verifier: fixed(verifier)?,
            expires_at,
            revoked,
            grants: Grants {
                read: self.filter(id, 0, read)?,
                write: self.filter(id, 1, write)?,
                call,
                provide,
                provide_names,
                manage_apps,
                manage_nodes,
            },
        }))
    }
    fn insert_app(&mut self, app: &App) -> Result<(), Error> {
        self.sql
            .execute(
                "INSERT INTO apps(app_id,verifier,expires_at,revoked,read_all,write_all,call,provide,manage_apps,manage_nodes) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                params![
                    app.id.0,
                    app.verifier,
                    app.expires_at,
                    app.revoked,
                    all_flag(&app.grants.read),
                    all_flag(&app.grants.write),
                    app.grants.call,
                    app.grants.provide,
                    app.grants.manage_apps,
                    app.grants.manage_nodes
                ],
            )
            .map_err(db)?;
        self.write_kinds(app)?;
        self.write_provide_names(app)
    }
    fn update_app(&mut self, app: &App) -> Result<(), Error> {
        let changed = self.sql.execute("UPDATE apps SET expires_at=?2,revoked=?3,read_all=?4,write_all=?5,call=?6,provide=?7,manage_apps=?8,manage_nodes=?9 WHERE app_id=?1",
            params![app.id.0, app.expires_at, app.revoked, all_flag(&app.grants.read), all_flag(&app.grants.write), app.grants.call, app.grants.provide, app.grants.manage_apps, app.grants.manage_nodes]).map_err(db)?;
        if changed != 1 {
            return Err(Error::NotFound);
        }
        self.write_kinds(app)?;
        self.write_provide_names(app)?;
        changes::advance_grants(self.sql, app.id)
    }
    fn interpreter_registration(
        &mut self,
        provider: ProviderId,
    ) -> Result<Option<harmion_node::interpreter::Registration>, Error> {
        let row = self
            .sql
            .query_row(
                "SELECT provider_id,app_id,name,contract_version,manifest,status,created_at,approved_at FROM interpreters WHERE provider_id=?1",
                [provider.0],
                interpreter_row,
            )
            .optional()
            .map_err(db)?;
        row.map(interpreter_from).transpose()
    }
    fn interpreter_registration_by_app_name(
        &mut self,
        app: AppId,
        name: &str,
    ) -> Result<Option<harmion_node::interpreter::Registration>, Error> {
        let row = self
            .sql
            .query_row(
                "SELECT provider_id,app_id,name,contract_version,manifest,status,created_at,approved_at FROM interpreters WHERE app_id=?1 AND name=?2",
                params![app.0, name],
                interpreter_row,
            )
            .optional()
            .map_err(db)?;
        row.map(interpreter_from).transpose()
    }
    fn interpreter_registrations(
        &mut self,
    ) -> Result<Vec<harmion_node::interpreter::Registration>, Error> {
        let mut statement = self
            .sql
            .prepare(
                "SELECT provider_id,app_id,name,contract_version,manifest,status,created_at,approved_at FROM interpreters ORDER BY created_at,provider_id",
            )
            .map_err(db)?;
        let rows = statement
            .query_map([], interpreter_row)
            .map_err(db)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db)?;
        rows.into_iter().map(interpreter_from).collect()
    }
    fn insert_interpreter_registration(
        &mut self,
        registration: &harmion_node::interpreter::Registration,
    ) -> Result<(), Error> {
        self.sql
            .execute(
                "INSERT INTO interpreters(provider_id,app_id,name,contract_version,manifest,status,created_at,approved_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    registration.provider.0,
                    registration.app.0,
                    registration.name,
                    i64::from(registration.contract_version),
                    registration.manifest,
                    registration.status.storage(),
                    registration.created_at,
                    registration.approved_at
                ],
            )
            .map_err(db)?;
        Ok(())
    }
    fn update_interpreter_registration(
        &mut self,
        registration: &harmion_node::interpreter::Registration,
    ) -> Result<(), Error> {
        let changed = self
            .sql
            .execute(
                "UPDATE interpreters SET name=?2,contract_version=?3,manifest=?4 WHERE provider_id=?1",
                params![
                    registration.provider.0,
                    registration.name,
                    i64::from(registration.contract_version),
                    registration.manifest
                ],
            )
            .map_err(db)?;
        if changed != 1 {
            return Err(Error::NotFound);
        }
        Ok(())
    }
    fn approved_interpreter_by_name(
        &mut self,
        name: &str,
    ) -> Result<Option<harmion_node::interpreter::Registration>, Error> {
        let row = self
            .sql
            .query_row(
                "SELECT provider_id,app_id,name,contract_version,manifest,status,created_at,approved_at FROM interpreters WHERE name=?1 AND status=1",
                [name],
                interpreter_row,
            )
            .optional()
            .map_err(db)?;
        row.map(interpreter_from).transpose()
    }
    fn set_interpreter_status(
        &mut self,
        provider: ProviderId,
        status: ProviderStatus,
        approved_at: Option<i64>,
    ) -> Result<(), Error> {
        let changed = self
            .sql
            .execute(
                "UPDATE interpreters SET status=?2,approved_at=?3 WHERE provider_id=?1",
                params![provider.0, status.storage(), approved_at],
            )
            .map_err(db)?;
        if changed != 1 {
            return Err(Error::NotFound);
        }
        Ok(())
    }
    fn connection_request(
        &mut self,
        id: ConnectionId,
    ) -> Result<Option<harmion_node::connect::ConnectionRequest>, Error> {
        let row = self
            .sql
            .query_row(
                "SELECT id,secret_verifier,name,redirect,web_origin,client,challenge,state,grants,provider_manifest,provider_replace,status,created_at,expires_at,approved_at,completed_at,app,provider FROM connection_requests WHERE id=?1",
                [id.0],
                connection_row,
            )
            .optional()
            .map_err(db)?;
        row.map(connection_from).transpose()
    }
    fn connection_requests(
        &mut self,
    ) -> Result<Vec<harmion_node::connect::ConnectionRequest>, Error> {
        let mut statement = self
            .sql
            .prepare(
                "SELECT id,secret_verifier,name,redirect,web_origin,client,challenge,state,grants,provider_manifest,provider_replace,status,created_at,expires_at,approved_at,completed_at,app,provider FROM connection_requests ORDER BY created_at DESC,id",
            )
            .map_err(db)?;
        let rows = statement
            .query_map([], connection_row)
            .map_err(db)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db)?;
        rows.into_iter().map(connection_from).collect()
    }
    fn insert_connection_request(
        &mut self,
        request: &harmion_node::connect::ConnectionRequest,
    ) -> Result<(), Error> {
        let grants =
            serde_json::to_string(&harmion_protocol::GrantSpec::from(request.grants.clone()))
                .map_err(|_| Error::InvalidRequest)?;
        self.sql
            .execute(
                "INSERT INTO connection_requests(id,secret_verifier,name,redirect,web_origin,client,challenge,state,grants,provider_manifest,provider_replace,status,created_at,expires_at,approved_at,completed_at,app,provider) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)",
                params![
                    request.id.0,
                    request.secret_verifier,
                    request.name,
                    request.redirect,
                    request.web_origin,
                    request.client.map(|app| app.0),
                    request.challenge,
                    request.state,
                    grants,
                    request.provider_manifest,
                    request.provider_replace,
                    request.status.storage(),
                    request.created_at,
                    request.expires_at,
                    request.approved_at,
                    request.completed_at,
                    request.app.map(|app| app.0),
                    request.provider_id.map(|provider| provider.0),
                ],
            )
            .map_err(db)?;
        Ok(())
    }
    fn update_connection_request(
        &mut self,
        request: &harmion_node::connect::ConnectionRequest,
    ) -> Result<(), Error> {
        let grants =
            serde_json::to_string(&harmion_protocol::GrantSpec::from(request.grants.clone()))
                .map_err(|_| Error::InvalidRequest)?;
        let changed = self
            .sql
            .execute(
                "UPDATE connection_requests SET secret_verifier=?2,name=?3,redirect=?4,web_origin=?5,client=?6,challenge=?7,state=?8,grants=?9,provider_manifest=?10,provider_replace=?11,status=?12,expires_at=?13,approved_at=?14,completed_at=?15,app=?16,provider=?17 WHERE id=?1",
                params![
                    request.id.0,
                    request.secret_verifier,
                    request.name,
                    request.redirect,
                    request.web_origin,
                    request.client.map(|app| app.0),
                    request.challenge,
                    request.state,
                    grants,
                    request.provider_manifest,
                    request.provider_replace,
                    request.status.storage(),
                    request.expires_at,
                    request.approved_at,
                    request.completed_at,
                    request.app.map(|app| app.0),
                    request.provider_id.map(|provider| provider.0),
                ],
            )
            .map_err(db)?;
        if changed != 1 {
            return Err(Error::NotFound);
        }
        Ok(())
    }
    fn connection_request_count(&mut self, now: i64) -> Result<usize, Error> {
        let count: i64 = self
            .sql
            .query_row(
                "SELECT COUNT(*) FROM connection_requests WHERE status IN (0,1) AND expires_at>?1",
                [now],
                |row| row.get(0),
            )
            .map_err(db)?;
        count.try_into().map_err(|_| Error::CorruptLog)
    }
    fn prune_connection_requests(&mut self, before: i64) -> Result<usize, Error> {
        self.sql
            .execute(
                "DELETE FROM connection_requests WHERE status IN (2,3,4) AND created_at<?1",
                [before],
            )
            .map_err(db)
    }

    fn grant_generation(&mut self, app: AppId) -> Result<u64, Error> {
        changes::grant_generation(self.sql, app)
    }
    fn token(&mut self, token: EditToken) -> Result<Option<TokenState>, Error> {
        let row = self
            .sql
            .query_row(
                "SELECT content,purpose,created_at FROM edit_tokens WHERE token=?1",
                [token.0],
                |r| {
                    Ok((
                        r.get::<_, Vec<u8>>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(db)?;
        let Some((content, purpose, created_at)) = row else {
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
            created_at,
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
                "INSERT INTO edit_tokens(token,content,purpose,created_at) VALUES(?1,?2,?3,?4)",
                params![token.0, state.content_id.0, purpose, state.created_at],
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
        let row = self.sql.query_row("SELECT request,update_id,snapshot_token,return_view,created_at FROM request_results WHERE app_id=?1 AND request_id=?2",
            params![app.0, request.0], |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, Vec<u8>>(1)?, r.get::<_, Vec<u8>>(2)?, r.get::<_, bool>(3)?, r.get::<_, i64>(4)?))).optional().map_err(db)?;
        row.map(|(request, update, token, return_view, created_at)| {
            Ok(RequestResult {
                request,
                update_id: UpdateId(fixed(update)?),
                snapshot_token: EditToken(fixed(token)?),
                return_view,
                created_at,
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
                "INSERT INTO request_results(app_id,request_id,request,update_id,snapshot_token,return_view,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![
                    app.0,
                    request.0,
                    result.request,
                    result.update_id.0,
                    result.snapshot_token.0,
                    result.return_view,
                    result.created_at
                ],
            )
            .map_err(db)?;
        Ok(())
    }
    fn gc_tokens(&mut self, cutoff: i64) -> Result<u64, Error> {
        // Receipts first: a live receipt keeps its snapshot token and basis.
        // Rows created before the retention policy carry created_at = 0 and are
        // deliberately kept rather than silently deleted.
        self.sql
            .execute(
                "DELETE FROM request_results WHERE created_at > 0 AND created_at < ?1",
                [cutoff],
            )
            .map_err(db)?;
        self.sql
            .execute(
                "DELETE FROM token_basis WHERE token IN (
                    SELECT token FROM edit_tokens
                    WHERE created_at > 0 AND created_at < ?1
                      AND NOT EXISTS (SELECT 1 FROM request_results WHERE snapshot_token=edit_tokens.token)
                )",
                [cutoff],
            )
            .map_err(db)?;
        self.sql
            .execute(
                "DELETE FROM edit_tokens
                 WHERE created_at > 0 AND created_at < ?1
                   AND NOT EXISTS (SELECT 1 FROM request_results WHERE snapshot_token=edit_tokens.token)",
                [cutoff],
            )
            .map_err(db)
            .map(|count| count as u64)
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
