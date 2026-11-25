//! Peer-only node `Store` over the Durable Object SQL host boundary.
//!
//! This adapter is intentionally narrow: it implements the records, cursors,
//! membership log, peers and identity pins that Peer synchronization needs.
//! App/Query/OAuth/Subscription methods fail closed instead of silently
//! pretending the data exists. Record bytes are kept verbatim; positions are
//! 8-byte big-endian BLOBs so 64-bit values never pass through a JS Number.
use crate::sql::{SqlHost, SqlTransaction, SqlValue, blob_u64};
use harmion_domain::{ContentId, NodeId, UpdateId};
use harmion_node::{
    Error,
    access::{App, Filter},
    application::{RequestResult, TokenState},
    identity::{KnownIdentity, Provenance},
    membership::{Peer, PeerKey},
    query::{QueryPosition, QueryRevision},
    storage::{ChangeWindow, ContentChange, Identity, Store, Transaction},
};
use harmion_protocol::membership::Certificate;
use harmion_protocol::{
    AppId, EditToken, LogEntry, LogEpoch, MAX_PAGE_ENTRIES, MAX_PAGE_RECORD_BYTES,
    MAX_RECORD_BYTES, Record, RequestId,
    query::{Condition, QueryCursor},
};

const UNSUPPORTED: &str = "not supported by the peer-only Worker store";

pub struct WorkerStore<H: SqlHost> {
    host: H,
    identity: Identity,
}

const SCHEMA: [&str; 13] = [
    "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value BLOB NOT NULL)",
    "CREATE TABLE IF NOT EXISTS accept_log (seq BLOB PRIMARY KEY, update_id BLOB NOT NULL UNIQUE, bytes BLOB NOT NULL, digest BLOB NOT NULL)",
    "CREATE TABLE IF NOT EXISTS content_records (content BLOB NOT NULL, seq BLOB NOT NULL, PRIMARY KEY(content,seq))",
    "CREATE TABLE IF NOT EXISTS peer_cursors (sender BLOB NOT NULL, epoch BLOB NOT NULL, through BLOB NOT NULL, PRIMARY KEY(sender,epoch))",
    "CREATE TABLE IF NOT EXISTS peers (node BLOB PRIMARY KEY, key_kind BLOB NOT NULL, key_bytes BLOB NOT NULL, endpoint TEXT NOT NULL, retired BLOB NOT NULL, UNIQUE(key_kind,key_bytes))",
    "CREATE TABLE IF NOT EXISTS identity_pins (node BLOB PRIMARY KEY, signing_key BLOB NOT NULL UNIQUE, key_kind BLOB NOT NULL, key_bytes BLOB NOT NULL, provenance_kind BLOB NOT NULL, provenance_id BLOB, UNIQUE(key_kind,key_bytes))",
    "CREATE TABLE IF NOT EXISTS membership_log (seq BLOB PRIMARY KEY, id BLOB NOT NULL UNIQUE, bytes BLOB NOT NULL)",
    "CREATE TABLE IF NOT EXISTS membership_cursors (sender BLOB NOT NULL, epoch BLOB NOT NULL, through BLOB NOT NULL, PRIMARY KEY(sender,epoch))",
    "CREATE TABLE IF NOT EXISTS membership_recovery_roots (id BLOB PRIMARY KEY, bytes BLOB NOT NULL)",
    "CREATE TABLE IF NOT EXISTS sync_observations (node BLOB PRIMARY KEY, value BLOB NOT NULL)",
    "CREATE TABLE IF NOT EXISTS sync_failures (node BLOB PRIMARY KEY, value BLOB NOT NULL)",
    "CREATE TABLE IF NOT EXISTS schema_marker (id INTEGER PRIMARY KEY CHECK(id=1))",
    "INSERT OR IGNORE INTO schema_marker VALUES(1)",
];

