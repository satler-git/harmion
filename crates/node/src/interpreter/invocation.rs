//! Caller-selected snapshot materialization and bounded invocation state.
use crate::Error;
use crate::access::{Credential, authenticate};
use crate::application::Host;
use crate::storage::{Store, read_content_graph};
use harmion_domain::Life;
use harmion_protocol::AppId;
use harmion_protocol::interpreter::{
    CONTRACT_VERSION, CallRequest, InvocationId, MAX_PROVIDER_MESSAGE_BYTES, ProviderContent,
    ProviderId, provider_content_json,
};
use serde_json::Value as Json;
use std::collections::BTreeMap;

pub struct PreparedCall {
    pub caller: AppId,
    pub provider: ProviderId,
    pub provider_name: String,
    pub contract_version: u32,
    pub input: Json,
    pub contents: Vec<ProviderContent>,
    pub timeout_ms: u64,
}

/// Resolve a caller's fixed Content snapshots and the approved Provider in one
/// storage transaction. No network or Provider work happens inside the callback.
pub fn prepare_call(
    store: &mut impl Store,
    host: &mut impl Host,
    credential: &Credential,
    request: &CallRequest,
) -> Result<PreparedCall, Error> {
    store.transaction(|tx| {
        let principal = authenticate(tx, credential, host.now_micros())?;
        let caller = principal.app.id;
        if !principal.app.grants.can_call() {
            return Err(Error::Forbidden);
        }
        let registration = match tx.approved_interpreter_by_name(&request.provider)? {
            Some(registration) => registration,
            None => {
                return if tx
                    .interpreter_registrations()?
                    .iter()
                    .any(|registration| registration.name == request.provider)
                {
                    Err(Error::InterpreterUnavailable)
                } else {
                    Err(Error::NotFound)
                };
            }
        };
        let provider_app = tx
            .app_by_id(registration.app)?
            .ok_or(Error::InterpreterUnavailable)?;
        if provider_app.revoked
            || host.now_micros() >= provider_app.expires_at
            || !provider_app.grants.allows_provider_name(&registration.name)
        {
            return Err(Error::InterpreterUnavailable);
        }
        let manifest = registration.decoded()?;
        if manifest.contract_version != CONTRACT_VERSION {
            return Err(Error::InterpreterUnavailable);
        }
        let mut contents = Vec::with_capacity(request.contents.len());
        for selection in &request.contents {
            let state = tx.token(selection.token)?.ok_or(Error::TokenExpired)?;
            if state.content_id != selection.content {
                return Err(Error::InvalidToken);
            }
            let snapshot = read_content_graph(tx, selection.content)?
                .snapshot_at(selection.content, &state.basis)
                .map_err(|_| Error::InvalidToken)?;
            let visible = snapshot.visible_content().ok_or(Error::NotFound)?;
            if !principal.app.grants.can_read(visible) {
                return Err(Error::NotFound);
            }
            contents.push(ProviderContent {
                content: provider_content_json(visible),
                conflict: snapshot.life == Life::Conflict,
            });
        }
        let encoded = serde_json::to_vec(&contents).map_err(|_| Error::InvalidRequest)?;
        // Leave room for the invocation envelope; the WS frame cap is authoritative.
        if encoded.len() > MAX_PROVIDER_MESSAGE_BYTES - 64 * 1024 {
            return Err(Error::InterpreterInputTooLarge);
        }
        Ok(PreparedCall {
            caller,
            provider: registration.provider,
            provider_name: registration.name,
            contract_version: registration.contract_version,
            input: request.input.clone(),
            contents,
            timeout_ms: request.timeout_ms,
        })
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveInvocation {
    pub id: InvocationId,
    pub app: AppId,
    pub provider: ProviderId,
    pub started_at: i64,
    pub deadline: i64,
}
impl ActiveInvocation {
    pub fn expired(&self, now: i64) -> bool {
        self.deadline <= now
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BeginError {
    NodeCapacity,
    AppCapacity,
    Duplicate,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CancelOutcome {
    Canceled(ActiveInvocation),
    NotFound,
    Forbidden,
}

#[derive(Debug)]
pub struct InvocationTable {
    max_node: usize,
    max_app: usize,
    active: BTreeMap<InvocationId, ActiveInvocation>,
    per_app: BTreeMap<AppId, usize>,
}
impl InvocationTable {
    pub fn new(max_node: usize, max_app: usize) -> Self {
        Self {
            max_node,
            max_app,
            active: BTreeMap::new(),
            per_app: BTreeMap::new(),
        }
    }
    pub fn default_limits() -> Self {
        Self::new(
            harmion_protocol::interpreter::MAX_ACTIVE_INVOCATIONS,
            harmion_protocol::interpreter::MAX_ACTIVE_INVOCATIONS_PER_APP,
        )
    }
    pub fn begin(&mut self, invocation: ActiveInvocation) -> Result<(), BeginError> {
        if self.active.contains_key(&invocation.id) {
            return Err(BeginError::Duplicate);
        }
        if self.active.len() >= self.max_node {
            return Err(BeginError::NodeCapacity);
        }
        let count = self.per_app.get(&invocation.app).copied().unwrap_or(0);
        if count >= self.max_app {
            return Err(BeginError::AppCapacity);
        }
        self.per_app.insert(invocation.app, count + 1);
        self.active.insert(invocation.id, invocation);
        Ok(())
    }
    /// Remove a completed invocation. Returns None when it was canceled or
    /// already finished; the caller must not deliver a second outcome.
    pub fn finish(&mut self, id: InvocationId) -> Option<ActiveInvocation> {
        let invocation = self.active.remove(&id)?;
        self.decrement(invocation.app);
        Some(invocation)
    }
    pub fn cancel(&mut self, id: InvocationId, app: AppId) -> CancelOutcome {
        let Some(invocation) = self.active.get(&id) else {
            return CancelOutcome::NotFound;
        };
        if invocation.app != app {
            return CancelOutcome::Forbidden;
        }
        CancelOutcome::Canceled(self.finish(id).expect("checked present"))
    }
    pub fn expire(&mut self, now: i64) -> Vec<ActiveInvocation> {
        let ids: Vec<_> = self
            .active
            .values()
            .filter(|invocation| invocation.expired(now))
            .map(|invocation| invocation.id)
            .collect();
        ids.into_iter().filter_map(|id| self.finish(id)).collect()
    }
    pub fn fail_provider(&mut self, provider: ProviderId) -> Vec<ActiveInvocation> {
        let ids: Vec<_> = self
            .active
            .values()
            .filter(|invocation| invocation.provider == provider)
            .map(|invocation| invocation.id)
            .collect();
        ids.into_iter().filter_map(|id| self.finish(id)).collect()
    }
    pub fn remove_app(&mut self, app: AppId) -> Vec<ActiveInvocation> {
        let ids: Vec<_> = self
            .active
            .values()
            .filter(|invocation| invocation.app == app)
            .map(|invocation| invocation.id)
            .collect();
        ids.into_iter().filter_map(|id| self.finish(id)).collect()
    }
    pub fn get(&self, id: InvocationId) -> Option<&ActiveInvocation> {
        self.active.get(&id)
    }
    pub fn len(&self) -> usize {
        self.active.len()
    }
    pub fn is_empty(&self) -> bool {
        self.active.is_empty()
    }
    fn decrement(&mut self, app: AppId) {
        if let Some(count) = self.per_app.get_mut(&app) {
            *count -= 1;
            if *count == 0 {
                self.per_app.remove(&app);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invocation(id: u8, app: u8, provider: u8, deadline: i64) -> ActiveInvocation {
        ActiveInvocation {
            id: InvocationId([id; 32]),
            app: AppId([app; 32]),
            provider: ProviderId([provider; 32]),
            started_at: 0,
            deadline,
        }
    }

    #[test]
    fn node_and_app_capacity_are_independent_and_released() {
        let mut table = InvocationTable::new(2, 1);
        table.begin(invocation(1, 1, 9, 100)).unwrap();
        assert_eq!(
            table.begin(invocation(2, 1, 9, 100)),
            Err(BeginError::AppCapacity)
        );
        table.begin(invocation(3, 2, 9, 100)).unwrap();
        assert_eq!(
            table.begin(invocation(4, 3, 9, 100)),
            Err(BeginError::NodeCapacity)
        );
        assert!(table.finish(InvocationId([1; 32])).is_some());
        table.begin(invocation(5, 1, 9, 100)).unwrap();
    }

    #[test]
    fn cancel_requires_the_owner_and_expiry_only_returns_due_entries() {
        let mut table = InvocationTable::new(8, 8);
        table.begin(invocation(1, 1, 9, 50)).unwrap();
        table.begin(invocation(2, 2, 9, 150)).unwrap();
        assert_eq!(
            table.cancel(InvocationId([1; 32]), AppId([2; 32])),
            CancelOutcome::Forbidden
        );
        assert_eq!(
            table.cancel(InvocationId([3; 32]), AppId([1; 32])),
            CancelOutcome::NotFound
        );
        let expired = table.expire(100);
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].id, InvocationId([1; 32]));
        assert_eq!(table.len(), 1);
        assert_eq!(
            table.cancel(InvocationId([2; 32]), AppId([2; 32])),
            CancelOutcome::Canceled(invocation(2, 2, 9, 150))
        );
        assert!(table.is_empty());
        assert!(table.per_app.is_empty());
    }

    #[test]
    fn provider_disconnect_and_app_revocation_remove_only_their_entries() {
        let mut table = InvocationTable::new(8, 8);
        table.begin(invocation(1, 1, 9, 100)).unwrap();
        table.begin(invocation(2, 1, 8, 100)).unwrap();
        table.begin(invocation(3, 2, 9, 100)).unwrap();
        let failed = table.fail_provider(ProviderId([9; 32]));
        assert_eq!(failed.len(), 2);
        assert_eq!(table.len(), 1);
        let removed = table.remove_app(AppId([1; 32]));
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].id, InvocationId([2; 32]));
        assert!(table.is_empty());
        assert!(table.per_app.is_empty());
    }
}
