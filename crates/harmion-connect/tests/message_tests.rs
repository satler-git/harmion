//! Tests for Message, MessageT<T>, PeerIndex, and Connection tuple impl.
//!
//! Frameworks used:
//! - Rust built-in test framework (cargo test)
//! - Tokio's async test attribute for async tests involving mpsc

use harmion_connect::{Message, MessageT, PeerIndex, TopicTree};
use ed25519_dalek::{SigningKey, VerifyingKey};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};

#[test]
fn message_new_produces_valid_signature_and_fields() {
    // Arrange
    let mut rng = OsRng;
    let key = SigningKey::generate(&mut rng);
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let content = b"hello world".to_vec();

    // Act
    let msg = Message::new(content.clone(), &key);

    let after = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // Assert
    assert_eq!(msg.content, content, "content should be preserved");
    assert_eq!(msg.origin, PeerIndex::from(key.verifying_key()), "origin should match verifying key");
    assert_eq!(msg.nonce.len(), 12, "nonce must be 12 bytes (NONCE_LEN)");
    // timestamp should be within [before, after]
    assert!(msg.timestamp >= before && msg.timestamp <= after, "timestamp should be current unix time");
    assert!(msg.verify(), "freshly created message must verify");
}

#[test]
fn message_verify_fails_if_content_tampered() {
    let mut rng = OsRng;
    let key = SigningKey::generate(&mut rng);
    let mut msg = Message::new(b"authentic".to_vec(), &key);

    // Tamper content
    msg.content = b"tampered".to_vec();
    assert!(!msg.verify(), "verification must fail when content is changed");
}

#[test]
fn message_verify_fails_if_nonce_tampered() {
    let mut rng = OsRng;
    let key = SigningKey::generate(&mut rng);
    let mut msg = Message::new(b"bytes".to_vec(), &key);

    // Tamper nonce
    msg.nonce[0] ^= 0xFF;
    assert!(!msg.verify(), "verification must fail when nonce changes");
}

#[test]
fn message_verify_fails_if_origin_key_changed() {
    let mut rng = OsRng;
    let key1 = SigningKey::generate(&mut rng);
    let key2 = SigningKey::generate(&mut rng);
    let mut msg = Message::new(b"payload".to_vec(), &key1);

    // Change origin to a different verifying key (signature remains from key1)
    msg.origin = PeerIndex::from(key2.verifying_key());
    assert!(!msg.verify(), "verification must fail when origin/pubkey mismatches signature");
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Demo {
    a: u32,
    b: String,
}

#[test]
fn message_from_and_try_from_roundtrip_success() {
    let mut rng = OsRng;
    let key = SigningKey::generate(&mut rng);
    let value = Demo { a: 42, b: "xyz".into() };

    // Create message using Message::from (msgpack encoded)
    let msg = Message::from(&value, &key).expect("encoding should succeed");
    assert!(msg.verify(), "encoded message should verify");

    // Convert to typed message
    let typed: MessageT<Demo> = msg.try_into().expect("decoding should succeed");
    assert_eq!(typed.content, value, "roundtrip content should match");
    assert!(typed.verify_result, "verify_result should be true for intact message");
}

#[test]
fn message_t_try_from_decode_error_for_invalid_content() {
    // Construct a Message with invalid msgpack content for target type
    let mut rng = OsRng;
    let key = SigningKey::generate(&mut rng);

    let mut msg = Message::new(b"not msgpack for u32".to_vec(), &key);
    // Ensure verify passes so that decode error is the observed failure
    assert!(msg.verify(), "signature should verify even if content is not valid msgpack for target type");

    let res: Result<MessageT<u32>, _> = msg.try_into();
    assert!(res.is_err(), "decoding invalid msgpack into u32 should fail");
}

// Additional test to ensure timestamp participates in signature:
// Changing the timestamp should invalidate signature.
#[test]
fn message_verify_fails_if_timestamp_tampered() {
    let mut rng = OsRng;
    let key = SigningKey::generate(&mut rng);
    let mut msg = Message::new(b"time".to_vec(), &key);

    msg.timestamp = msg.timestamp.saturating_add(1);
    assert!(!msg.verify(), "verification must fail when timestamp changes");
}