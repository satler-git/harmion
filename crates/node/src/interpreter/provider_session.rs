//! Runtime Provider connections. The host owns the actual WebSocket and maps a
//! generation to its sender; this table decides replacement and ownership.
use harmion_protocol::AppId;
use harmion_protocol::interpreter::ProviderId;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub provider: ProviderId,
    pub app: AppId,
    pub generation: u64,
    pub connected_at: i64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachOutcome {
    pub session: Session,
    pub replaced: Option<u64>,
}

#[derive(Debug, Default)]
pub struct SessionTable {
    next_generation: u64,
    sessions: BTreeMap<ProviderId, Session>,
}
impl SessionTable {
    pub fn new() -> Self {
        Self {
            next_generation: 1,
            sessions: BTreeMap::new(),
        }
    }
    /// One provider has at most one live connection. A newer connection replaces
    /// the old one and receives a distinct generation for stale-task detection.
    pub fn attach(&mut self, provider: ProviderId, app: AppId, now: i64) -> AttachOutcome {
        let generation = self.next_generation;
        self.next_generation = self.next_generation.wrapping_add(1).max(1);
        let replaced = self
            .sessions
            .get(&provider)
            .map(|session| session.generation);
        let session = Session {
            provider,
            app,
            generation,
            connected_at: now,
        };
        self.sessions.insert(provider, session.clone());
        AttachOutcome { session, replaced }
    }
    /// Remove only the current generation; a stale task cannot evict its
    /// replacement.
    pub fn detach(&mut self, provider: ProviderId, generation: u64) -> bool {
        if self
            .sessions
            .get(&provider)
            .is_some_and(|session| session.generation == generation)
        {
            self.sessions.remove(&provider);
            true
        } else {
            false
        }
    }
    pub fn get(&self, provider: ProviderId) -> Option<&Session> {
        self.sessions.get(&provider)
    }
    pub fn generation(&self, provider: ProviderId) -> Option<u64> {
        self.sessions
            .get(&provider)
            .map(|session| session.generation)
    }
    pub fn is_connected(&self, provider: ProviderId) -> bool {
        self.sessions.contains_key(&provider)
    }
    pub fn remove(&mut self, provider: ProviderId) -> Option<Session> {
        self.sessions.remove(&provider)
    }
    pub fn remove_app(&mut self, app: AppId) -> Vec<Session> {
        let removed: Vec<_> = self
            .sessions
            .values()
            .filter(|session| session.app == app)
            .cloned()
            .collect();
        for session in &removed {
            self.sessions.remove(&session.provider);
        }
        removed
    }
    pub fn len(&self) -> usize {
        self.sessions.len()
    }
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attach_replaces_only_the_same_provider_and_stale_detach_is_ignored() {
        let mut table = SessionTable::new();
        let provider = ProviderId([1; 32]);
        let first = table.attach(provider, AppId([2; 32]), 10);
        let second = table.attach(provider, AppId([2; 32]), 11);
        assert_eq!(first.replaced, None);
        assert_eq!(second.replaced, Some(first.session.generation));
        assert_eq!(table.len(), 1);
        assert!(!table.detach(provider, first.session.generation));
        assert_eq!(table.get(provider), Some(&second.session));
        assert!(table.detach(provider, second.session.generation));
        assert!(table.is_empty());
    }

    #[test]
    fn remove_app_returns_every_owned_session() {
        let mut table = SessionTable::new();
        let app = AppId([3; 32]);
        table.attach(ProviderId([1; 32]), app, 1);
        table.attach(ProviderId([2; 32]), app, 2);
        table.attach(ProviderId([4; 32]), AppId([5; 32]), 3);
        let mut removed = table.remove_app(app);
        removed.sort_by_key(|session| session.provider);
        assert_eq!(removed.len(), 2);
        assert_eq!(table.len(), 1);
        assert!(table.get(ProviderId([1; 32])).is_none());
    }
}