impl<H: SqlHost> WorkerStore<H> {
    /// Creates the peer-only schema once and verifies the durable node identity.
    pub fn open(mut host: H, identity: Identity) -> Result<Self, Error> {
        let mut failure = None;
        let outcome = host.transaction(|sql| {
            let result = (|| -> Result<(), Error> {
                for statement in SCHEMA {
                    sql.execute(statement, &[]).map_err(host_error)?;
                }
                let expected = identity_bytes(identity);
                let mut rows = sql
                    .query("SELECT value FROM meta WHERE key='identity'", &[])
                    .map_err(host_error)?;
                match rows.pop() {
                    Some(row) => {
                        if value(&row, 0)? != expected {
                            return Err(Error::IdentityMismatch);
                        }
                    }
                    None => {
                        sql.execute(
                            "INSERT INTO meta(key,value) VALUES('identity',?)",
                            &[SqlValue::Blob(expected)],
                        )
                        .map_err(host_error)?;
                    }
                }
                Ok(())
            })();
            match result {
                Ok(()) => Ok(()),
                Err(error) => {
                    failure = Some(error);
                    Err(host_error_msg("could not open the Worker store"))
                }
            }
        });
        match outcome {
            Ok(()) => Ok(Self { host, identity }),
            Err(error) => Err(failure.unwrap_or_else(|| host_error(error))),
        }
    }

    /// Physical durability gate. The host must call it after a committed
    /// transaction and before releasing an ACK.
    pub fn sync(&mut self) -> Result<(), Error> {
        self.host.sync().map_err(host_error)
    }

    pub fn identity(&self) -> Identity {
        self.identity
    }
}

impl<H: SqlHost> Store for WorkerStore<H> {
    fn transaction<T>(
        &mut self,
        f: impl FnOnce(&mut dyn Transaction) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let identity = self.identity;
        let mut failure = None;
        let outcome = self.host.transaction(|sql| {
            let mut transaction = WorkerTransaction { sql, identity };
            match f(&mut transaction) {
                Ok(value) => Ok(Some(value)),
                Err(error) => {
                    failure = Some(error);
                    Err(host_error_msg("node transaction failed"))
                }
            }
        });
        match outcome {
            Ok(Some(value)) => Ok(value),
            Ok(None) => Err(Error::Storage("missing transaction result".into())),
            Err(error) => Err(failure.unwrap_or_else(|| host_error(error))),
        }
    }
}

fn host_error(error: crate::kv::HostError) -> Error {
    Error::Storage(error.0)
}
fn host_error_msg(message: &str) -> crate::kv::HostError {
    crate::kv::HostError(message.into())
}
fn identity_bytes(identity: Identity) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(96);
    bytes.extend_from_slice(&identity.cluster.0);
    bytes.extend_from_slice(&identity.node.0);
    bytes.extend_from_slice(&identity.epoch.0);
    bytes
}
fn value(row: &[SqlValue], index: usize) -> Result<Vec<u8>, Error> {
    match row.get(index) {
        Some(SqlValue::Blob(bytes)) => Ok(bytes.clone()),
        _ => Err(Error::CorruptLog),
    }
}
fn text(row: &[SqlValue], index: usize) -> Result<String, Error> {
    match row.get(index) {
        Some(SqlValue::Text(value)) => Ok(value.clone()),
        _ => Err(Error::CorruptLog),
    }
}
fn small(row: &[SqlValue], index: usize) -> Result<u8, Error> {
    match row.get(index) {
        Some(SqlValue::Int(value)) => u8::try_from(*value).map_err(|_| Error::CorruptLog),
        Some(SqlValue::Blob(bytes)) if bytes.len() == 1 => Ok(bytes[0]),
        _ => Err(Error::CorruptLog),
    }
}
fn position(row: &[SqlValue], index: usize) -> Result<u64, Error> {
    let bytes = value(row, index)?;
    Ok(u64::from_be_bytes(
        bytes.try_into().map_err(|_| Error::CorruptLog)?,
    ))
}
fn fixed(bytes: Vec<u8>) -> Result<[u8; 32], Error> {
    bytes.try_into().map_err(|_| Error::CorruptLog)
}
fn byte(value: u8) -> SqlValue {
    SqlValue::Blob(vec![value])
}

struct WorkerTransaction<'a> {
    sql: &'a mut dyn SqlTransaction,
    identity: Identity,
}

