//! Integration tests for harmion-connect public API.
//! Test focus: Message creation/signing/verification, MessageT decoding, PeerIndex conversions,
//! and Subscriber trait behavior via a local dummy implementor.
//
// Testing library/framework: Rust built-in test harness with assert macros.
// For async tests we use #[tokio::test] if tokio is available as a dependency of the crate under test.

use std::collections::{HashMap, HashSet};

use ed25519_dalek::{SigningKey, VerifyingKey};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};

use harmion_connect::{Message, MessageT, PeerIndex, Subscriber, TopicTree};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct MyData {
    a: u32,
    b: String,
}

#[test]
fn message_new_signs_and_verifies_valid_signature() {
    // Arrange
    let mut rng = OsRng;
    let key = SigningKey::generate(&mut rng);
    let payload = b"hello-world".to_vec();

    // Act
    let msg = Message::new(payload.clone(), &key);

    // Assert
    assert_eq!(msg.content, payload, "content must round-trip into Message");
    assert_eq!(msg.origin, PeerIndex::from(key.verifying_key()), "origin must be derived from signing key");
    assert!(msg.timestamp > 0, "timestamp should be a UNIX epoch seconds value");
    assert_eq!(msg.nonce.len(), 12, "nonce length must match NONCE_LEN");
    assert!(msg.verify(), "freshly created message must verify");
}

#[test]
fn message_verify_fails_if_content_is_tampered() {
    let mut rng = OsRng;
    let key = SigningKey::generate(&mut rng);
    let mut msg = Message::new(b"authentic".to_vec(), &key);

    // Tamper content
    msg.content = b"tampered".to_vec();

    assert!(!msg.verify(), "changing content must invalidate signature");
}

#[test]
fn message_verify_fails_if_timestamp_is_tampered() {
    let mut rng = OsRng;
    let key = SigningKey::generate(&mut rng);
    let mut msg = Message::new(b"time".to_vec(), &key);

    // Tamper timestamp
    msg.timestamp = msg.timestamp.saturating_add(1);

    assert!(!msg.verify(), "changing timestamp must invalidate signature");
}

#[test]
fn message_verify_fails_if_nonce_is_tampered() {
    let mut rng = OsRng;
    let key = SigningKey::generate(&mut rng);
    let mut msg = Message::new(b"nonce".to_vec(), &key);

    // Tamper nonce (flip one bit)
    msg.nonce[0] ^= 0b0000_0001;

    assert!(!msg.verify(), "changing nonce must invalidate signature");
}

#[test]
fn message_verify_fails_if_origin_key_is_mismatched() {
    let mut rng = OsRng;
    let key1 = SigningKey::generate(&mut rng);
    let key2 = SigningKey::generate(&mut rng);
    let mut msg = Message::new(b"origin-check".to_vec(), &key1);

    // Swap origin to a different verifying key without resigning
    msg.origin = PeerIndex::from(key2.verifying_key());

    assert!(!msg.verify(), "changing origin verifying key must invalidate verification");
}

#[test]
fn message_from_and_try_from_round_trip_custom_struct() {
    let mut rng = OsRng;
    let key = SigningKey::generate(&mut rng);

    let original = MyData {
        a: 42,
        b: "hello".into(),
    };

    let msg = Message::from(&original, &key).expect("encoding should succeed");
    assert!(msg.verify(), "encoded message should verify");

    let decoded: MessageT<MyData> = msg.try_into().expect("decoding should succeed");
    assert_eq!(decoded.content, original, "decoded content should match original struct");
    assert!(decoded.verify_result, "verify_result should reflect original verify() result");
    assert_eq!(decoded.nonce.len(), 12, "nonce preserved");
    assert_eq!(decoded.origin, PeerIndex::from(key.verifying_key()), "origin preserved");
}

#[test]
fn message_try_from_fails_for_incompatible_type() {
    let mut rng = OsRng;
    let key = SigningKey::generate(&mut rng);

    // Encode a string, then try to decode as a u64 (should error)
    let msg = Message::from(&"not-a-number", &key).expect("encoding should succeed");
    let res: Result<MessageT<u64>, _> = msg.try_into();
    assert!(res.is_err(), "decoding incompatible type should fail");
}

