//! Best-effort recovery hints, not assertions that a peer or the Internet is reachable.
//!
//! Linux suspend is inferred from kernel clocks. Network changes come from netwatch's
//! OS monitor plus periodic interface-state refreshes. Runtime gaps are deliberately
//! distinguished from suspend: process suspension or executor starvation can cause them.
use std::{collections::BTreeSet, time::Duration};

use iroh::{Endpoint, Watcher as _};
use tokio::{sync::watch, task::JoinSet, time::MissedTickBehavior};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Reason {
    Resumed,
    RuntimeResumed,
    NetworkChanged,
    RelayConnected,
}

/// Runs until the receiver disappears or this future is cancelled.
///
/// The watch channel intentionally coalesces bursts; the scheduler must throttle
/// retries and retain its normal periodic fallback. Dropping this future aborts
/// all owned tasks, including netwatch's monitor. No network probes are added.
pub async fn monitor(sender: watch::Sender<Option<Reason>>, endpoint: Option<Endpoint>) {
    let mut tasks = JoinSet::new();
    tasks.spawn(resume_hints(sender.clone()));
    tasks.spawn(network_hints(sender.clone()));
    if let Some(endpoint) = endpoint {
        tasks.spawn(relay_hints(sender.clone(), endpoint));
    }
    loop {
        tokio::select! {
            _ = sender.closed() => break,
            result = tasks.join_next(), if !tasks.is_empty() => {
                if let Some(Err(_)) = result {
                    tracing::warn!("recovery hint task stopped; periodic sync remains active");
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
struct Sample {
    observed: Duration,
    suspended: Option<Duration>,
}

#[derive(Default)]
struct ResumeDetector {
    previous: Option<Sample>,
}

impl ResumeDetector {
    fn observe(&mut self, sample: Sample) -> Option<Reason> {
        let previous = self.previous.replace(sample)?;
        let suspend_delta = sample
            .suspended
            .zip(previous.suspended)
            .and_then(|(now, before)| now.checked_sub(before));
        if suspend_delta.is_some_and(|delta| delta >= Duration::from_secs(1)) {
            Some(Reason::Resumed)
        } else if sample
            .observed
            .checked_sub(previous.observed)
            .is_some_and(|gap| gap >= Duration::from_secs(5))
        {
            Some(Reason::RuntimeResumed)
        } else {
            None
        }
    }
}

// Read boottime between two monotonic samples. Discard a slow sample so that
// scheduling delay between clock reads cannot masquerade as a suspend interval.
#[cfg(target_os = "linux")]
fn suspended_time() -> Option<Duration> {
    use rustix::time::{ClockId, Timespec, clock_gettime};
    fn duration(time: Timespec) -> Option<Duration> {
        Some(Duration::new(
            time.tv_sec.try_into().ok()?,
            time.tv_nsec.try_into().ok()?,
        ))
    }
    let before = duration(clock_gettime(ClockId::Monotonic))?;
    let boot = duration(clock_gettime(ClockId::Boottime))?;
    let after = duration(clock_gettime(ClockId::Monotonic))?;
    suspend_offset(before, boot, after)
}

#[cfg(any(target_os = "linux", test))]
fn suspend_offset(before: Duration, boot: Duration, after: Duration) -> Option<Duration> {
    if after.checked_sub(before)? > Duration::from_millis(100) {
        return None;
    }
    // The earlier monotonic read keeps the zero-suspend baseline nonnegative.
    // Using `after` could discard every baseline on a freshly booted machine,
    // then miss its very first suspend. Bracketing bounds the sampling skew.
    boot.checked_sub(before)
}

#[cfg(not(target_os = "linux"))]
fn suspended_time() -> Option<Duration> {
    None
}

async fn resume_hints(sender: watch::Sender<Option<Reason>>) {
    let start = std::time::Instant::now();
    let mut detector = ResumeDetector::default();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        if let Some(reason) = detector.observe(Sample {
            observed: start.elapsed(),
            suspended: suspended_time(),
        }) {
            sender.send_replace(Some(reason));
        }
    }
}

async fn network_hints(sender: watch::Sender<Option<Reason>>) {
    loop {
        let monitor = match netwatch::netmon::Monitor::new().await {
            Ok(monitor) => monitor,
            Err(_) => {
                tracing::warn!("network recovery monitor unavailable; retrying in 30 seconds");
                tokio::time::sleep(Duration::from_secs(30)).await;
                continue;
            }
        };
        let mut state = monitor.interface_state();
        // Treat the initial snapshot as a baseline, not a restoration event.
        let mut previous = state.get();
        let mut refresh = tokio::time::interval(Duration::from_secs(5));
        refresh.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                update = state.updated() => {
                    let Ok(current) = update else { break; };
                    if current != previous {
                        previous = current;
                        sender.send_replace(Some(Reason::NetworkChanged));
                    }
                }
                _ = refresh.tick() => {
                    // This only re-reads local state; it does not send network traffic.
                    // It covers link-only changes missing from Linux netlink subscriptions.
                    if monitor.network_change().await.is_err() {
                        break;
                    }
                }
            }
        }
        tracing::warn!("network recovery monitor stopped; retrying in 30 seconds");
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
}

fn newly_connected<T: Ord>(previous: &BTreeSet<T>, current: &BTreeSet<T>) -> bool {
    current.iter().any(|relay| !previous.contains(relay))
}

async fn relay_hints(sender: watch::Sender<Option<Reason>>, endpoint: Endpoint) {
    let mut status = endpoint.home_relay_status();
    let connected = |statuses: Vec<iroh::endpoint::RelayStatus>| {
        statuses
            .into_iter()
            .filter(|status| status.is_connected())
            .map(|status| status.url().clone())
            .collect::<BTreeSet<_>>()
    };
    let mut previous = connected(status.get());
    loop {
        let current = tokio::select! {
            _ = endpoint.closed() => break,
            result = status.updated() => {
                let Ok(statuses) = result else { break; };
                connected(statuses)
            }
        };
        if newly_connected(&previous, &current) {
            // No relay URLs or transport errors are logged.
            sender.send_replace(Some(Reason::RelayConnected));
        }
        previous = current;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(observed: u64, suspended: Option<u64>) -> Sample {
        Sample {
            observed: Duration::from_secs(observed),
            suspended: suspended.map(Duration::from_secs),
        }
    }

    #[test]
    fn first_suspend_after_boot_has_a_nonnegative_clock_baseline() {
        let before = Duration::from_secs(100);
        let skew = Duration::from_micros(10);
        let baseline = suspend_offset(before, before + skew, before + skew * 2).unwrap();
        let after_resume =
            suspend_offset(before, before + Duration::from_secs(60), before + skew).unwrap();
        let mut detector = ResumeDetector::default();
        detector.observe(Sample {
            observed: Duration::ZERO,
            suspended: Some(baseline),
        });
        assert_eq!(
            detector.observe(Sample {
                observed: Duration::from_secs(1),
                suspended: Some(after_resume)
            }),
            Some(Reason::Resumed)
        );
        assert!(suspend_offset(before, before, before + Duration::from_millis(101)).is_none());
        assert!(suspend_offset(before, before, before - skew).is_none());
    }

    #[test]
    fn suspend_delta_emits_once_and_takes_priority_over_runtime_gap() {
        let mut detector = ResumeDetector::default();
        assert_eq!(detector.observe(sample(0, Some(20))), None);
        assert_eq!(detector.observe(sample(1, Some(20))), None);
        assert_eq!(detector.observe(sample(2, Some(80))), Some(Reason::Resumed));
        assert_eq!(detector.observe(sample(3, Some(80))), None);
        assert_eq!(
            detector.observe(sample(10, Some(90))),
            Some(Reason::Resumed)
        );
    }

    #[test]
    fn process_pause_is_not_claimed_as_kernel_suspend() {
        let mut detector = ResumeDetector::default();
        detector.observe(sample(0, Some(20)));
        assert_eq!(
            detector.observe(sample(5, Some(20))),
            Some(Reason::RuntimeResumed)
        );
        assert_eq!(detector.observe(sample(6, Some(20))), None);
    }

    #[test]
    fn missing_or_reset_clock_samples_rebaseline_without_panicking() {
        let mut detector = ResumeDetector::default();
        detector.observe(sample(0, Some(20)));
        assert_eq!(detector.observe(sample(1, None)), None);
        assert_eq!(detector.observe(sample(2, Some(80))), None);
        assert_eq!(detector.observe(sample(3, Some(0))), None);
        assert_eq!(detector.observe(sample(2, Some(0))), None);
        assert_eq!(
            detector.observe(sample(7, None)),
            Some(Reason::RuntimeResumed)
        );
    }

    #[test]
    fn subthreshold_clock_changes_do_not_report_suspend() {
        let mut detector = ResumeDetector::default();
        detector.observe(sample(0, Some(20)));
        assert_eq!(
            detector.observe(Sample {
                observed: Duration::from_secs(1),
                suspended: Some(Duration::from_millis(20_999)),
            }),
            None
        );
    }

    #[test]
    fn only_newly_connected_relays_signal_recovery() {
        let empty = BTreeSet::new();
        let a = BTreeSet::from([1]);
        let b = BTreeSet::from([2]);
        assert!(!newly_connected(&empty, &empty));
        assert!(newly_connected(&empty, &a));
        assert!(!newly_connected(&a, &a));
        assert!(!newly_connected(&a, &empty));
        assert!(newly_connected(&a, &b));
        assert!(newly_connected(&a, &BTreeSet::from([1, 2])));
    }
}