impl WorkerTransaction<'_> {
    fn get(&mut self, key: &str) -> Result<Option<Vec<u8>>, Error> {
        let mut rows = self
            .sql
            .query(
                "SELECT value FROM meta WHERE key=?",
                &[SqlValue::Text(key.into())],
            )
            .map_err(host_error)?;
        rows.pop().map(|row| value(&row, 0)).transpose()
    }
    fn put(&mut self, key: &str, bytes: Vec<u8>) -> Result<(), Error> {
        self.sql
            .execute(
                "INSERT INTO meta(key,value) VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                &[SqlValue::Text(key.into()), SqlValue::Blob(bytes)],
            )
            .map_err(host_error)?;
        Ok(())
    }
    fn high(&mut self, table: &str) -> Result<u64, Error> {
        // Table name is a fixed internal literal, never caller input.
        let sql = format!("SELECT seq FROM {table} ORDER BY seq DESC LIMIT 1");
        let mut rows = self.sql.query(&sql, &[]).map_err(host_error)?;
        rows.pop()
            .map(|row| position(&row, 0))
            .transpose()
            .map(|v| v.unwrap_or(0))
    }
    fn read_records(&mut self, sql: &str, params: &[SqlValue]) -> Result<Vec<LogEntry>, Error> {
        let rows = self.sql.query(sql, params).map_err(host_error)?;
        let mut entries = Vec::with_capacity(rows.len());
        for row in rows {
            let sequence = position(&row, 0)?;
            let id = fixed(value(&row, 1)?)?;
            let bytes = value(&row, 2)?;
            let digest = fixed(value(&row, 3)?)?;
            let record = Record::from_bytes(&bytes)?;
            if sequence == 0 || id != record.update().update_id.0 || digest != *record.digest() {
                return Err(Error::CorruptLog);
            }
            entries.push(LogEntry {
                seq: sequence,
                record,
            });
        }
        Ok(entries)
    }
}

