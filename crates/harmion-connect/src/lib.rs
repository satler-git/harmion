// Topic, Connectionなどいろいろ考えれてないことばかり

mod topic;
mod webrtc;

use topic::TopicTree;

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Message {
    pub content: Vec<u8>,
    pub timestamp: u64,

    pub origin: VerifyingKey,
    pub sig: Signature,

    pub nonce: [u8; NONCE_LEN],
}

// TODO: replay-cache(3~10m)
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
            origin: key.verifying_key(),
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

pub trait Sender {
    type E: std::error::Error;

    fn send(
        &mut self,
        topic: &TopicTree,
        message: String,
    ) -> impl std::future::Future<Output = Result<(), Self::E>> + Send; // Messageを作る
}

pub trait Receiver {
    type E: std::error::Error;

    fn topic(&self) -> &TopicTree;
    fn recv(&mut self) -> impl std::future::Future<Output = Result<Message, Self::E>> + Send;
}

use serde::de::DeserializeOwned;

pub trait Connection
where
    Self: Subscriber + Sender,
{
    type PeerInfo: Serialize + DeserializeOwned; // SDP
    type PeerID: Serialize + DeserializeOwned; // Node Ident

    type E: std::error::Error;

    fn node_info(
        &self,
    ) -> impl std::future::Future<Output = &(Self::PeerID, Self::PeerInfo)> + Send;
    fn node_list(
        &self,
    ) -> impl std::future::Future<Output = &[(Self::PeerID, Self::PeerInfo)]> + Send;

    fn receiver(
        &self,
        topic: &TopicTree,
    ) -> impl std::future::Future<Output = impl Receiver> + Send;

    fn connect(
        &mut self,
        node: Self::PeerInfo,
        node_id: Option<Self::PeerID>,
    ) -> impl std::future::Future<Output = Result<Self::PeerID, <Self as Connection>::E>> + Send;
}
