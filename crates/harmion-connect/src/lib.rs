// Topic, Connectionなどいろいろ考えれてないことばかり

mod topic;
mod webrtc;

use topic::TopicTree;

pub trait Subscriber {
    type E: std::error::Error;

    fn subscribe(&self, topic: &TopicTree) -> Result<(), Self::E>;
    fn un_subscribe(&self, topic: &TopicTree) -> Result<(), Self::E>;
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

    async fn send(&mut self, topic: &TopicTree, message: String) -> Result<(), Self::E>; // Messageを作る
}

pub trait Receiver {
    type E: std::error::Error;

    fn topic(&self) -> &TopicTree;
    async fn recv(&mut self) -> Result<Message, Self::E>;
}

use serde::de::DeserializeOwned;

pub trait Connection
where
    Self: Subscriber + Sender,
{
    type PeerInfo: Serialize + DeserializeOwned; // SDP
    type PeerID: Serialize + DeserializeOwned; // Node Ident

    type E: std::error::Error;

    fn node_info(&self) -> &(Self::PeerID, Self::PeerInfo);
    fn node_list(&self) -> &[(Self::PeerID, Self::PeerInfo)];

    fn receiver(&self, topic: &TopicTree) -> impl Receiver;

    async fn connect(
        &mut self,
        node: Self::PeerInfo,
        node_id: Option<Self::PeerID>,
    ) -> Result<Self::PeerID, <Self as Connection>::E>;
}
