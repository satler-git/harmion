// Topic, Connectionなどいろいろ考えれてないことばかり

mod topic;

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
    type NodeInfo: Serialize + DeserializeOwned; // SDP
    type NodeID: Serialize + DeserializeOwned; // Node Ident

    type E: std::error::Error;

    fn node_info(&self) -> &(Self::NodeInfo, Self::NodeInfo);
    fn node_list(&self) -> &[(Self::NodeInfo, Self::NodeInfo)];

    fn receiver(&self, topic: &TopicTree) -> impl Receiver;

    async fn connect(
        &mut self,
        node: Self::NodeInfo,
        node_id: Option<Self::NodeID>,
    ) -> Result<Self::NodeID, <Self as Connection>::E>;
}
