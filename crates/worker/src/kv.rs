//! Durable host boundary for a SQLite-backed Durable Object.
//!
//! The host supplies exactly one synchronous transaction boundary (Workers
//! `storage.transactionSync` semantics: throwing rolls back, returning normally
//! commits) and a physical durability gate (`storage.sync`). This crate never
//! assumes that a logical commit alone is durable, and it does not open network
//! connections, read secrets from the environment, or decide membership.
use std::collections::BTreeMap;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct HostError(pub String);

pub trait KvHost {
    /// Runs one synchronous transaction. Returning `Err` must roll back every
    /// write made through the callback; returning `Ok` commits them.
    fn transaction<T>(
        &mut self,
        f: impl FnOnce(&mut dyn KvTransaction) -> Result<T, HostError>,
    ) -> Result<T, HostError>;
    /// Physical durability gate. Must be awaited by the host after a committed
    /// transaction and before any ACK/response leaves the isolate.
    fn sync(&mut self) -> Result<(), HostError>;
}

pub type Row = (Vec<u8>, Vec<u8>);

pub trait KvTransaction {
    fn get(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>, HostError>;
    fn put(&mut self, key: &[u8], value: Vec<u8>) -> Result<(), HostError>;
    fn delete(&mut self, key: &[u8]) -> Result<(), HostError>;
    /// Ordered (key, value) pairs sharing the prefix, bounded by `limit`.
    fn range(&mut self, prefix: &[u8], limit: usize) -> Result<Vec<Row>, HostError>;
}

/// In-process host used by tests and by a local non-deployed replica.
/// Committed data survives dropping the `WorkerChallenges` adapter, which models
/// an ordinary activation, not a Cloudflare deployment.
#[derive(Default)]
pub struct MemoryKv {
    data: BTreeMap<Vec<u8>, Vec<u8>>,
    syncs: usize,
    committed_writes: usize,
}

impl MemoryKv {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn syncs(&self) -> usize {
        self.syncs
    }
    pub fn len(&self) -> usize {
        self.data.len()
    }
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

struct MemoryTransaction<'a> {
    data: &'a mut BTreeMap<Vec<u8>, Vec<u8>>,
    writes: usize,
}

impl KvTransaction for MemoryTransaction<'_> {
    fn get(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>, HostError> {
        Ok(self.data.get(key).cloned())
    }
    fn put(&mut self, key: &[u8], value: Vec<u8>) -> Result<(), HostError> {
        self.data.insert(key.to_vec(), value);
        self.writes += 1;
        Ok(())
    }
    fn delete(&mut self, key: &[u8]) -> Result<(), HostError> {
        if self.data.remove(key).is_some() {
            self.writes += 1;
        }
        Ok(())
    }
    fn range(&mut self, prefix: &[u8], limit: usize) -> Result<Vec<Row>, HostError> {
        Ok(self
            .data
            .range(prefix.to_vec()..)
            .take_while(|(key, _)| key.starts_with(prefix))
            .take(limit)
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect())
    }
}

impl KvHost for MemoryKv {
    fn transaction<T>(
        &mut self,
        f: impl FnOnce(&mut dyn KvTransaction) -> Result<T, HostError>,
    ) -> Result<T, HostError> {
        let mut staged = self.data.clone();
        let (value, writes) = {
            let mut transaction = MemoryTransaction {
                data: &mut staged,
                writes: 0,
            };
            let value = f(&mut transaction)?;
            (value, transaction.writes)
        };
        self.data = staged;
        self.committed_writes += writes;
        Ok(value)
    }
    fn sync(&mut self) -> Result<(), HostError> {
        self.syncs += 1;
        Ok(())
    }
}
