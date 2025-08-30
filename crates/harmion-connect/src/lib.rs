// Topic, Connectionなどいろいろ考えれてないことばかり

pub mod topic;
pub mod webrtc;

pub use topic::TopicTree;

pub trait Subscriber {
    type E: std::error::Error;

    fn subscribe(
        &self,
        topic: &TopicTree,
    ) -> impl std::future::Future<Output = Result<(), Self::E>> + Send;
    fn un_subscribe(
        &self,
        topic: &TopicTree,
    ) -> impl std::future::Future<Output = Result<(), Self::E>> + Send;
}

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

const NONCE_LEN: usize = 12;

#[derive(Copy, Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct PeerIndex(VerifyingKey);

impl<T: Into<VerifyingKey>> From<T> for PeerIndex {
    fn from(value: T) -> Self {
        Self(value.into())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Message {
    pub content: Vec<u8>,
    pub timestamp: u64,

    pub origin: PeerIndex,
    pub sig: Signature,

    pub nonce: [u8; NONCE_LEN],
}

use rand::{rngs::ThreadRng, RngCore}; // TODO: Reseed?

impl Message {
    pub fn new(content: Vec<u8>, key: &SigningKey) -> Self {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("System time is before the UNIX epoch")
            .as_secs();

        let mut data_to_sign = Vec::with_capacity(
            content.len() + std::mem::size_of::<u64>() + std::mem::size_of::<[u8; NONCE_LEN]>(),
        );
        data_to_sign.extend_from_slice(&content);
        data_to_sign.extend_from_slice(&timestamp.to_be_bytes());

        let mut rng = ThreadRng::default();
        let mut nonce = [0; NONCE_LEN];
        rng.fill_bytes(&mut nonce);

        data_to_sign.extend_from_slice(&nonce);

        Self {
            sig: key.sign(&data_to_sign),
            origin: PeerIndex(key.verifying_key()),
            nonce,

            content,
            timestamp,
        }
    }

    pub fn from<T: Serialize>(
        value: &T,
        key: &SigningKey,
    ) -> Result<Self, rmp_serde::encode::Error> {
        Ok(Self::new(rmp_serde::to_vec(value)?, key))
    }

    pub fn verify(&self) -> bool {
        (self.origin)
            .0
            .verify(
                &{
                    let mut data = Vec::with_capacity(
                        self.content.len()
                            + std::mem::size_of::<u64>()
                            + std::mem::size_of::<[u8; NONCE_LEN]>(),
                    );
                    data.extend_from_slice(&self.content);
                    data.extend_from_slice(&self.timestamp.to_be_bytes());
                    data.extend_from_slice(&self.nonce);

                    data
                },
                &self.sig,
            )
            .is_ok()
    }
}

use serde::de::DeserializeOwned;

pub struct MessageT<T: DeserializeOwned> {
    pub content: T,
    pub timestamp: u64,

    pub origin: PeerIndex,
    pub sig: Signature,

    pub nonce: [u8; NONCE_LEN],

    pub verify_result: bool,
}

impl<T: DeserializeOwned> TryFrom<Message> for MessageT<T> {
    type Error = rmp_serde::decode::Error;

    fn try_from(value: Message) -> Result<Self, rmp_serde::decode::Error> {
        let verify_res = value.verify();
        let content: T = rmp_serde::from_slice(&value.content)?;

        Ok(Self {
            content,
            timestamp: value.timestamp,
            origin: value.origin,
            sig: value.sig,
            nonce: value.nonce,
            verify_result: verify_res,
        })
    }
}

trait Connection<T, R> {
    type Error: std::error::Error;

    async fn send(&mut self, value: T) -> Result<(), Self::Error>;
    async fn recv(&mut self) -> Result<Option<R>, Self::Error>;
}

use tokio::sync::mpsc;

impl<T, R> Connection<T, R> for (mpsc::Sender<T>, mpsc::Receiver<R>) {
    type Error = mpsc::error::SendError<T>;

    async fn send(&mut self, value: T) -> Result<(), Self::Error> {
        self.0.send(value).await
    }

    async fn recv(&mut self) -> Result<Option<R>, Self::Error> {
        Ok(self.1.recv().await)
    }
}

// Encrypt<T: Connection<Message, Message>>: Connection<T, MessageT<Decrypted<T>>>
// or Messageにもっと組み込む
// Handshakeする

#[cfg(test)]
mod tests {
    // Test framework note:
    // - Framework: Rust built-in test harness (#[test])
    // - Async: Manual tokio runtime via tokio::runtime::Builder (no additional test frameworks)
    use super::*;
    use ed25519_dalek::SigningKey;
    use serde::{Deserialize, Serialize};
    use std::collections::HashSet;

    fn fixed_signing_key(byte: u8) -> SigningKey {
        // Deterministic signing key for reproducible tests
        let bytes = [byte; 32];
        SigningKey::from_bytes(&bytes)
    }

    #[test]
    fn message_new_and_verify_success() {
        let key = fixed_signing_key(1);
        let content = b"hello".to_vec();

        let msg = Message::new(content.clone(), &key);

        assert_eq!(msg.content, content, "content should be preserved");
        assert_eq!(msg.origin, PeerIndex(key.verifying_key()), "origin should match verifying key");
        assert_eq!(msg.nonce.len(), NONCE_LEN, "nonce length must match NONCE_LEN");
        assert!(msg.verify(), "freshly created message must verify");
    }

    #[test]
    fn message_verify_fails_when_content_mutated() {
        let key = fixed_signing_key(2);
        let mut msg = Message::new(b"hi".to_vec(), &key);

        msg.content.push(0);

        assert!(!msg.verify(), "mutating content should invalidate signature");
    }

    #[test]
    fn message_verify_fails_when_nonce_mutated() {
        let key = fixed_signing_key(3);
        let mut msg = Message::new(b"hi".to_vec(), &key);

        msg.nonce[0] ^= 0xFF;

        assert!(!msg.verify(), "mutating nonce should invalidate signature");
    }

    #[test]
    fn message_verify_fails_when_timestamp_mutated() {
        let key = fixed_signing_key(4);
        let mut msg = Message::new(b"hi".to_vec(), &key);

        // Flip one bit to ensure a different timestamp value
        msg.timestamp ^= 1;

        assert!(!msg.verify(), "mutating timestamp should invalidate signature");
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct Sample {
        a: u32,
        b: String,
    }

    #[test]
    fn message_from_roundtrip_and_tryfrom_success() {
        let key = fixed_signing_key(5);
        let data = Sample { a: 42, b: "z".into() };

        let msg = Message::from(&data, &key).expect("encoding to msgpack should succeed");
        assert!(msg.verify(), "encoded message should verify");

        let decoded: MessageT<Sample> =
            MessageT::try_from(msg).expect("decoding MessageT<Sample> should succeed");

        assert_eq!(decoded.content, data, "decoded payload should equal original");
        assert_eq!(decoded.origin, PeerIndex(key.verifying_key()), "origin should round-trip");
        assert!(decoded.verify_result, "verify_result should reflect a valid signature");
    }

    #[test]
    fn message_tryfrom_fails_on_type_mismatch() {
        let key = fixed_signing_key(6);

        // Encode raw bytes that represent a msgpack string; decoding into a struct must fail.
        let msg = Message::new(b"plain-bytes".to_vec(), &key);

        let res: Result<MessageT<Sample>, _> = MessageT::try_from(msg);
        assert!(res.is_err(), "decoding string bytes as struct should error");
    }

    #[test]
    fn peer_index_equality_and_hashing() {
        let k1 = fixed_signing_key(7);
        let k2 = fixed_signing_key(8);

        let p1 = PeerIndex(k1.verifying_key());
        let p1b = PeerIndex(k1.verifying_key());
        let p2 = PeerIndex(k2.verifying_key());

        assert_eq!(p1, p1b, "same verifying key should be equal");
        assert_ne!(p1, p2, "different verifying keys should not be equal");

        let mut set = HashSet::new();
        set.insert(p1);
        set.insert(p1b);
        set.insert(p2);
        assert_eq!(set.len(), 2, "hashing should treat identical keys as one entry");
    }

    // Exercise the private Connection trait implemented for (Sender<T>, Receiver<R>).
    // We build a minimal current-thread tokio runtime to drive async send/recv.
    #[test]
    fn connection_tuple_send_and_recv_happy_path_and_closed_channel() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("failed to build tokio runtime");

        rt.block_on(async {
            use tokio::sync::mpsc;

            // Channel for outbound T
            let (tx_t, mut rx_t) = mpsc::channel::<u8>(1);
            // Channel for inbound R
            let (tx_r, rx_r) = mpsc::channel::<String>(1);

            let mut conn: (mpsc::Sender<u8>, mpsc::Receiver<String>) = (tx_t, rx_r);

            // send() should forward to tx_t
            Connection::<u8, String>::send(&mut conn, 9)
                .await
                .expect("send ok");
            let got_t = rx_t.recv().await.expect("rx_t should yield a value");
            assert_eq!(got_t, 9, "send should deliver payload");

            // recv() should read from rx_r (after we feed it via its paired sender)
            tx_r.send("ok".to_owned()).await.expect("tx_r send");
            let got_r = Connection::<u8, String>::recv(&mut conn)
                .await
                .expect("recv ok")
                .expect("should be Some");
            assert_eq!(got_r, "ok", "recv should yield the payload sent into rx_r");

            // When peer sender is dropped, recv() should return Ok(None)
            drop(tx_r);
            let none = Connection::<u8, String>::recv(&mut conn)
                .await
                .expect("recv ok");
            assert!(none.is_none(), "recv should be None after sender is dropped");
        });
    }
}
