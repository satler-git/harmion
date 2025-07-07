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

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub content: String,
    pub timestamp: u64,
}

impl Message {
    pub fn new(content: String) -> Self {
        Self {
            content,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("System time is before the UNIX epoch")
                .as_secs(),
        }
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
