// Topic, Connectionなどいろいろ考えれてないことばかり

mod topic;
mod webrtc;

use topic::TopicTree;

pub trait Subscriber {
    type E: std::error::Error;

    fn subscribe(&self, topic: &TopicTree) -> Result<(), Self::E>;
    fn un_subscribe(&self, topic: &TopicTree) -> Result<(), Self::E>;
}

pub trait Message {}

pub trait Sender {
    type E: std::error::Error;

    async fn send(&mut self, topic: &TopicTree, message: impl Message) -> Result<(), Self::E>;
}

pub trait Receiver {
    type E: std::error::Error;

    fn topic(&self) -> &TopicTree;
    async fn recv(&mut self) -> Result<impl Message, Self::E>;
}

use serde::{de::DeserializeOwned, Serialize};

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
