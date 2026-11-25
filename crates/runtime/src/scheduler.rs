//! Sync scheduling shared by the daemon and Android.
//!
//! The policy itself is host-specific: Android asks ConnectivityManager and the
//! battery APIs, the daemon always allows. This module only asks a [`SyncGate`]
//! before starting a round, coalesces triggers and keeps the existing per-peer
//! backoff. Blocking never breaks local reads or writes; the durable cursors are
//! the resume point once the gate permits again.
use crate::status::SyncStatusView;
use harmion_domain::NodeId;
use harmion_net_native::{SharedStore, SystemHost, sync};
use harmion_node::{application::Host, storage::Store};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::time::Instant;

/// Why sync was requested. The trigger is diagnostic only; it never bypasses
/// the gate and never expands what is synchronized.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncTrigger {
    Periodic,
    LocalSave,
    AppConnect,
    NetworkReturn,
    Manual,
}

impl SyncTrigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Periodic => "periodic",
            Self::LocalSave => "local_save",
            Self::AppConnect => "app_connect",
            Self::NetworkReturn => "network_return",
            Self::Manual => "manual",
        }
    }
}

/// Why the runtime is not synchronizing. Values are stable so the UI can map
/// them to strings without parsing free-form messages.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockReason {
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl BlockReason {
    pub fn new(code: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: None,
        }
    }
    pub fn with_message(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: Some(message.into()),
        }
    }
}

/// Host policy check. Implementations must be cheap and must not block.
pub trait SyncGate: Send + Sync + 'static {
    fn permit(&self) -> Result<(), BlockReason>;
}

/// Gate used by the daemon and by embedded hosts that have no policy of their own.
#[derive(Debug, Default)]
pub struct OpenGate;

impl SyncGate for OpenGate {
    fn permit(&self) -> Result<(), BlockReason> {
        Ok(())
    }
}

/// Gate driven by the host (Android) through [`crate::NodeRuntime::set_sync_policy`].
#[derive(Debug)]
pub struct PolicyGate {
    permitted: std::sync::atomic::AtomicBool,
    reason: Mutex<Option<BlockReason>>,
    changed: tokio::sync::Notify,
}

impl Default for PolicyGate {
    fn default() -> Self {
        Self {
            permitted: std::sync::atomic::AtomicBool::new(true),
            reason: Mutex::new(None),
            changed: tokio::sync::Notify::new(),
        }
    }
}

impl PolicyGate {
    pub fn new() -> Self {
        Self::default()
    }

    /// Update the policy and wake the scheduler. `permitted = false` aborts
    /// in-flight rounds; every sync round is transactional and resumable.
    pub fn set(&self, permitted: bool, reason: Option<BlockReason>) {
        *self.reason.lock().expect("policy reason") = if permitted { None } else { reason };
        self.permitted
            .store(permitted, std::sync::atomic::Ordering::SeqCst);
        self.changed.notify_waiters();
    }

    pub fn is_permitted(&self) -> bool {
        self.permitted.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn blocked_reason(&self) -> Option<BlockReason> {
        self.reason.lock().expect("policy reason").clone()
    }

    pub async fn changed(&self) {
        self.changed.notified().await;
    }
}

impl SyncGate for PolicyGate {
    fn permit(&self) -> Result<(), BlockReason> {
        if self.is_permitted() {
            Ok(())
        } else {
            Err(self
                .blocked_reason()
                .unwrap_or_else(|| BlockReason::new("policy")))
        }
    }
}

/// Trigger mailbox with a coalescing wake-up. Multiple triggers arriving while
/// a round is running are merged into one follow-up pass.
#[derive(Debug, Default)]
pub struct SyncControl {
    pending: Mutex<BTreeSet<SyncTrigger>>,
    wake: tokio::sync::Notify,
}

impl SyncControl {
    pub fn request(&self, trigger: SyncTrigger) {
        self.pending.lock().expect("sync triggers").insert(trigger);
        self.wake.notify_waiters();
    }

    pub async fn wait(&self) {
        self.wake.notified().await;
    }