impl Transaction for WorkerTransaction<'_> {
    fn identity(&self) -> Identity {
        self.identity
    }

    fn membership_identity(&mut self) -> Result<Option<Certificate>, Error> {
        self.get("membership_identity")?
            .map(|bytes| Certificate::from_bytes(&bytes).map_err(Into::into))
            .transpose()
    }
    fn set_membership_identity(&mut self, identity: &Certificate) -> Result<(), Error> {
        if let Some(previous) = self.membership_identity()? {
            if previous != *identity {
                return Err(Error::IdentityMismatch);
            }
            return Ok(());
        }
        self.put("membership_identity", identity.bytes().to_vec())
    }
    fn recovery_roots(&mut self) -> Result<Vec<Certificate>, Error> {
        let rows = self
            .sql
            .query(
                "SELECT id,bytes FROM membership_recovery_roots ORDER BY id",
                &[],
            )
            .map_err(host_error)?;
        rows.into_iter()
            .map(|row| {
                let certificate = Certificate::from_bytes(&value(&row, 1)?)?;
                if fixed(value(&row, 0)?)? != certificate.id() {
                    return Err(Error::CorruptLog);
                }
                Ok(certificate)
            })
            .collect()
    }
    fn insert_recovery_root(&mut self, root: &Certificate) -> Result<(), Error> {
        self.sql
            .execute(
                "INSERT INTO membership_recovery_roots(id,bytes) VALUES(?,?)",
                &[
                    SqlValue::Blob(root.id().to_vec()),
                    SqlValue::Blob(root.bytes().to_vec()),
                ],
            )
            .map_err(host_error)?;
        Ok(())
    }
    fn membership_root(&mut self) -> Result<Option<Certificate>, Error> {
        self.get("membership_root")?
            .map(|bytes| Certificate::from_bytes(&bytes).map_err(Into::into))
            .transpose()
    }
    fn set_membership_root(&mut self, root: &Certificate) -> Result<(), Error> {
        if let Some(existing) = self.membership_root()? {
            if existing != *root {
                return Err(Error::IdentityMismatch);
            }
            return Ok(());
        }
        self.put("membership_root", root.bytes().to_vec())
    }
    fn membership_entries(&mut self) -> Result<Vec<LogEntry<Certificate>>, Error> {
        let rows = self
            .sql
            .query("SELECT seq,id,bytes FROM membership_log ORDER BY seq", &[])
            .map_err(host_error)?;
        let mut entries = Vec::with_capacity(rows.len());
        for row in rows {
            let sequence = position(&row, 0)?;
            let certificate = Certificate::from_bytes(&value(&row, 2)?)?;
            if sequence == 0 || fixed(value(&row, 1)?)? != certificate.id() {
                return Err(Error::CorruptLog);
            }
            entries.push(LogEntry {
                seq: sequence,
                record: certificate,
            });
        }
        Ok(entries)
    }
    fn append_membership(&mut self, certificate: &Certificate) -> Result<u64, Error> {
        let mut rows = self
            .sql
            .query(
                "SELECT seq,bytes FROM membership_log WHERE id=?",
                &[SqlValue::Blob(certificate.id().to_vec())],
            )
            .map_err(host_error)?;
        if let Some(row) = rows.pop() {
            if value(&row, 1)? != certificate.bytes() {
                return Err(Error::IdentityMismatch);
            }
            return position(&row, 0);
        }
        let sequence = self.high("membership_log")? + 1;
        self.sql
            .execute(
                "INSERT INTO membership_log(seq,id,bytes) VALUES(?,?,?)",
                &[
                    blob_u64(sequence),
                    SqlValue::Blob(certificate.id().to_vec()),
                    SqlValue::Blob(certificate.bytes().to_vec()),
                ],
            )
            .map_err(host_error)?;
        Ok(sequence)
    }
    fn membership_cursor(&mut self, sender: NodeId, epoch: LogEpoch) -> Result<u64, Error> {
        let mut rows = self
            .sql
            .query(
                "SELECT through FROM membership_cursors WHERE sender=? AND epoch=?",
                &[
                    SqlValue::Blob(sender.0.to_vec()),
                    SqlValue::Blob(epoch.0.to_vec()),
                ],
            )
            .map_err(host_error)?;
        rows.pop()
            .map(|row| position(&row, 0))
            .transpose()
            .map(|v| v.unwrap_or(0))
    }
    fn set_membership_cursor(
        &mut self,
        sender: NodeId,
        epoch: LogEpoch,
        through: u64,
    ) -> Result<(), Error> {
        self.sql
            .execute(
                "INSERT INTO membership_cursors(sender,epoch,through) VALUES(?,?,?)
                 ON CONFLICT(sender,epoch) DO UPDATE SET through=excluded.through",
                &[
                    SqlValue::Blob(sender.0.to_vec()),
                    SqlValue::Blob(epoch.0.to_vec()),
                    blob_u64(through),
                ],
            )
            .map_err(host_error)?;
        Ok(())
    }

    fn peer(&mut self, node: NodeId) -> Result<Option<Peer>, Error> {
        let mut rows = self
            .sql
            .query(
                "SELECT key_kind,key_bytes,endpoint,retired FROM peers WHERE node=?",
                &[SqlValue::Blob(node.0.to_vec())],
            )
            .map_err(host_error)?;
        rows.pop()
            .map(|row| {
                Ok(Peer {
                    node,
                    key: PeerKey::from_parts(small(&row, 0)?, value(&row, 1)?)?,
                    endpoint: text(&row, 2)?,
                    retired: small(&row, 3)? != 0,
                })
            })
            .transpose()
    }
    fn peers(&mut self) -> Result<Vec<Peer>, Error> {
        let rows = self
            .sql
            .query("SELECT node FROM peers ORDER BY node", &[])
            .map_err(host_error)?;
        rows.into_iter()
            .map(|row| {
                let node = NodeId(fixed(value(&row, 0)?)?);
                self.peer(node)?.ok_or(Error::CorruptLog)
            })
            .collect()
    }
    fn put_peer(&mut self, peer: &Peer) -> Result<(), Error> {
        self.sql
            .execute(
                "INSERT INTO peers(node,key_kind,key_bytes,endpoint,retired) VALUES(?,?,?,?,?)
                 ON CONFLICT(node) DO UPDATE SET key_kind=excluded.key_kind,
                   key_bytes=excluded.key_bytes,endpoint=excluded.endpoint,retired=excluded.retired",
                &[
                    SqlValue::Blob(peer.node.0.to_vec()),
                    byte(peer.key.kind()),
                    SqlValue::Blob(peer.key.bytes().to_vec()),
                    SqlValue::Text(peer.endpoint.clone()),
                    byte(u8::from(peer.retired)),
                ],
            )
            .map_err(host_error)?;
        Ok(())
    }

    fn known_peer_identity(&mut self, node: NodeId) -> Result<Option<KnownIdentity>, Error> {
        let mut rows = self
            .sql
            .query(
                "SELECT signing_key,key_kind,key_bytes,provenance_kind,provenance_id
                 FROM identity_pins WHERE node=?",
                &[SqlValue::Blob(node.0.to_vec())],
            )
            .map_err(host_error)?;
        let Some(row) = rows.pop() else {
            return Ok(None);
        };
        let evidence = match row.get(4) {
            Some(SqlValue::Blob(bytes)) if !bytes.is_empty() => Some(fixed(bytes.clone())?),
            Some(SqlValue::Null) | None => None,
            _ => return Err(Error::CorruptLog),
        };
        Ok(Some(KnownIdentity {
            node,
            signing_key: fixed(value(&row, 0)?)?,
            key: PeerKey::from_parts(small(&row, 1)?, value(&row, 2)?)?,
            provenance: Provenance::from_parts(small(&row, 3)?, evidence)?,
        }))
    }
    fn known_peer_identities(&mut self) -> Result<Vec<KnownIdentity>, Error> {
        let rows = self
            .sql
            .query("SELECT node FROM identity_pins ORDER BY node", &[])
            .map_err(host_error)?;
        rows.into_iter()
            .map(|row| {
                let node = NodeId(fixed(value(&row, 0)?)?);
                self.known_peer_identity(node)?.ok_or(Error::CorruptLog)
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
                "INSERT INTO identity_pins(node,signing_key,key_kind,key_bytes,provenance_kind,provenance_id)
                 VALUES(?,?,?,?,?,?)",
                &[
                    SqlValue::Blob(identity.node.0.to_vec()),
                    SqlValue::Blob(identity.signing_key.to_vec()),
                    byte(identity.key.kind()),
                    SqlValue::Blob(identity.key.bytes().to_vec()),
                    byte(identity.provenance.kind()),
                    match identity.provenance.certificate() {
                        Some(id) => SqlValue::Blob(id.to_vec()),
                        None => SqlValue::Null,
                    },
                ],
            )
            .map_err(host_error)?;
        Ok(())
    }

    fn entries(&mut self) -> Result<Vec<LogEntry>, Error> {
        self.read_records(
            "SELECT seq,update_id,bytes,digest FROM accept_log ORDER BY seq",
            &[],
        )
    }
    fn content_entries(
        &mut self,
        content: ContentId,
        through: Option<u64>,
    ) -> Result<Vec<LogEntry>, Error> {
        let entries = self.read_records(
            "SELECT log.seq,log.update_id,log.bytes,log.digest
             FROM content_records AS records JOIN accept_log AS log ON log.seq=records.seq
             WHERE records.content=? AND records.seq<=? ORDER BY records.seq",
            &[
                SqlValue::Blob(content.0.to_vec()),
                blob_u64(through.unwrap_or(u64::MAX)),
            ],
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
        self.high("accept_log")
    }
    fn accepted_page(&mut self, after: u64, high: u64) -> Result<Vec<LogEntry>, Error> {
        let mut entries = self.read_records(
            "SELECT seq,update_id,bytes,digest FROM accept_log
             WHERE seq>? AND seq<=? ORDER BY seq LIMIT ?",
            &[
                blob_u64(after),
                blob_u64(high),
                SqlValue::Int(MAX_PAGE_ENTRIES as i64),
            ],
        )?;
        // Keep the first record even if it exceeds the budget, matching the
        // native adapter; never send more than the page byte budget.
        let mut bytes = 0;
        let mut keep = 0;
        for entry in &entries {
            let length = entry.record.bytes().len();
            if length > MAX_RECORD_BYTES {
                return Err(Error::InvalidPage);
            }
            if keep > 0 && bytes + length > MAX_PAGE_RECORD_BYTES {
                break;
            }
            bytes += length;
            keep += 1;
        }
        entries.truncate(keep);
        Ok(entries)
    }
    fn accepted_record(&mut self, id: UpdateId) -> Result<Option<Record>, Error> {
        let mut entries = self.read_records(
            "SELECT seq,update_id,bytes,digest FROM accept_log WHERE update_id=?",
            &[SqlValue::Blob(id.0.to_vec())],
        )?;
        Ok(entries.pop().map(|entry| entry.record))
    }
    fn accepted_sequence(&mut self, id: UpdateId) -> Result<Option<u64>, Error> {
        let mut rows = self
            .sql
            .query(
                "SELECT seq FROM accept_log WHERE update_id=?",
                &[SqlValue::Blob(id.0.to_vec())],
            )
            .map_err(host_error)?;
        rows.pop().map(|row| position(&row, 0)).transpose()
    }
    fn accepted_count_after(&mut self, after: u64) -> Result<u64, Error> {
        let rows = self
            .sql
            .query(
                "SELECT COUNT(*) FROM accept_log WHERE seq>?",
                &[blob_u64(after)],
            )
            .map_err(host_error)?;
        match rows.first().and_then(|row| row.first()) {
            Some(SqlValue::Int(count)) if *count >= 0 => Ok(*count as u64),
            _ => Err(Error::CorruptLog),
        }
    }
    fn append(&mut self, record: &Record) -> Result<u64, Error> {
        let sequence = self.high("accept_log")? + 1;
        self.sql
            .execute(
                "INSERT INTO accept_log(seq,update_id,bytes,digest) VALUES(?,?,?,?)",
                &[
                    blob_u64(sequence),
                    SqlValue::Blob(record.update().update_id.0.to_vec()),
                    SqlValue::Blob(record.bytes().to_vec()),
                    SqlValue::Blob(record.digest().to_vec()),
                ],
            )
            .map_err(host_error)?;
        self.sql
            .execute(
                "INSERT INTO content_records(content,seq) VALUES(?,?)",
                &[
                    SqlValue::Blob(record.update().content_id.0.to_vec()),
                    blob_u64(sequence),
                ],
            )
            .map_err(host_error)?;
        Ok(sequence)
    }
    fn put_search_projection(
        &mut self,
        _snapshot: &harmion_domain::ContentSnapshot,
    ) -> Result<(), Error> {
        // Peer-only replica: the search/subscription projection is not built.
        Ok(())
    }
    fn cursor(&mut self, sender: NodeId, epoch: LogEpoch) -> Result<u64, Error> {
        let mut rows = self
            .sql
            .query(
                "SELECT through FROM peer_cursors WHERE sender=? AND epoch=?",
                &[
                    SqlValue::Blob(sender.0.to_vec()),
                    SqlValue::Blob(epoch.0.to_vec()),
                ],
            )
            .map_err(host_error)?;
        rows.pop()
            .map(|row| position(&row, 0))
            .transpose()
            .map(|v| v.unwrap_or(0))
    }
    fn set_cursor(&mut self, sender: NodeId, epoch: LogEpoch, through: u64) -> Result<(), Error> {
        self.sql
            .execute(
                "INSERT INTO peer_cursors(sender,epoch,through) VALUES(?,?,?)
                 ON CONFLICT(sender,epoch) DO UPDATE SET through=excluded.through",
                &[
                    SqlValue::Blob(sender.0.to_vec()),
                    SqlValue::Blob(epoch.0.to_vec()),
                    blob_u64(through),
                ],
            )
            .map_err(host_error)?;
        Ok(())
    }

    // ---- App/Query/OAuth/Subscription surface: fail closed ------------------
    fn node_metrics(&mut self, _now: i64) -> Result<harmion_protocol::metrics::NodeMetrics, Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
    fn export_content_ids(
        &mut self,
        _through: u64,
        _after: Option<ContentId>,
        _limit: u32,
    ) -> Result<Vec<ContentId>, Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
    fn change_window(&mut self) -> Result<ChangeWindow, Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
    fn changes_after(&mut self, _after: u64, _limit: u32) -> Result<Vec<ContentChange>, Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
    fn search_content_ids(
        &mut self,
        _condition: &Condition,
        _read: &Filter,
        _after: Option<ContentId>,
        _limit: u32,
    ) -> Result<Vec<ContentId>, Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
    fn freeze_query(
        &mut self,
        _start: QueryCursor,
        _app: AppId,
        _condition: &Condition,
        _read: &Filter,
        _now: i64,
        _expires_at: i64,
    ) -> Result<(), Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
    fn query_position(&mut self, _cursor: QueryCursor) -> Result<Option<QueryPosition>, Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
    fn discard_query(&mut self, _session: QueryCursor) -> Result<(), Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
    fn query_revisions(
        &mut self,
        _position: &QueryPosition,
        _limit: u32,
    ) -> Result<Vec<QueryRevision>, Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
    fn put_query_cursor(
        &mut self,
        _session: QueryCursor,
        _after: ContentId,
        _cursor: QueryCursor,
    ) -> Result<QueryCursor, Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
    fn sync_observation(
        &mut self,
        _node: NodeId,
    ) -> Result<Option<harmion_node::sync_status::Observation>, Error> {
        Ok(None)
    }
    fn put_sync_observation(
        &mut self,
        _node: NodeId,
        _observation: &harmion_node::sync_status::Observation,
    ) -> Result<(), Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
    fn sync_failure(
        &mut self,
        _node: NodeId,
    ) -> Result<Option<harmion_node::sync_status::Failure>, Error> {
        Ok(None)
    }
    fn put_sync_failure(
        &mut self,
        _node: NodeId,
        _failure: Option<&harmion_node::sync_status::Failure>,
    ) -> Result<(), Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
    fn has_apps(&mut self) -> Result<bool, Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
    fn app_by_verifier(&mut self, _verifier: [u8; 32]) -> Result<Option<App>, Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
    fn app_by_id(&mut self, _id: AppId) -> Result<Option<App>, Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
    fn insert_app(&mut self, _app: &App) -> Result<(), Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
    fn apps(&mut self) -> Result<Vec<App>, Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
    fn update_app(&mut self, _app: &App) -> Result<(), Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
    fn grant_generation(&mut self, _app: AppId) -> Result<u64, Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
    fn token(&mut self, _token: EditToken) -> Result<Option<TokenState>, Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
    fn insert_token(&mut self, _token: EditToken, _state: &TokenState) -> Result<(), Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
    fn request(
        &mut self,
        _app: AppId,
        _request: RequestId,
    ) -> Result<Option<RequestResult>, Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
    fn insert_request(
        &mut self,
        _app: AppId,
        _request: RequestId,
        _result: &RequestResult,
    ) -> Result<(), Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
    fn gc_tokens(&mut self, _cutoff: i64) -> Result<u64, Error> {
        Ok(0)
    }
}

// Peer-only replica: no OAuth surface is exposed.
impl harmion_node::oauth::Storage for WorkerTransaction<'_> {
    fn oauth_client(&mut self, _id: AppId) -> Result<Option<harmion_node::oauth::Client>, Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
    fn put_oauth_client(&mut self, _client: &harmion_node::oauth::Client) -> Result<(), Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
    fn oauth_pending(
        &mut self,
        _request: [u8; 32],
    ) -> Result<Option<harmion_node::oauth::Authorization>, Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
    fn oauth_pending_count(&mut self, _now: i64) -> Result<usize, Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
    fn put_oauth_pending(
        &mut self,
        _authorization: &harmion_node::oauth::Authorization,
    ) -> Result<(), Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
    fn delete_oauth_pending(&mut self, _request: [u8; 32]) -> Result<(), Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
    fn oauth_code(
        &mut self,
        _digest: [u8; 32],
    ) -> Result<Option<harmion_node::oauth::Code>, Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
    fn put_oauth_code(
        &mut self,
        _digest: [u8; 32],
        _code: &harmion_node::oauth::Code,
    ) -> Result<(), Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
    fn consume_oauth_code(&mut self, _digest: [u8; 32], _family: [u8; 32]) -> Result<(), Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
    fn oauth_access(
        &mut self,
        _verifier: [u8; 32],
    ) -> Result<Option<harmion_node::oauth::Access>, Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
    fn put_oauth_access(
        &mut self,
        _verifier: [u8; 32],
        _family: [u8; 32],
        _expires_at: i64,
    ) -> Result<(), Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
    fn oauth_refresh(
        &mut self,
        _digest: [u8; 32],
    ) -> Result<Option<harmion_node::oauth::Refresh>, Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
    fn put_oauth_refresh(&mut self, _digest: [u8; 32], _family: [u8; 32]) -> Result<(), Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
    fn consume_oauth_refresh(&mut self, _digest: [u8; 32]) -> Result<(), Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
    fn put_oauth_family(
        &mut self,
        _family: [u8; 32],
        _app: AppId,
        _expires_at: i64,
    ) -> Result<(), Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
    fn revoke_oauth_family(&mut self, _family: [u8; 32]) -> Result<(), Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
    fn revoke_oauth_app(&mut self, _app: AppId) -> Result<(), Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
    fn prune_oauth(&mut self, _now: i64) -> Result<(), Error> {
        Err(Error::Storage(UNSUPPORTED.into()))
    }
}
