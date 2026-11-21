use crate::Error;
use crate::membership::Peer;
use crate::query::{QueryPosition, QueryRevision};
use crate::{
    access::{App, Filter},
    application::{RequestResult, TokenState},
};
use harmion_domain::{
    Acceptance, Action, ClusterId, ContentId, ContentSnapshot, Graph, NodeId, UpdateId,
};
use harmion_protocol::{
    AppId, EditToken, RequestId,
    query::{Condition, QueryCursor},
};
use harmion_protocol::{LogEntry, LogEpoch, Record};
use std::collections::{BTreeMap, btree_map::Entry};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    pub cluster: ClusterId,
    pub node: NodeId,
    pub epoch: LogEpoch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChangeWindow {
    /// A consumer below this position must take a new snapshot.
    pub discarded_through: u64,
    pub high: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContentChange {
    pub sequence: u64,
    pub content: ContentId,
}

pub const MAX_RETAINED_CHANGES: u64 = 4096;

/// No await or external side effects inside the callback. The adapter must roll
/// back all writes on Err/panic and return Ok only after a successful commit.
pub trait Store {
    fn transaction<T>(
        &mut self,
        f: impl FnOnce(&mut dyn Transaction) -> Result<T, Error>,
    ) -> Result<T, Error>;
    /// Reclaims edit tokens and request receipts older than `cutoff` (micros).
    /// Rows created before the retention policy (created_at = 0) are kept.
    /// Returns the number of edit tokens removed.
    fn gc_tokens(&mut self, cutoff: i64) -> Result<u64, Error> {
        self.transaction(|tx| tx.gc_tokens(cutoff))
    }
}

/// Storage operations, not application policy. Called only within a transaction.
/// Full-log reads are the initial reference implementation, not an indexing plan.
pub trait Transaction: crate::oauth::Storage {
    fn membership_identity(
        &mut self,
    ) -> Result<Option<harmion_protocol::membership::Certificate>, Error>;
    fn set_membership_identity(
        &mut self,
        identity: &harmion_protocol::membership::Certificate,
    ) -> Result<(), Error>;
    fn recovery_roots(&mut self) -> Result<Vec<harmion_protocol::membership::Certificate>, Error>;
    fn insert_recovery_root(
        &mut self,
        root: &harmion_protocol::membership::Certificate,
    ) -> Result<(), Error>;
    fn membership_root(
        &mut self,
    ) -> Result<Option<harmion_protocol::membership::Certificate>, Error>;
    fn set_membership_root(
        &mut self,
        root: &harmion_protocol::membership::Certificate,
    ) -> Result<(), Error>;
    fn membership_entries(
        &mut self,
    ) -> Result<Vec<LogEntry<harmion_protocol::membership::Certificate>>, Error>;
    fn append_membership(
        &mut self,
        certificate: &harmion_protocol::membership::Certificate,
    ) -> Result<u64, Error>;
    fn membership_cursor(&mut self, sender: NodeId, epoch: LogEpoch) -> Result<u64, Error>;
    fn set_membership_cursor(
        &mut self,
        sender: NodeId,
        epoch: LogEpoch,
        through: u64,
    ) -> Result<(), Error>;
    fn identity(&self) -> Identity;
    fn entries(&mut self) -> Result<Vec<LogEntry>, Error>;
    /// All accepted records for one Content, in local acceptance order.
    fn content_entries(
        &mut self,
        content: ContentId,
        through: Option<u64>,
    ) -> Result<Vec<LogEntry>, Error>;
    fn accepted_high(&mut self) -> Result<u64, Error>;
    fn node_metrics(&mut self, now: i64) -> Result<harmion_protocol::metrics::NodeMetrics, Error>;
    /// Ordered prefix of (after, high] within the replication count and byte budgets.
    fn accepted_page(&mut self, after: u64, high: u64) -> Result<Vec<LogEntry>, Error>;
    fn accepted_record(&mut self, id: UpdateId) -> Result<Option<Record>, Error>;
    fn accepted_sequence(&mut self, id: UpdateId) -> Result<Option<u64>, Error>;
    /// Count records, not sequence distance: local acceptance logs may have holes.
    fn accepted_count_after(&mut self, after: u64) -> Result<u64, Error>;
    fn sync_observation(
        &mut self,
        node: NodeId,
    ) -> Result<Option<crate::sync_status::Observation>, Error>;
    fn put_sync_observation(
        &mut self,
        node: NodeId,
        observation: &crate::sync_status::Observation,
    ) -> Result<(), Error>;
    fn sync_failure(&mut self, node: NodeId) -> Result<Option<crate::sync_status::Failure>, Error>;
    /// None clears the last failure after a completed successful round.
    fn put_sync_failure(
        &mut self,
        node: NodeId,
        failure: Option<&crate::sync_status::Failure>,
    ) -> Result<(), Error>;
    /// Includes deleted Content and uses the fixed acceptance position.
    fn export_content_ids(
        &mut self,
        through: u64,
        after: Option<ContentId>,
        limit: u32,
    ) -> Result<Vec<ContentId>, Error>;
    fn append(&mut self, record: &Record) -> Result<u64, Error>;
    /// Maintains the derived search state in the update's acceptance transaction.
    fn put_search_projection(&mut self, snapshot: &ContentSnapshot) -> Result<(), Error>;
    fn change_window(&mut self) -> Result<ChangeWindow, Error>;
    fn changes_after(&mut self, after: u64, limit: u32) -> Result<Vec<ContentChange>, Error>;
    /// Match currently visible Content within the caller's authenticated Read
    /// Filter, ordered by ID and excluding IDs through `after`. The bound
    /// applies after filtering. This operation does not persist a snapshot.
    fn search_content_ids(
        &mut self,
        condition: &Condition,
        read: &Filter,
        after: Option<ContentId>,
        limit: u32,
    ) -> Result<Vec<ContentId>, Error>;
    /// Capture matching IDs and their edit bases within the current transaction.
    fn freeze_query(
        &mut self,
        start: QueryCursor,
        app: AppId,
        condition: &Condition,
        read: &Filter,
        now: i64,
        expires_at: i64,
    ) -> Result<(), Error>;
    fn query_position(&mut self, cursor: QueryCursor) -> Result<Option<QueryPosition>, Error>;
    fn discard_query(&mut self, session: QueryCursor) -> Result<(), Error>;
    fn query_revisions(
        &mut self,
        position: &QueryPosition,
        limit: u32,
    ) -> Result<Vec<QueryRevision>, Error>;
    /// Reuse an existing cursor for this position, including when a page is retried.
    fn put_query_cursor(
        &mut self,
        session: QueryCursor,
        after: ContentId,
        cursor: QueryCursor,
    ) -> Result<QueryCursor, Error>;
    fn cursor(&mut self, sender: NodeId, epoch: LogEpoch) -> Result<u64, Error>;
    fn set_cursor(&mut self, sender: NodeId, epoch: LogEpoch, through: u64) -> Result<(), Error>;
    fn has_apps(&mut self) -> Result<bool, Error>;
    fn app_by_verifier(&mut self, verifier: [u8; 32]) -> Result<Option<App>, Error>;
    fn app_by_id(&mut self, id: AppId) -> Result<Option<App>, Error>;
    fn insert_app(&mut self, app: &App) -> Result<(), Error>;
    fn update_app(&mut self, app: &App) -> Result<(), Error>;
    /// Interpreter registration is an optional host capability. Peer-only hosts
    /// fail closed instead of pretending an approvable local registry exists.
    fn interpreter_registration(
        &mut self,
        _provider: harmion_protocol::interpreter::ProviderId,
    ) -> Result<Option<crate::interpreter::Registration>, Error> {
        Err(Error::Unsupported)
    }
    fn interpreter_registration_by_app_name(
        &mut self,
        _app: AppId,
        _name: &str,
    ) -> Result<Option<crate::interpreter::Registration>, Error> {
        Err(Error::Unsupported)
    }
    fn interpreter_registrations(
        &mut self,
    ) -> Result<Vec<crate::interpreter::Registration>, Error> {
        Err(Error::Unsupported)
    }
    fn insert_interpreter_registration(
        &mut self,
        _registration: &crate::interpreter::Registration,
    ) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
    /// Updates manifest, name, and contract version, preserving approval state.
    fn update_interpreter_registration(
        &mut self,
        _registration: &crate::interpreter::Registration,
    ) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
    fn approved_interpreter_by_name(
        &mut self,
        _name: &str,
    ) -> Result<Option<crate::interpreter::Registration>, Error> {
        Err(Error::Unsupported)
    }
    fn set_interpreter_status(
        &mut self,
        _provider: harmion_protocol::interpreter::ProviderId,
        _status: harmion_protocol::interpreter::ProviderStatus,
        _approved_at: Option<i64>,
    ) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
    /// Monotonic local authorization generation, including grant changes that
    /// later restore the original values.
    fn grant_generation(&mut self, app: AppId) -> Result<u64, Error>;
    /// Integrated OAuth connection requests are a local App capability.
    fn connection_request(
        &mut self,
        _id: harmion_protocol::connect::ConnectionId,
    ) -> Result<Option<crate::connect::ConnectionRequest>, Error> {
        Err(Error::Unsupported)
    }
    fn connection_requests(&mut self) -> Result<Vec<crate::connect::ConnectionRequest>, Error> {
        Err(Error::Unsupported)
    }
    fn insert_connection_request(
        &mut self,
        _request: &crate::connect::ConnectionRequest,
    ) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
    fn update_connection_request(
        &mut self,
        _request: &crate::connect::ConnectionRequest,
    ) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
    fn connection_request_count(&mut self, _now: i64) -> Result<usize, Error> {
        Err(Error::Unsupported)
    }
    fn prune_connection_requests(&mut self, _before: i64) -> Result<usize, Error> {
        Err(Error::Unsupported)
    }
    fn token(&mut self, token: EditToken) -> Result<Option<TokenState>, Error>;
    fn insert_token(&mut self, token: EditToken, state: &TokenState) -> Result<(), Error>;
    fn request(&mut self, app: AppId, request: RequestId) -> Result<Option<RequestResult>, Error>;
    fn insert_request(
        &mut self,
        app: AppId,
        request: RequestId,
        result: &RequestResult,
    ) -> Result<(), Error>;
    /// Deletes receipts first, then basis rows, then unreferenced tokens.
    fn gc_tokens(&mut self, cutoff: i64) -> Result<u64, Error>;
    fn peer(&mut self, node: NodeId) -> Result<Option<Peer>, Error>;
    fn peers(&mut self) -> Result<Vec<Peer>, Error>;
    fn put_peer(&mut self, peer: &Peer) -> Result<(), Error>;
    /// Durable authentication identity, separate from current membership grants.
    fn known_peer_identity(
        &mut self,
        node: NodeId,
    ) -> Result<Option<crate::identity::KnownIdentity>, Error>;
    fn known_peer_identities(&mut self) -> Result<Vec<crate::identity::KnownIdentity>, Error>;
    /// Insert or accept an identical pin. Implementations must reject overwriting
    /// a pin and reusing a signing key or binding key for another Node ID.
    fn put_known_identity(
        &mut self,
        identity: &crate::identity::KnownIdentity,
    ) -> Result<(), Error>;
}

pub fn read_graph(tx: &mut dyn Transaction) -> Result<Graph, Error> {
    graph_from_entries(tx.identity().cluster, tx.entries()?)
}

pub(crate) fn read_content_graph(
    tx: &mut dyn Transaction,
    content: ContentId,
) -> Result<Graph, Error> {
    read_content_graph_through(tx, content, None)
}

pub(crate) fn read_content_graph_through(
    tx: &mut dyn Transaction,
    content: ContentId,
    through: Option<u64>,
) -> Result<Graph, Error> {
    graph_from_entries(tx.identity().cluster, tx.content_entries(content, through)?)
}

fn graph_from_entries(cluster: ClusterId, entries: Vec<LogEntry>) -> Result<Graph, Error> {
    let mut graph = Graph::new(cluster);
    let mut previous = 0;
    for entry in entries {
        if entry.seq <= previous {
            return Err(Error::CorruptLog);
        }
        previous = entry.seq;
        if graph.accept(entry.record.update().clone())? != Acceptance::Inserted {
            return Err(Error::CorruptLog);
        }
    }
    Ok(graph)
}

/// Semantically validates before appending. Callers must wrap this entire operation
/// and any associated cursor/request result writes in one Store::transaction.
/// App authentication/authorization must be added by application use cases.
pub fn accept_records(tx: &mut dyn Transaction, records: &[Record]) -> Result<usize, Error> {
    let mut graphs = BTreeMap::new();
    let mut inserted = 0;
    for record in records {
        let update = record.update();
        if let Some(existing) = tx.accepted_record(update.update_id)? {
            if existing.bytes() != record.bytes() {
                return Err(Error::RecordRejected {
                    update_id: update.update_id,
                    reason: harmion_domain::Error::IdMismatch,
                });
            }
            continue;
        }
        let graph = match graphs.entry(update.content_id) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert(read_content_graph(tx, update.content_id)?),
        };
        if let Err(mut reason) = graph.accept(update.clone()) {
            if let harmion_domain::Error::MissingDependency(id) = reason
                && tx.accepted_record(id)?.is_some()
            {
                reason = if update.action != Action::Create && id == update.root_id {
                    harmion_domain::Error::RootCollision
                } else {
                    harmion_domain::Error::InvalidBasis
                };
            }
            return Err(Error::RecordRejected {
                update_id: update.update_id,
                reason,
            });
        }
        tx.append(record)?;
        inserted += 1;
    }
    for (content, graph) in graphs {
        let snapshot = graph.snapshot(content).ok_or(Error::CorruptLog)?;
        tx.put_search_projection(&snapshot)?;
    }
    Ok(inserted)
}
