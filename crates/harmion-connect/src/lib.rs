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

#[derive(Debug, Clone, PartialEq, Eq)]
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
