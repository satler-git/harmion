//! Local Interpreter registry, Provider session state, and invocation brokering.
//!
//! Network I/O lives in the host. These modules own validation, durable
//! registration, snapshot materialization, and the bounded in-memory state
//! machine for one node.
pub mod invocation;
pub mod provider_session;
pub mod registry;

pub use invocation::{InvocationTable, PreparedCall};
pub use provider_session::SessionTable;

use crate::Error;
use harmion_protocol::AppId;
use harmion_protocol::interpreter::{
    ProviderDescription, ProviderId, ProviderRegistration, ProviderStatus, ProviderSummary,
};

/// Durable registration state. `manifest` is canonical registration JSON; the
/// name and contract version are duplicated for bounded lookups.
#[derive(Clone, Debug, PartialEq)]
pub struct Registration {
    pub provider: ProviderId,
    pub app: AppId,
    pub name: String,
    pub contract_version: u32,
    pub manifest: Vec<u8>,
    pub status: ProviderStatus,
    pub created_at: i64,
    pub approved_at: Option<i64>,
}
impl Registration {
    pub fn decoded(&self) -> Result<ProviderRegistration, Error> {
        let registration = harmion_protocol::interpreter::decode_registration(&self.manifest)
            .map_err(|_| Error::CorruptLog)?;
        if registration.name != self.name || registration.contract_version != self.contract_version
        {
            return Err(Error::CorruptLog);
        }
        Ok(registration)
    }
    pub fn summary(&self, connected: bool) -> Result<ProviderSummary, Error> {
        self.decoded()?;
        Ok(ProviderSummary {
            provider_id: self.provider,
            name: self.name.clone(),
            contract_version: self.contract_version,
            status: self.status.as_str(),
            connected,
        })
    }
    pub fn describe(&self, connected: bool) -> Result<ProviderDescription, Error> {
        let registration = self.decoded()?;
        Ok(ProviderDescription {
            provider_id: self.provider,
            name: self.name.clone(),
            contract_version: self.contract_version,
            status: self.status.as_str(),
            connected,
            input_schema: registration.input_schema,
            output_schema: registration.output_schema,
            default_query: registration.default_query,
        })
    }
}

pub fn manifest_bytes(registration: &ProviderRegistration) -> Result<Vec<u8>, Error> {
    harmion_protocol::interpreter::validate_registration(registration)?;
    serde_json::to_vec(registration).map_err(|_| Error::InvalidRequest)
}
