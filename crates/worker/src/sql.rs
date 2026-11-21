//! Durable SQL host boundary for a SQLite-backed Durable Object.
//!
//! The host supplies one synchronous transaction (Workers `transactionSync`
//! semantics: throwing rolls back, returning normally commits) and a physical
//! durability gate (`storage.sync`). Positions are passed as 8-byte big-endian
//! BLOBs, never as JS Numbers, so 64-bit values survive exactly. Small bounded
//! integers use `Int`; callers must not send values outside the exact JS range
//! through `Int`.
use crate::kv::HostError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SqlValue {
    Null,
    Int(i64),
    Blob(Vec<u8>),
    Text(String),
}

pub trait SqlHost {
    /// Runs one synchronous transaction. `Err` must roll back every statement
    /// issued inside the callback; `Ok` commits them.
    fn transaction<T>(
        &mut self,
        f: impl FnOnce(&mut dyn SqlTransaction) -> Result<T, HostError>,
    ) -> Result<T, HostError>;
    /// Physical durability gate, awaited by the host before releasing an ACK.
    fn sync(&mut self) -> Result<(), HostError>;
}

pub trait SqlTransaction {
    fn execute(&mut self, sql: &str, params: &[SqlValue]) -> Result<usize, HostError>;
    fn query(&mut self, sql: &str, params: &[SqlValue]) -> Result<Vec<Vec<SqlValue>>, HostError>;
}

pub fn blob_u64(value: u64) -> SqlValue {
    SqlValue::Blob(value.to_be_bytes().to_vec())
}