    fn take(&self) -> BTreeSet<SyncTrigger> {
        std::mem::take(&mut *self.pending.lock().expect("sync triggers"))
    }

    #[cfg(test)]
    fn pending(&self) -> bool {
        !self.pending.lock().expect("sync triggers").is_empty()
    }
}

/// Snapshot of the scheduler for status APIs. Times are microseconds since the
/// Unix epoch and match the node's persisted `sync-status` clock.
#[derive(Clone, Debug, Default, Serialize)]
pub struct SchedulerState {
    pub enabled: bool,
    pub permitted: bool,
    pub blocked: Option<BlockReason>,
    pub inflight: Vec<String>,
    pub retrying: BTreeMap<String, u32>,
    pub last_trigger: Option<String>,
    pub last_round_at: Option<i64>,
    pub last_success_at: Option<i64>,
    pub last_error: Option<String>,
    pub rounds_completed: u64,
    pub rounds_failed: u64,
}

impl SchedulerState {
    pub fn view(&self) -> SyncStatusView {
        SyncStatusView {
            enabled: self.enabled,
            permitted: self.permitted,
            blocked: self.blocked.clone(),
            inflight: self.inflight.clone(),
            retrying: self.retrying.clone(),
            last_trigger: self.last_trigger.clone(),
            last_round_at: self.last_round_at,
            last_success_at: self.last_success_at,
            last_error: self.last_error.clone(),
            rounds_completed: self.rounds_completed,
            rounds_failed: self.rounds_failed,
        }
    }
}

#[derive(Default)]
struct SyncSchedule {
    retries: BTreeMap<NodeId, (u32, Instant)>,
    inflight: BTreeSet<NodeId>,
    pending_saves: BTreeSet<NodeId>,
    pending_recovery: BTreeSet<NodeId>,
    recovery_due: Option<Instant>,
    last_recovery: Option<Instant>,
    ready: VecDeque<NodeId>,
}

impl SyncSchedule {
    /// A hint is not proof of connectivity. Coalesce it, allow interfaces two
    /// seconds to settle, and admit at most one backoff-bypassing wave per 30s.
    /// Preserve failure counts so a failed recovery attempt resumes backoff.
    fn request_recovery(&mut self, now: Instant) {
        let due = self
            .last_recovery
            .map_or(now + Duration::from_secs(2), |last| {
                (last + Duration::from_secs(30)).max(now + Duration::from_secs(2))
            });
        self.recovery_due.get_or_insert(due);
    }

    fn start_ready(&mut self, peers: BTreeSet<NodeId>, saved: bool, now: Instant) -> Vec<NodeId> {
        const MAX_ROUNDS: usize = 4;
        self.retries.retain(|node, _| peers.contains(node));
        self.pending_saves.retain(|node| peers.contains(node));
        self.pending_recovery.retain(|node| peers.contains(node));
        self.ready.retain(|node| peers.contains(node));
        if self.recovery_due.is_some_and(|due| due <= now) {
            self.pending_recovery.extend(&peers);
            self.last_recovery = Some(now);
            self.recovery_due = None;
        }
        if saved {
            self.pending_saves.extend(&peers);
        }

        // A new save can make a peer ready again, behind peers already waiting.
        for node in peers {
            if self.inflight.contains(&node)
                || self.ready.contains(&node)
                || self.retries.get(&node).is_some_and(|(failures, next)| {
                    *next > now
                        && !self.pending_recovery.contains(&node)
                        && (*failures != 0 || !self.pending_saves.contains(&node))
                })
            {
                continue;
            }
            self.ready.push_back(node);
        }
        let count = (MAX_ROUNDS - self.inflight.len()).min(self.ready.len());
        let starting: Vec<_> = self.ready.drain(..count).collect();
        for node in &starting {
            self.inflight.insert(*node);
            // A later save can schedule a follow-up to this fixed-high round.
            self.pending_saves.remove(node);
            self.pending_recovery.remove(node);
        }
        starting
    }

