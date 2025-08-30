// Integration test scaffold for harmion-connect
// Uses Rust's built-in test framework and Tokio for async tests.

// ===========================
// Tests appended by CI helper
// Framework: Rust built-in test harness + Tokio for async tests
// ===========================

use std::time::Duration;

#[test]
fn google_stun_list_has_expected_length_and_contents() {
    // Focus: GOOGLE_STUN_LIST constant
    // Validate length and presence of all known endpoints.
    assert_eq!(GOOGLE_STUN_LIST.len(), 10, "Expected 10 Google STUN entries");
    let expected = [
        "stun:stun.1.google.com:19302",
        "stun:stun.1.google.com:5349",
        "stun:stun1.1.google.com:3478",
        "stun:stun1.1.google.com:5349",
        "stun:stun2.1.google.com:19302",
        "stun:stun2.1.google.com:5349",
        "stun:stun3.1.google.com:3478",
        "stun:stun3.1.google.com:5349",
        "stun:stun4.1.google.com:19302",
        "stun:stun4.1.google.com:5349",
    ];
    for item in expected {
        assert!(GOOGLE_STUN_LIST.contains(&item), "Missing STUN entry: {item}");
    }
}

#[test]
fn config_default_populates_stun_from_google_list() {
    // Focus: Config::default uses GOOGLE_STUN_LIST
    let cfg = Config::default();
    assert_eq!(cfg.stun.len(), GOOGLE_STUN_LIST.len());
    for (i, s) in GOOGLE_STUN_LIST.iter().enumerate() {
        assert_eq!(&cfg.stun[i], s);
    }
}

#[test]
fn peer_error_display_messages_are_meaningful() {
    // Focus: PeerError variants and Display messages
    // We avoid constructing external webrtc/rmp_serde errors here; test stable local variants.
    let e = PeerError::PeerNotConnected;
    assert!(format!("{e}").to_lowercase().contains("peer not connected"));

    let e = PeerError::DataChannelNotAvailable;
    assert!(format!("{e}").to_lowercase().contains("data channel"));

    let e = PeerError::ConnectionFailed;
    assert!(format!("{e}").to_lowercase().contains("failed"));

    let e = PeerError::Other;
    assert!(!format!("{e}").is_empty());
}

#[test]
fn peer_alias_new_and_display_roundtrip() {
    // Focus: PeerAlias::new and Display/AsRef implementations
    let a = PeerAlias::new("alias-1234".to_string());
    assert_eq!(a.as_ref(), "alias-1234");
    assert_eq!(a.to_string(), "alias-1234");
}