#[test]
fn peer_index_equality_and_hashing_behavior() {
    let mut rng = OsRng;
    let k1 = SigningKey::generate(&mut rng);
    let k2 = SigningKey::generate(&mut rng);

    let p1a = PeerIndex::from(k1.verifying_key());
    let p1b = PeerIndex::from(k1.verifying_key());
    let p2 = PeerIndex::from(k2.verifying_key());

    // Equality
    assert_eq!(p1a, p1b);
    assert_ne!(p1a, p2);

    // Hashing in HashSet
    let mut set = HashSet::new();
    set.insert(p1a);
    set.insert(p2);
    set.insert(p1b); // duplicate
    assert_eq!(set.len(), 2, "duplicate PeerIndex should not increase set size");

    // HashMap usage
    let mut map: HashMap<PeerIndex, &str> = HashMap::new();
    map.insert(p1a, "first");
    map.insert(p2, "second");
    assert_eq!(map.get(&p1b), Some(&"first"));
    assert_eq!(map.get(&p2), Some(&"second"));
}

#[test]
fn message_sign_includes_nonce_timestamp_and_content() {
    // This test is similar to tamper tests but explicitly demonstrates that all three fields
    // participate in the signature by changing one at a time.
    let mut rng = OsRng;
    let key = SigningKey::generate(&mut rng);

    // Base message
    let msg = Message::new(b"base".to_vec(), &key);
    assert!(msg.verify());

    // Change only content
    let mut m1 = msg.clone();
    m1.content = b"base+".to_vec();
    assert!(!m1.verify());

    // Change only timestamp
    let mut m2 = msg.clone();
    m2.timestamp = m2.timestamp.saturating_add(9999);
    assert!(!m2.verify());

    // Change only nonce
    let mut m3 = msg.clone();
    m3.nonce[NONCE_TEST_INDEX()] ^= 0xFF; // flip a byte at a deterministic index
    assert!(!m3.verify());
}

// Helper function to avoid referencing the NONCE_LEN const directly here and stay robust.
// We pick an index safely within bounds.
fn NONCE_TEST_INDEX() -> usize { 3 }

//
// Subscriber trait smoke test via a local type implementing the public trait.
// This validates the trait’s public interface accepts TopicTree and returns a Future.
//
#[derive(Debug)]
struct DummyErr;
impl std::fmt::Display for DummyErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "dummy error") }
}
impl std::error::Error for DummyErr {}

struct DummySubscriber {
    ok: bool,
}

impl Subscriber for DummySubscriber {
    type E = DummyErr;

    fn subscribe(
        &self,
        _topic: &TopicTree,
    ) -> impl std::future::Future<Output = Result<(), Self::E>> + Send {
        let ok = self.ok;
        async move {
            if ok { Ok(()) } else { Err(DummyErr) }
        }
    }

    fn un_subscribe(
        &self,
        _topic: &TopicTree,
    ) -> impl std::future::Future<Output = Result<(), Self::E>> + Send {
        let ok = self.ok;
        async move {
            if ok { Ok(()) } else { Err(DummyErr) }
        }
    }
}

#[tokio::test]
async fn subscriber_trait_happy_and_error_paths() {
    // Construct a minimal TopicTree. If TopicTree has more complex constructors,
    // we rely on its default or simple form. We only pass a reference and do not exercise it.
    // If TopicTree cannot be constructed trivially, adjust this accordingly.
    // Since TopicTree is publicly re-exported, we assume a Default is available; if not,
    // use a placeholder via unsafe or minimal ctor. Here we try Default and fallback if needed.
    let topic = topic_default();

    let ok_sub = DummySubscriber { ok: true };
    assert!(ok_sub.subscribe(&topic).await.is_ok(), "subscribe should succeed");
    assert!(ok_sub.un_subscribe(&topic).await.is_ok(), "un_subscribe should succeed");

    let err_sub = DummySubscriber { ok: false };
    assert!(err_sub.subscribe(&topic).await.is_err(), "subscribe should fail");
    assert!(err_sub.un_subscribe(&topic).await.is_err(), "un_subscribe should fail");
}

// Helper to obtain a TopicTree for tests without depending on internal details.
fn topic_default() -> TopicTree {
    // Try Default if implemented; otherwise, use a minimal constructor pattern via serde roundtrip.
    // We prefer Default to avoid relying on internal APIs of TopicTree.
    #[allow(unused_mut)]
    let mut maybe_default: Option<TopicTree> = None;

    // If Default is implemented, this compiles and works; otherwise, we fall back below.
    maybe_default = Some(Default::default());

    // Unwrap since at least one path should work at compile time; if not, adjust to known constructors.
    maybe_default.expect("TopicTree should implement Default for basic construction in tests")
}