    fn complete(&mut self, node: NodeId, failures: u32, next: Instant) {
        self.inflight.remove(&node);
        self.retries.insert(node, (failures, next));
    }
}

/// Sync scheduler inputs. Grouped so the loop signature stays readable and so a
/// host can construct the scheduler without a positional argument list.
pub struct Scheduler {
    pub store: SharedStore<harmion_store_sqlite::SqliteStore>,
    pub transport: Arc<harmion_net_native::NativeTransport>,
    pub seconds: u64,
    pub gate: Arc<dyn SyncGate>,
    pub control: Arc<SyncControl>,
    pub state: Arc<Mutex<SchedulerState>>,
    pub recovery: tokio::sync::watch::Receiver<Option<harmion_net_native::recovery::Reason>>,
    pub stop: tokio::sync::watch::Receiver<bool>,
}

/// Runs until `stop` flips or the recovery channel closes.
impl Scheduler {
    pub async fn run(mut self) {
        let Scheduler {
            store,
            transport,
            seconds,
            gate,
            control,
            state,
            recovery,
            stop,
        } = &mut self;
        let store = store.clone();
        let transport = transport.clone();
        let seconds = *seconds;
        let gate = gate.clone();
        let control = control.clone();
        let state = state.clone();
        let base = Duration::from_secs(seconds);
        let mut schedule = SyncSchedule::default();
        let mut local_save = false;
        let mut failures = 0u32;
        let mut rounds = tokio::task::JoinSet::new();
        {
            let mut state = state.lock().expect("scheduler state");
            state.enabled = true;
        }
        loop {
            if *stop.borrow() {
                rounds.abort_all();
                return;
            }
            // Host policy is checked before every round and on every wake.
            match gate.permit() {
                Ok(()) => {
                    let mut state = state.lock().expect("scheduler state");
                    if !state.permitted || state.blocked.is_some() {
                        state.permitted = true;
                        state.blocked = None;
                    }
                }
                Err(reason) => {
                    let blocked_now = {
                        let mut state = state.lock().expect("scheduler state");
                        let blocked_now =
                            state.permitted || state.blocked.as_ref() != Some(&reason);
                        state.permitted = false;
                        state.blocked = Some(reason);
                        blocked_now
                    };
                    if blocked_now {
                        // A policy change aborts in-flight rounds. Page transfers are
                        // transactional and the durable cursor resumes afterwards.
                        if !rounds.is_empty() {
                            tracing::info!("sync policy stopped; aborting in-flight rounds");
                            rounds.abort_all();
                        }
                        while rounds.join_next().await.is_some() {}
                        let mut state = state.lock().expect("scheduler state");
                        state.inflight.clear();
                        if let Some(trigger) = control.take().last() {
                            state.last_trigger = Some(trigger.as_str().to_owned());
                        }
                    }
                    tokio::select! {
                        _ = stop.changed() => {},
                        _ = tokio::time::sleep(base.min(Duration::from_secs(1))) => {},
                    }
                    continue;
                }
            }

            let peers = match store
                .run(|s| {
                    s.transaction(|tx| Ok(tx.peers()?.into_iter().map(|peer| peer.node).collect()))
                })
                .await
            {
                Ok(peers) => {
                    failures = 0;
                    peers
                }
                Err(error) => {
                    // A transient SQLite error must not take down the HTTP services.
                    failures = failures.saturating_add(1);
                    let delay =
                        Duration::from_secs(1u64 << failures.min(6)).min(Duration::from_secs(60));
                    tracing::warn!(%error, retry_seconds = delay.as_secs(), "peer list unavailable; retrying automatic sync");
                    tokio::time::sleep(delay).await;
                    continue;
                }
            };
            let triggers = control.take();
            let trigger = triggers.last().copied();
            if trigger.is_some_and(|trigger| trigger != SyncTrigger::Periodic) {
                schedule.request_recovery(Instant::now());
            }
            if triggers.contains(&SyncTrigger::LocalSave) {
                local_save = true;
            }
            let started =
                schedule.start_ready(peers, std::mem::take(&mut local_save), Instant::now());
            if !started.is_empty() {
                let now = SystemHost.now_micros();
                let mut state = state.lock().expect("scheduler state");
                state.last_trigger = trigger.map(|trigger| trigger.as_str().to_owned());
                state.last_round_at = Some(now);
                state.inflight = started
                    .iter()
                    .map(|node| harmion_protocol::hex(&node.0))
                    .collect();
            }
            for node in started {
                let store = store.clone();
                let transport = transport.clone();
                rounds.spawn(async move {
                    let result = sync::sync_once(&store, transport.as_ref(), node).await;
                    (node, result)
                });
            }
            tokio::select! {
                completed = rounds.join_next(), if !rounds.is_empty() => {
                    match completed {
                        Some(Ok((node, result))) => {
                            let previous = schedule.retries.get(&node).map_or(0, |(failures, _)| *failures);
                            let failed = match result {
                                Ok(report) => {
                                    if previous != 0 {
                                        tracing::info!(peer = %harmion_protocol::hex(&node.0), "peer reconnected");
                                    }
                                    let mut state = state.lock().expect("scheduler state");
                                    state.rounds_completed = state.rounds_completed.saturating_add(1);
                                    state.last_success_at = Some(SystemHost.now_micros());
                                    state.last_error = None;
                                    let _ = report;
                                    0
                                }
                                Err(harmion_net_native::Error::Node(
                                    harmion_node::Error::PeerRetired | harmion_node::Error::MembershipConflict,
                                )) => 0,
                                Err(error) => {
                                    if previous == 0 {
                                        tracing::warn!(peer = %harmion_protocol::hex(&node.0), %error, "peer unavailable; retrying");
                                    }
                                    let code = failure_code(&error);
                                    let mut state = state.lock().expect("scheduler state");
                                    state.rounds_failed = state.rounds_failed.saturating_add(1);
                                    state.last_error = Some(format!("{}: {code}", harmion_protocol::hex(&node.0)));
                                    previous.saturating_add(1)
                                }
                            };
                            let delay = base.saturating_mul(1u32 << failed.min(5)).min(Duration::from_secs(3600));
                            let jitter = u16::from_be_bytes(
                                SystemHost.random_bytes().map(|bytes| [bytes[0], bytes[1]]).unwrap_or([0, 0]),
                            ) as u64 % 1000;
                            tracing::debug!(peer = %harmion_protocol::hex(&node.0), failures = failed, delay_ms = delay.as_millis() + u128::from(jitter), "next sync scheduled");
                            schedule.complete(node, failed, Instant::now() + delay + Duration::from_millis(jitter));
                            {
                                let mut state = state.lock().expect("scheduler state");
                                state.inflight = schedule
                                    .inflight
                                    .iter()
                                    .map(|node| harmion_protocol::hex(&node.0))
                                    .collect();
                                state.retrying = schedule
                                    .retries
                                    .iter()
                                    .filter(|(_, (failures, _))| *failures > 0)
                                    .map(|(node, (failures, _))| (harmion_protocol::hex(&node.0), *failures))
                                    .collect();
                            }
                        }
                        Some(Err(error)) => {
                            tracing::warn!(%error, "sync task ended unexpectedly");
                            let mut state = state.lock().expect("scheduler state");
                            state.rounds_failed = state.rounds_failed.saturating_add(1);
                            state.last_error = Some("sync task failed".into());
                            schedule.inflight.clear();
                        }
                        None => {}
                    }
                }
                _ = store.local_save_notified() => {
                    local_save = true;
                    control.request(SyncTrigger::LocalSave);
                },
                changed = recovery.changed() => {
                    if changed.is_err() {
                        tracing::warn!("recovery monitor stopped; periodic sync remains active");
                    } else {
                        let reason = *recovery.borrow_and_update();
                        tracing::debug!(?reason, "recovery hint; scheduling bounded sync retry");
                        schedule.request_recovery(Instant::now());
                    }
                },
                _ = control.wait() => {},
                _ = stop.changed() => {},
                _ = tokio::time::sleep(base.min(Duration::from_secs(1))) => {},
            }
        }
    }
}

/// Stable failure codes; never persist transport messages or payloads.
pub(crate) fn failure_code(error: &harmion_net_native::Error) -> &'static str {
    use harmion_net_native::Error as E;
    use harmion_node::Error as N;
    match error {
        E::Timeout => "timeout",
        E::Capacity => "capacity",
        E::Io(_) | E::Transport(_) => "transport_error",
        E::InvalidResponse => "invalid_response",
        E::RemoteRejected { .. } | E::Node(N::RecordRejected { .. }) => "record_rejected",
        E::Node(N::PeerRetired) => "peer_retired",
        E::Node(N::MembershipConflict) => "membership_conflict",
        E::Node(N::Unauthenticated) => "unauthenticated",
        E::Node(N::IdentityMismatch) => "identity_mismatch",
        E::Node(N::Storage(_) | N::CorruptLog) => "storage_error",
        _ => "sync_error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_gate_blocks_with_stable_reason() {
        let gate = PolicyGate::new();
        assert!(gate.permit().is_ok());
        gate.set(false, Some(BlockReason::new("wifi_only")));
        assert_eq!(gate.permit().unwrap_err().code, "wifi_only");
        gate.set(true, None);
        assert!(gate.permit().is_ok());
        assert!(gate.blocked_reason().is_none());
    }

