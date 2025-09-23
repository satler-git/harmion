use crate::{Connection, Layer, Message, Nonce, PeerIndex};
use std::time::Duration;

#[derive(Clone)]
pub(crate) struct Cache(moka::future::Cache<(PeerIndex, Nonce), ()>);

const TTL_SECS: u64 = 10 * 60;
const MAX_CAPACITY: u64 = 10_000;

impl Default for Cache {
    fn default() -> Self {
        Self::new(MAX_CAPACITY, Duration::from_secs(TTL_SECS))
    }
}

impl Cache {
    pub(crate) fn new(capacity: u64, ttl: Duration) -> Self {
        Self(
            moka::future::Cache::builder()
                .max_capacity(capacity)
                .time_to_live(ttl)
                .build(),
        )
    }
}

pub(crate) struct VerifyLayer(Cache);

impl VerifyLayer {
    pub(crate) fn new(cache: Cache) -> Self {
        Self(cache)
    }
}

impl<C> Layer<Message, Message, C> for VerifyLayer
where
    C: Connection<Message, Message>,
{
    type Connection = VerifiedConnection<C>;

    async fn layer(&self, inner: C) -> Self::Connection {
        VerifiedConnection {
            inner,
            cache: self.0.clone(),
        }
    }
}

pub(crate) struct VerifiedConnection<C>
where
    C: Connection<Message, Message>,
{
    cache: Cache,
    inner: C,
}

impl<C> Connection<Message, Message> for VerifiedConnection<C>
where
    C: Connection<Message, Message>,
{
    type Error = VerificationError<C::Error>;

    async fn send(&mut self, value: Message) -> Result<(), Self::Error> {
        self.inner
            .send(value)
            .await
            .map_err(VerificationError::Inner)
    }

    async fn recv(&mut self) -> Result<Option<Message>, Self::Error> {
        let message = self.inner.recv().await.map_err(VerificationError::Inner)?;

        if let Some(message) = message {
            if !message.verify() {
                Err(VerificationError::Sign)
            } else if !self
                .cache
                .0
                .entry((message.origin, message.nonce))
                .or_default()
                .await
                .is_fresh()
            {
                Err(VerificationError::Replay)
            } else {
                Ok(Some(message))
            }
        } else {
            Ok(None)
        }
    }
}

use thiserror::Error;

#[derive(Debug, Error)]
pub(crate) enum VerificationError<E> {
    #[error(transparent)]
    Inner(#[from] E),
    #[error("failed to trust the message because of unmatched sign")]
    Sign,
    #[error("failed to trust the message because the message is replayed")]
    Replay,
}
