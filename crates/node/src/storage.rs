use crate::Error;
use crate::membership::Peer;
use crate::{
    access::App,
    application::{RequestResult, TokenState},
};
use harmion_domain::{Acceptance, ClusterId, Graph, NodeId};
use harmion_protocol::{AppId, EditToken, RequestId};
use harmion_protocol::{LogEntry, LogEpoch, Record};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    pub cluster: ClusterId,
    pub node: NodeId,
    pub epoch: LogEpoch,
}

/// No await or external side effects inside the callback. The adapter must roll
/// back all writes on Err/panic and return Ok only after a successful commit.
pub trait Store {
    fn transaction<T>(
        &mut self,
        f: impl FnOnce(&mut dyn Transaction) -> Result<T, Error>,
    ) -> Result<T, Error>;
}

/// Storage operations, not application policy. Called only within a transaction.
/// Full-log reads are the initial reference implementation, not an indexing plan.
pub trait Transaction {
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
    fn append(&mut self, record: &Record) -> Result<u64, Error>;
    fn cursor(&mut self, sender: NodeId, epoch: LogEpoch) -> Result<u64, Error>;
    fn set_cursor(&mut self, sender: NodeId, epoch: LogEpoch, through: u64) -> Result<(), Error>;
    fn has_apps(&mut self) -> Result<bool, Error>;
    fn app_by_verifier(&mut self, verifier: [u8; 32]) -> Result<Option<App>, Error>;
    fn app_by_id(&mut self, id: AppId) -> Result<Option<App>, Error>;
    fn insert_app(&mut self, app: &App) -> Result<(), Error>;
    fn update_app(&mut self, app: &App) -> Result<(), Error>;
    fn token(&mut self, token: EditToken) -> Result<Option<TokenState>, Error>;
    fn insert_token(&mut self, token: EditToken, state: &TokenState) -> Result<(), Error>;
    fn request(&mut self, app: AppId, request: RequestId) -> Result<Option<RequestResult>, Error>;
    fn insert_request(
        &mut self,
        app: AppId,
        request: RequestId,
        result: &RequestResult,
    ) -> Result<(), Error>;
    fn peer(&mut self, node: NodeId) -> Result<Option<Peer>, Error>;
    fn peers(&mut self) -> Result<Vec<Peer>, Error>;
    fn put_peer(&mut self, peer: &Peer) -> Result<(), Error>;
}

pub fn read_graph(tx: &mut dyn Transaction) -> Result<Graph, Error> {
    let mut graph = Graph::new(tx.identity().cluster);
    let mut previous = 0;
    for entry in tx.entries()? {
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
    let mut graph = read_graph(tx)?;
    let mut known: BTreeMap<_, _> = tx
        .entries()?
        .into_iter()
        .map(|entry| (entry.record.update().update_id, entry.record))
        .collect();
    let mut inserted = 0;
    for record in records {
        let update = record.update();
        if let Some(existing) = known.get(&update.update_id) {
            if existing.bytes() != record.bytes() {
                return Err(Error::RecordRejected {
                    update_id: update.update_id,
                    reason: harmion_domain::Error::IdMismatch,
                });
            }
            continue;
        }
        graph
            .accept(update.clone())
            .map_err(|reason| Error::RecordRejected {
                update_id: update.update_id,
                reason,
            })?;
        tx.append(record)?;
        known.insert(update.update_id, record.clone());
        inserted += 1;
    }
    Ok(inserted)
}