    #[test]
    fn triggers_coalesce() {
        let control = SyncControl::default();
        assert!(!control.pending());
        control.request(SyncTrigger::AppConnect);
        control.request(SyncTrigger::NetworkReturn);
        control.request(SyncTrigger::AppConnect);
        let taken = control.take();
        assert_eq!(taken.len(), 2);
        assert!(taken.contains(&SyncTrigger::AppConnect));
        assert!(!control.pending());
    }

    #[test]
    fn schedule_backs_off_and_recovers() {
        let now = Instant::now();
        let peers: BTreeSet<NodeId> = [NodeId([1; 32]), NodeId([2; 32])].into_iter().collect();
        let mut schedule = SyncSchedule::default();
        assert_eq!(schedule.start_ready(peers.clone(), false, now).len(), 2);
        let retry = now + Duration::from_secs(30);
        schedule.complete(NodeId([1; 32]), 1, retry);
        schedule.complete(NodeId([2; 32]), 0, retry);
        // Neither the failed peer nor the successful one is due before `retry`.
        assert_eq!(schedule.start_ready(peers.clone(), false, now).len(), 0);
        // A recovery hint waits for interfaces to settle and then bypasses backoff.
        schedule.request_recovery(now);
        assert_eq!(schedule.start_ready(peers.clone(), false, now).len(), 0);
        assert_eq!(
            schedule
                .start_ready(peers, false, now + Duration::from_secs(2))
                .len(),
            2
        );
    }

