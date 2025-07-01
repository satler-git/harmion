// Topic, Connectionなどいろいろ考えれてないことばかり

pub trait Topic {
    fn value(&self) -> &str;
}

impl<T: AsRef<str>> Topic for T {
    fn value(&self) -> &str {
        self.as_ref()
    }
}

// child, parent
pub struct TopicTree(pub Vec<Box<dyn Topic>>);

// topic!("/to/file" / harmion::channel::builtin::Write)
// Write -> harmion/ ...
// parent: None
#[macro_export]
macro_rules! topic {
    ( $single:expr ) => {{
        let mut v = Vec::new();
        v.push(Box::new($single) as Box<dyn Topic>);
        TopicTree(v)
    }};
    // 再帰ケース
    ( $head:expr , $($rest:expr),+ ) => {{
        let mut tree = $crate::topic!($($rest),+).0;
        tree.push(Box::new($head) as Box<dyn Topic>);
        TopicTree(tree)
    }};
}

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

trait Connection
where
    Self: Subscriber + Receiver + Sender,
{
    type NodeInfo: Serialize + DeserializeOwned; // SDP
    type NodeID: Serialize + DeserializeOwned; // Node Ident

    type E: std::error::Error;

    fn node_info(&self) -> &(Self::NodeInfo, Self::NodeInfo);
    fn node_list(&self) -> &[(Self::NodeInfo, Self::NodeInfo)];

    async fn connect(
        &mut self,
        node: Self::NodeInfo,
        node_id: Option<Self::NodeID>,
    ) -> Result<Self::NodeID, <Self as Connection>::E>;
}
