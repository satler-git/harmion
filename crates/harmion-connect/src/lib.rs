// Topic, Connectionなどいろいろ考えれてないことばかり

pub mod topic;
mod verify;
pub mod webrtc;

use std::marker::PhantomData;
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

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

const NONCE_LEN: usize = 12;
type Nonce = [u8; NONCE_LEN];

#[derive(Copy, Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct PeerIndex(VerifyingKey);

impl<T: Into<VerifyingKey>> From<T> for PeerIndex {
    fn from(value: T) -> Self {
        Self(value.into())
    }
}

impl PeerIndex {
    fn inner(self) -> VerifyingKey {
        self.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Message {
    pub content: Vec<u8>,
    pub timestamp: u64,

    pub origin: PeerIndex,
    pub sig: Signature,

    pub nonce: Nonce,
}

use rand::{rngs::ThreadRng, RngCore}; // TODO: Reseed?

impl Message {
    pub fn new(content: Vec<u8>, key: &SigningKey) -> Self {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("System time is before the UNIX epoch")
            .as_secs();

        let mut data_to_sign = Vec::with_capacity(
            content.len() + std::mem::size_of::<u64>() + std::mem::size_of::<Nonce>(),
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
            .verify_strict(
                &{
                    let mut data = Vec::with_capacity(
                        self.content.len()
                            + std::mem::size_of::<u64>()
                            + std::mem::size_of::<Nonce>(),
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

    pub nonce: Nonce,

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

trait Layer<T, R, C> {
    type Connection: Connection<T, R>;

    async fn layer(&self, inner: C) -> Self::Connection;

    fn stack<O, T2, R2>(self, outer: O) -> Stack<O, Self, T, R, C, T2, R2>
    where
        O: Layer<T2, R2, Self::Connection>,
        Self: Sized,
    {
        Stack {
            outer,
            inner: self,
            _phantom: PhantomData,
        }
    }
}

struct Stack<O, I, T, R, C, T2, R2>
where
    I: Layer<T, R, C>,
    O: Layer<T2, R2, I::Connection>,
    I::Connection: Connection<T, R>,
{
    outer: O,
    inner: I,
    _phantom: PhantomData<(T, R, C, T2, R2)>,
}

impl<O, I, T, R, C, T2, R2> Layer<T2, R2, C> for Stack<O, I, T, R, C, T2, R2>
where
    I: Layer<T, R, C>,
    O: Layer<T2, R2, I::Connection>,
    I::Connection: Connection<T, R>,
{
    type Connection = O::Connection;

    async fn layer(&self, inner: C) -> Self::Connection {
        self.outer.layer(self.inner.layer(inner).await).await
    }
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

struct ToBytes; // TODO: お遊びだから消すかも

impl<C> Layer<Message, Vec<u8>, C> for ToBytes
where
    C: Connection<Message, Message>,
{
    type Connection = ToBytesConn<C>;

    async fn layer(&self, inner: C) -> Self::Connection {
        ToBytesConn(inner)
    }
}

struct ToBytesConn<C>(C)
where
    C: Connection<Message, Message>;

impl<C> Connection<Message, Vec<u8>> for ToBytesConn<C>
where
    C: Connection<Message, Message>,
{
    type Error = C::Error;

    async fn send(&mut self, value: Message) -> Result<(), Self::Error> {
        self.0.send(value).await
    }

    async fn recv(&mut self) -> Result<Option<Vec<u8>>, Self::Error> {
        if let Some(message) = self.0.recv().await? {
            Ok(Some(message.content))
        } else {
            Ok(None)
        }
    }
}

use std::ops::ControlFlow;

trait Handshake<T, R> {
    // 最初にstateがないときでもmessageだけはある、offerer/responderを明確に区別するのも考えたけどめんどい
    type State;
    type Error;

    type Return;

    fn start(&mut self)
        -> Result<(ControlFlow<Self::Return, Self::State>, Option<T>), Self::Error>;

    fn next_state(
        &mut self,
        state: Self::State,
        message: R,
    ) -> Result<(ControlFlow<Self::Return, Self::State>, Option<T>), Self::Error>;

    async fn run<C>(
        &mut self,
        conn: &mut C,
    ) -> Result<Self::Return, HandshakeRunnerError<Self::Error, C::Error>>
    where
        C: Connection<T, R>,
    {
        let (mut state, mut message) = self.start().map_err(HandshakeRunnerError::Handshake)?;

        loop {
            match state {
                ControlFlow::Continue(prev_state) => {
                    if let Some(m) = message.take() {
                        conn.send(m).await.map_err(HandshakeRunnerError::Conn)?;
                    }

                    if let Some(m) = conn.recv().await.map_err(HandshakeRunnerError::Conn)? {
                        (state, message) = self
                            .next_state(prev_state, m)
                            .map_err(HandshakeRunnerError::Handshake)?;
                    } else {
                        return Err(HandshakeRunnerError::ConnClosed);
                    }
                }
                ControlFlow::Break(r) => return Ok(r),
            }
        }
    }
}

enum HandshakeRunnerError<H, C> {
    Conn(C),
    Handshake(H),
    ConnClosed,
}

// Encrypt<T: Connection<Message, Message>>: Connection<T, MessageT<Decrypted<T>>>
// or Messageにもっと組み込む
// Handshakeする

#[cfg(test)]
mod tests {
    use sha2::digest::crypto_common::rand_core::RngCore;

    use crate::Message;

    pub(crate) fn init_log() {
        let _ = tracing_subscriber::fmt()
            // .with_max_level(tracing::Level::DEBUG)
            .with_max_level(tracing::Level::ERROR)
            // .with_max_level(tracing::Level::INFO)
            .with_file(true)
            .with_line_number(true)
            .try_init();
    }

    #[test]
    fn signature() -> Result<(), Box<dyn std::error::Error>> {
        let mut rng = ed25519_dalek::ed25519::signature::rand_core::OsRng;
        let key = ed25519_dalek::SigningKey::generate(&mut rng);

        let msg = Message::from(&rng.next_u64(), &key)?;

        assert!(msg.verify());

        Ok(())
    }
}