    // Ported from the daemon's previous in-process scheduler tests: ordering and
    // backoff semantics must not change when the daemon moves onto this loop.
    #[test]
    fn waiting_peer_gets_next_slot_during_continuous_saves() {
        let [a, b, c, d, e] = [1, 2, 3, 4, 5].map(|id| NodeId([id; 32]));
        let peers = BTreeSet::from([a, b, c, d, e]);
        let now = Instant::now();
        let periodic_due = now + Duration::from_secs(120);
        let mut schedule = SyncSchedule::default();

        assert_eq!(
            schedule.start_ready(peers.clone(), false, now),
            [a, b, c, d]
        );
        assert!(schedule.start_ready(peers.clone(), true, now).is_empty());
        schedule.complete(a, 0, periodic_due);
        assert_eq!(schedule.start_ready(peers.clone(), true, now), [e]);

        schedule.complete(b, 0, periodic_due);
        assert_eq!(schedule.start_ready(peers, true, now), [a]);
    }

    #[test]
    fn recovery_bypasses_backoff_once_after_settling_and_preserves_failures() {
        let node = NodeId([1; 32]);
        let peers = BTreeSet::from([node]);
        let now = Instant::now();
        let mut schedule = SyncSchedule::default();
        assert_eq!(schedule.start_ready(peers.clone(), false, now), [node]);
        schedule.complete(node, 4, now + Duration::from_secs(3600));
        schedule.request_recovery(now);
        assert!(
            schedule
                .start_ready(peers.clone(), false, now + Duration::from_secs(1))
                .is_empty()
        );
        assert_eq!(
            schedule.start_ready(peers.clone(), false, now + Duration::from_secs(2)),
            [node]
        );
        assert_eq!(schedule.retries[&node].0, 4);
        schedule.complete(node, 5, now + Duration::from_secs(3600));
        assert!(
            schedule
                .start_ready(peers, true, now + Duration::from_secs(3))
                .is_empty()
        );
    }

