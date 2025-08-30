// Integration tests for signal module
// This file was created by CI; it will be populated below.

// ---------- Added tests (focus: SignalInfo, PeerSignalStateChange handling, route/heartbeat) ----------
// NOTE: Tests use Rust built-in test framework with tokio::test for async and warp::test for filter testing.
#[cfg(test)]
mod added_signal_diff_tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration as StdDuration;
    use tokio::sync::{mpsc, RwLock};
    use std::collections::{HashMap, HashSet};
    use warp::Filter;

    // Helper to build a default SignalInfo for tests
    fn sig(host: &str, port: u16, id: PeerIndex) -> SignalInfo {
        SignalInfo { addr: (host.to_string(), port), id }
    }

    // --- SignalInfo.to_http / to_ws ---
    #[tokio::test]
    async fn signal_info_to_http_ws_should_format_correctly() {
        let s = sig("127.0.0.1", 8080, 1);
        assert_eq!(s.to_http(), "http://127.0.0.1:8080");
        assert_eq!(s.to_ws(), "ws://127.0.0.1:8080");

        let s2 = sig("example.com", 65535, 42);
        assert_eq!(s2.to_http(), "http://example.com:65535");
        assert_eq!(s2.to_ws(), "ws://example.com:65535");
    }

    // --- handle_peer_signal_state_changes: Connect ---
    #[tokio::test]
    async fn connect_adds_peer_and_signal_mappings() {
        let sig_peers = Arc::new(RwLock::new((HashMap::<SignalInfo, HashSet<PeerIndex>>::new(), HashMap::<PeerIndex, SignalInfo>::new())));
        let s = sig("host", 1000, 99);

        handle_peer_signal_state_changes(PeerSignalStateChange::Connect(7, s.clone()), &sig_peers).await;

        let lock = sig_peers.read().await;
        let (sigs, peers) = &*lock;

        // peers map contains the peer -> signal mapping
        assert_eq!(peers.get(&7), Some(&s));
        // sigs map contains signal -> {peer} mapping
        let set = sigs.get(&s).expect("signal entry");
        assert!(set.contains(&7));
    }

    // --- handle_peer_signal_state_changes: Connect switching signals removes from old and cleans empty set ---
    #[tokio::test]
    async fn connect_switching_signal_moves_peer_and_removes_empty_signal() {
        // initial state: peer 1 connected to s_old
        let s_old = sig("old", 1, 1);
        let s_new = sig("new", 2, 1);

        let mut sigs: HashMap<SignalInfo, HashSet<PeerIndex>> = HashMap::new();
        sigs.entry(s_old.clone()).or_default().insert(1);

        let mut peers: HashMap<PeerIndex, SignalInfo> = HashMap::new();
        peers.insert(1, s_old.clone());

        let sig_peers = Arc::new(RwLock::new((sigs, peers)));

        // now switch to new signal
        handle_peer_signal_state_changes(PeerSignalStateChange::Connect(1, s_new.clone()), &sig_peers).await;

        let lock = sig_peers.read().await;
        let (sigs, peers) = &*lock;

        // peer now points to new signal
        assert_eq!(peers.get(&1), Some(&s_new));

        // old signal set should be removed since it becomes empty
        assert!(!sigs.contains_key(&s_old));

        // new signal set has the peer
        let set_new = sigs.get(&s_new).expect("new signal entry");
        assert!(set_new.contains(&1));
    }

    // --- handle_peer_signal_state_changes: DisConnect ---
    #[tokio::test]
    async fn disconnect_removes_peer_and_cleans_signal_when_empty() {
        let s = sig("h", 2, 2);
        let mut sigs: HashMap<SignalInfo, HashSet<PeerIndex>> = HashMap::new();
        sigs.entry(s.clone()).or_default().extend([10, 11]);

        let mut peers: HashMap<PeerIndex, SignalInfo> = HashMap::new();
        peers.insert(10, s.clone());
        peers.insert(11, s.clone());

        let sig_peers = Arc::new(RwLock::new((sigs, peers)));

        // disconnect 10 from s
        handle_peer_signal_state_changes(PeerSignalStateChange::DisConnect(10, s.clone()), &sig_peers).await;

        {
            let lock = sig_peers.read().await;
            let (sigs, peers) = &*lock;
            // peer 10 removed, peer 11 remains
            assert!(!peers.contains_key(&10));
            assert_eq!(peers.get(&11), Some(&s));
            let set = sigs.get(&s).unwrap();
            assert!(!set.contains(&10) && set.contains(&11));
        }

        // disconnect 11 too -> set should be removed entirely
        handle_peer_signal_state_changes(PeerSignalStateChange::DisConnect(11, s.clone()), &sig_peers).await;

        let lock = sig_peers.read().await;
        let (sigs, peers) = &*lock;
        assert!(!peers.contains_key(&11));
        assert!(!sigs.contains_key(&s));
    }

    // --- handle_peer_signal_state_changes: DisConnect with mismatched signal should not remove mapping ---
    #[tokio::test]
    async fn disconnect_with_wrong_signal_does_not_remove_peer_mapping() {
        let s_real = sig("real", 3, 3);
        let s_wrong = sig("wrong", 4, 3);

        let mut sigs: HashMap<SignalInfo, HashSet<PeerIndex>> = HashMap::new();
        sigs.entry(s_real.clone()).or_default().insert(20);

        let mut peers: HashMap<PeerIndex, SignalInfo> = HashMap::new();
        peers.insert(20, s_real.clone());

        let sig_peers = Arc::new(RwLock::new((sigs, peers)));

        handle_peer_signal_state_changes(PeerSignalStateChange::DisConnect(20, s_wrong.clone()), &sig_peers).await;

        let lock = sig_peers.read().await;
        let (sigs, peers) = &*lock;
        // Mapping should remain unchanged
        assert_eq!(peers.get(&20), Some(&s_real));
        let set = sigs.get(&s_real).unwrap();
        assert!(set.contains(&20));
        // wrong signal shouldn't appear
        assert!(!sigs.contains_key(&s_wrong));
    }

    // --- handle_peer_signal_state_changes: All ---
    #[tokio::test]
    async fn all_replaces_signal_set_connects_and_disconnects_accordingly() {
        // old set: {1,2} on sA; peer 3 is on sB and should move to sA if included
        let s_a = sig("A", 10, 1);
        let s_b = sig("B", 20, 1);

        let mut sigs: HashMap<SignalInfo, HashSet<PeerIndex>> = HashMap::new();
        sigs.insert(s_a.clone(), HashSet::from([1, 2]));
        sigs.insert(s_b.clone(), HashSet::from([3]));

        let mut peers: HashMap<PeerIndex, SignalInfo> = HashMap::new();
        peers.insert(1, s_a.clone());
        peers.insert(2, s_a.clone());
        peers.insert(3, s_b.clone());

        let sig_peers = Arc::new(RwLock::new((sigs, peers)));

        // New set for sA: {2,3,4} -> 1 should disconnect, 3 should move from sB to sA, 4 should connect new
        handle_peer_signal_state_changes(PeerSignalStateChange::All(vec![2, 3, 4], s_a.clone()), &sig_peers).await;

        let lock = sig_peers.read().await;
        let (sigs, peers) = &*lock;

        // sA now has {2,3,4}
        let set_a = sigs.get(&s_a).unwrap();
        assert_eq!(set_a.len(), 3);
        assert!(set_a.contains(&2) && set_a.contains(&3) && set_a.contains(&4));
        // peer mappings
        assert_eq!(peers.get(&2), Some(&s_a));
        assert_eq!(peers.get(&3), Some(&s_a));
        assert_eq!(peers.get(&4), Some(&s_a));
        // 1 removed from peers
        assert!(!peers.contains_key(&1));
        // If sB had only 3, after moving it should be removed
        assert!(!sigs.contains_key(&s_b));
    }

    // --- route()/heartbeat(): verify formatting via filter and 404 on unknown ---
    #[tokio::test]
    async fn heartbeat_returns_human_duration_and_unknown_path_404() {
        // Build a minimal ServerInfo using the fields exercised by route()/heartbeat()
        // We'll reuse a CancellationToken and mpsc/broadcast as placeholders.
        use tokio_util::sync::CancellationToken;
        use dashmap::DashMap;
        use tokio::sync::{broadcast, mpsc};

        // Prepare a dummy ServerInfo-compatible instance from super context
        // Note: We access the constructor via Signal::new(...) + run(..) typically, but that binds sockets.
        // Instead, construct Arc<ServerInfo> via available struct in scope.
        let token = CancellationToken::new();
        let (tx_state, rx_state) = broadcast::channel(super::BUFFER_SIZE);
        let (tx_new_sig, _rx_new_sig) = mpsc::channel(super::BUFFER_SIZE);

        let server_info = Arc::new(ServerInfo {
            shutdown_token: token.clone(),
            signal_info: sig("localhost", 0, 0),
            received_connection_state_changes: mpsc::channel::<MessageT<PeerSignalStateChange>>(super::BUFFER_SIZE).0,
            sigs_peers: Arc::new(RwLock::new((HashMap::new(), HashMap::new()))),
            signal_conns: DashMap::new(),
            peer_conns: DashMap::new(),
            connection_state_changes_rx: rx_state,
            connection_state_changes_tx: tx_state,
            new_sig_tx: tx_new_sig,
        });

        // Build the filter
        let signing = super::SigningKey::from_bytes([0u8; 32]); // deterministic test key if available
        let filter = route(signing, server_info);

        // Call heartbeat with a known baseline: pretend started 1.2 seconds earlier
        let on = Instant::now() - Duration::from_millis(1200);
        let hb = heartbeat(on);

        // Using warp::test to drive the filter directly
        let resp = warp::test::request()
            .method("GET")
            .path("/heartbeat")
            .reply(&hb)
            .await;

        assert_eq!(resp.status(), 200);
        let body = std::str::from_utf8(resp.body()).unwrap();
        // Acceptable outputs include "1s200ms" or "1s" "200ms" order defined by implementation; here it's seconds then millis.
        assert!(body.contains('s'), "expected seconds marker, got {body}");
        // The duration should not be empty
        assert!(!body.trim().is_empty());

        // Unknown path on the route() filter returns 404
        let resp_404 = warp::test::request()
            .method("GET")
            .path("/does-not-exist")
            .reply(&filter)
            .await;
        assert_eq!(resp_404.status(), 404);
    }

    // --- heartbeat edge cases: exactly zero and millis-only ---
    #[tokio::test]
    async fn heartbeat_zero_and_millis_only_cases() {
        // zero duration -> "0s"
        let on = Instant::now();
        let hb = heartbeat(on);
        let resp = warp::test::request().method("GET").path("/heartbeat").reply(&hb).await;
        assert_eq!(std::str::from_utf8(resp.body()).unwrap(), "0s");

        // small millis -> should render ms component
        let on2 = Instant::now() - Duration::from_millis(5);
        let hb2 = heartbeat(on2);
        let resp2 = warp::test::request().method("GET").path("/heartbeat").reply(&hb2).await;
        let b2 = std::str::from_utf8(resp2.body()).unwrap().to_string();
        assert!(b2.ends_with("ms") && b2.contains('5'));
    }

    // --- Composite formatting: hours, minutes, seconds, millis combined ---
    #[tokio::test]
    async fn heartbeat_composite_format_includes_all_components_when_nonzero() {
        // Simulate ~1h2m3s45ms ago (using standard Duration arithmetic)
        let dur = Duration::from_secs(1*3600 + 2*60 + 3) + Duration::from_millis(45);
        let on = Instant::now() - dur;
        let hb = heartbeat(on);
        let resp = warp::test::request().method("GET").path("/heartbeat").reply(&hb).await;
        let body = std::str::from_utf8(resp.body()).unwrap().to_string();
        // All components should appear in order h, m, s, ms if non-zero
        assert!(body.contains('h'), "missing hours in {body}");
        assert!(body.contains('m'), "missing minutes in {body}");
        assert!(body.contains('s'), "missing seconds in {body}");
        assert!(body.contains("ms"), "missing millis in {body}");
    }
}