#[test]
fn peer_alias_from_key_generates_expected_size() {
    // Focus: PeerAlias::from_key logic: bs58(SHA256(key))[..8]
    // We don't assert an exact value (non-deterministic without a fixed key),
    // but we assert non-empty and length <= 8 as implemented.
    use ed25519_dalek::{SigningKey, VerifyingKey, SECRET_KEY_LENGTH};
    use rand::rngs::StdRng;
    use rand::{SeedableRng, RngCore};

    // Deterministic key for reproducibility
    let mut rng = StdRng::seed_from_u64(42);
    let mut sk_bytes = [0u8; SECRET_KEY_LENGTH];
    rng.fill_bytes(&mut sk_bytes);
    let sk = SigningKey::from_bytes(&sk_bytes);
    let vk: VerifyingKey = sk.verifying_key();

    let alias = PeerAlias::from_key(vk);
    let s = alias.to_string();
    assert!(!s.is_empty());
    assert!(s.len() <= 8, "Alias should be at most 8 characters, got {}", s.len());
    // Ensure it's valid base58 (decode should succeed)
    bs58::decode(&s).into_vec().expect("alias must be valid base58 prefix");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn peer_state_transitions_to_connected_on_channel_open_waiting_paths() {
    // Focus: Peer<WaitingICE>::wait transitions and on_open handlers set state to Connected
    // Construct minimal flow to exercise 'wait' path with ready notification.
    // Note: We avoid network/STUN by using empty STUN list to favor host candidates and in-process signaling.

    // Build offer on A (Peer::new)
    let cfg_a = Config { stun: vec![] };
    let peer_id_a = PeerAlias::new("peerA".to_string());
    let (peer_a_waiting_answer, offer) = Peer::<WaitingAnswer>::new(cfg_a, &peer_id_a).await.expect("A new");

    // Build B from offer (Peer::from_offer)
    let cfg_b = Config { stun: vec![] };
    let peer_id_b = PeerAlias::new("peerB".to_string());
    let (peer_b_waiting_ice, answer) = Peer::<WaitingICE>::from_offer(&offer, cfg_b, &peer_id_b).await.expect("B from offer");

    // A sets remote answer (Peer<WaitingAnswer>::set_remote_answer) -> Connected
    let mut peer_a_connected = peer_a_waiting_answer.set_remote_answer(&answer).await.expect("A set remote answer");
    assert_eq!(peer_a_connected.get_peer_states().await, PeerState::Connected);

    // B waits for data channel ready (Peer<WaitingICE>::wait) -> Connected
    let mut peer_b_connected = peer_b_waiting_ice.wait().await.expect("B wait");
    assert_eq!(peer_b_connected.get_peer_states().await, PeerState::Connected);

    // Sanity: disconnect should succeed and close resources
    peer_a_connected.disconnect().await.expect("A disconnect");
    peer_b_connected.disconnect().await.expect("B disconnect");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connected_peer_send_and_recv_happy_path() {
    // Focus: Peer<Connected>::send, receiver, and Connection impl send/recv
    // Establish a loopback pair and exchange a message.
    use super::Message;

    // Build peers
    let cfg_a = Config { stun: vec![] };
    let peer_id_a = PeerAlias::new("peerA2".to_string());
    let (peer_a_waiting_answer, offer) = Peer::<WaitingAnswer>::new(cfg_a, &peer_id_a).await.expect("A new");

    let cfg_b = Config { stun: vec![] };
    let peer_id_b = PeerAlias::new("peerB2".to_string());
    let (peer_b_waiting_ice, answer) = Peer::<WaitingICE>::from_offer(&offer, cfg_b, &peer_id_b).await.expect("B from offer");

    let mut a = peer_a_waiting_answer.set_remote_answer(&answer).await.expect("A connected");
    let mut b = peer_b_waiting_ice.wait().await.expect("B connected");

    // Send a small message from A to B
    let msg = Message::Ping(123); // Adjust to an existing Message variant if different
    a.send(msg.clone()).await.expect("A send ok");

    // B should receive it
    let received = tokio::time::timeout(Duration::from_secs(10), b.receiver().recv())
        .await
        .expect("B timed out receiving");
    assert_eq!(received, Some(msg));

    // Clean up
    a.disconnect().await.ok();
    b.disconnect().await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_fails_if_data_channel_not_ready_or_closed() {
    // Focus: Peer<Connected>::send error path when dc is None or not Open
    // Strategy: Connect peers, then close dc on receiver side and attempt send to trigger PeerNotConnected.
    // Note: ReadyState transitions are event-driven; we simulate by closing pc, which triggers dc close.
    let cfg_a = Config { stun: vec![] };
    let peer_id_a = PeerAlias::new("peerA3".to_string());
    let (peer_a_waiting_answer, offer) = Peer::<WaitingAnswer>::new(cfg_a, &peer_id_a).await.expect("A new");

    let cfg_b = Config { stun: vec![] };
    let peer_id_b = PeerAlias::new("peerB3".to_string());
    let (peer_b_waiting_ice, answer) = Peer::<WaitingICE>::from_offer(&offer, cfg_b, &peer_id_b).await.expect("B from offer");

    let mut a = peer_a_waiting_answer.set_remote_answer(&answer).await.expect("A connected");
    let b = peer_b_waiting_ice.wait().await.expect("B connected");

    // Close B to cause A's data channel to eventually close or at least fail sends
    // (ICE disconnection -> dc close callback)
    b.disconnect().await.ok();

    // Give a moment for state to propagate
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Expect PeerNotConnected on send
    let res = a.send(super::Message::Pong(321)).await;
    assert!(matches!(res, Err(PeerError::PeerNotConnected) | Err(PeerError::WebRTC(_))));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inner_message_pack_roundtrip_and_corrupt_payload_handling() {
    // Focus: set_data_channel_callbacks: rmp_serde encode/decode branches and error logging path
    // Validate that InnerMessage encodes/decodes and that corrupted payload doesn't panic.

    // Encode a valid InnerMessage and decode it back
    let original = InnerMessage::Message(Box::new(super::Message::Ping(7)));
    let packed = rmp_serde::to_vec(&original).expect("pack");
    let decoded: InnerMessage = rmp_serde::from_slice(&packed).expect("unpack");
    match decoded {
        InnerMessage::Message(b) => assert_eq!(*b, super::Message::Ping(7)),
        _ => panic!("unexpected variant"),
    }

    // Corrupt payload: ensure deserialization error occurs (handled via warn! in callbacks; here we just ensure it fails)
    let bad = vec![0xde, 0xad, 0xbe, 0xef];
    let res: Result<InnerMessage, _> = rmp_serde::from_slice(&bad);
    assert!(res.is_err(), "Corrupt MessagePack should error");
}