    #[test]
    fn recovery_during_an_inflight_round_is_retained_for_one_followup() {
        let node = NodeId([1; 32]);
        let peers = BTreeSet::from([node]);
        let now = Instant::now();
        let mut schedule = SyncSchedule::default();
        assert_eq!(schedule.start_ready(peers.clone(), false, now), [node]);
        schedule.request_recovery(now);
        assert!(
            schedule
                .start_ready(peers.clone(), false, now + Duration::from_secs(2))
                .is_empty()
        );
        schedule.complete(node, 3, now + Duration::from_secs(3600));
        assert_eq!(
            schedule.start_ready(peers.clone(), false, now + Duration::from_secs(3)),
            [node]
        );
        schedule.complete(node, 4, now + Duration::from_secs(3600));
        assert!(
            schedule
                .start_ready(peers, false, now + Duration::from_secs(4))
                .is_empty()
        );
    }

    #[test]
    fn recovery_bursts_are_coalesced_but_a_later_reconnect_is_not_lost() {
        let node = NodeId([1; 32]);
        let peers = BTreeSet::from([node]);
        let now = Instant::now();
        let mut schedule = SyncSchedule::default();
        schedule.start_ready(peers.clone(), false, now);
        schedule.complete(node, 1, now + Duration::from_secs(3600));
        schedule.request_recovery(now);
        schedule.request_recovery(now + Duration::from_secs(1));
        assert_eq!(
            schedule.start_ready(peers.clone(), false, now + Duration::from_secs(2)),
            [node]
        );
        schedule.complete(node, 2, now + Duration::from_secs(3600));
        schedule.request_recovery(now + Duration::from_secs(3));
        schedule.request_recovery(now + Duration::from_secs(20));
        assert!(
            schedule
                .start_ready(peers.clone(), false, now + Duration::from_secs(31))
                .is_empty()
        );
        assert_eq!(
            schedule.start_ready(peers, false, now + Duration::from_secs(32)),
            [node]
        );
    }

    #[test]
    fn recovery_preserves_four_slot_limit_and_existing_wait_order() {
        let [a, b, c, d, e] = [1, 2, 3, 4, 5].map(|id| NodeId([id; 32]));
        let peers = BTreeSet::from([a, b, c, d, e]);
        let now = Instant::now();
        let mut schedule = SyncSchedule::default();
        assert_eq!(
            schedule.start_ready(peers.clone(), false, now),
            [a, b, c, d]
        );
        schedule.request_recovery(now);
        assert!(
            schedule
                .start_ready(peers.clone(), false, now + Duration::from_secs(2))
                .is_empty()
        );
        schedule.complete(a, 3, now + Duration::from_secs(3600));
        assert_eq!(
            schedule.start_ready(peers.clone(), false, now + Duration::from_secs(3)),
            [e]
        );
        schedule.complete(b, 2, now + Duration::from_secs(3600));
        assert_eq!(
            schedule.start_ready(peers, false, now + Duration::from_secs(4)),
            [a]
        );
    }

    #[test]
    fn pending_save_does_not_bypass_failure_backoff() {
        let node = NodeId([1; 32]);
        let peers = BTreeSet::from([node]);
        let now = Instant::now();
        let retry_due = now + Duration::from_secs(60);
        let mut schedule = SyncSchedule::default();

        assert_eq!(schedule.start_ready(peers.clone(), false, now), [node]);
        schedule.complete(node, 1, retry_due);
        assert!(schedule.start_ready(peers.clone(), true, now).is_empty());
        assert_eq!(schedule.start_ready(peers, false, retry_due), [node]);
    }
}
