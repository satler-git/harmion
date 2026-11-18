//! Native HTTP/TLS host adapters. Shared node semantics remain in harmion-node.
pub mod app;
pub mod iroh_peer;
mod membership_sync;
pub mod peer;
pub mod sync;
pub mod tls;

use harmion_node::{application::Host, storage::Store};
use std::sync::{Arc, Mutex};
type RoundLocks =
    std::collections::BTreeMap<harmion_domain::NodeId, std::sync::Weak<tokio::sync::Mutex<()>>>;

#[derive(Debug)]
pub enum Error {
    Node(harmion_node::Error),
    Io(std::io::Error),
    Transport(String),
    InvalidResponse,
    Timeout,
    RemoteRejected {
        update_id: harmion_domain::UpdateId,
        reason: harmion_protocol::Rejection,
    },
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}
impl From<harmion_node::Error> for Error {
    fn from(e: harmion_node::Error) -> Self {
        Self::Node(e)
    }
}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<harmion_protocol::CodecError> for Error {
    fn from(e: harmion_protocol::CodecError) -> Self {
        Self::Node(e.into())
    }
}
fn transport(error: impl std::fmt::Display) -> Error {
    Error::Transport(error.to_string())
}

pub struct SystemHost;

/// Host selection is separate from the shared synchronization state machine.
#[derive(Clone)]
pub struct NativeTransport {
    pub membership_signer: Option<Arc<harmion_node::membership_state::MembershipSigner>>,
    pub tls: Arc<tls::TlsIdentity>,
    pub iroh: Option<iroh_peer::IrohTransport>,
}
impl sync::PeerTransport for NativeTransport {
    async fn exchange(
        &self,
        local: harmion_node::storage::Identity,
        peer: &harmion_node::membership::Peer,
        command: harmion_protocol::PeerCommand,
    ) -> Result<harmion_protocol::PeerReply, Error> {
        match peer.key {
            harmion_node::membership::PeerKey::TlsCertificate(_) => {
                peer::exchange(&self.tls, local, peer, command).await
            }
            harmion_node::membership::PeerKey::Iroh(_) => {
                self.iroh
                    .as_ref()
                    .ok_or(Error::InvalidResponse)?
                    .exchange(local, peer, command)
                    .await
            }
        }
    }
}
impl Host for SystemHost {
    fn random_bytes(&mut self) -> Result<[u8; 32], harmion_node::Error> {
        let mut bytes = [0; 32];
        getrandom::fill(&mut bytes).map_err(|e| harmion_node::Error::Entropy(e.to_string()))?;
        Ok(bytes)
    }
    fn now_micros(&self) -> i64 {
        match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(time) => time.as_micros().min(i64::MAX as u128) as i64,
            Err(time) => -(time.duration().as_micros().min(i64::MAX as u128) as i64),
        }
    }
}

/// Moves DB callbacks off async network tasks; each callback holds the DB mutex
/// only for synchronous work. No lock is held across a network await.
pub struct SharedStore<S> {
    inner: Arc<Mutex<S>>,
    sync: Arc<Mutex<RoundLocks>>,
}
impl<S> Clone for SharedStore<S> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            sync: self.sync.clone(),
        }
    }
}
impl<S: Store + Send + 'static> SharedStore<S> {
    pub fn new(store: S) -> Self {
        Self {
            inner: Arc::new(Mutex::new(store)),
            sync: Arc::new(Mutex::new(RoundLocks::new())),
        }
    }
    pub(crate) fn round_lock(
        &self,
        node: harmion_domain::NodeId,
    ) -> Result<Arc<tokio::sync::Mutex<()>>, Error> {
        let mut locks = self
            .sync
            .lock()
            .map_err(|_| Error::Transport("round locks poisoned".into()))?;
        locks.retain(|_, lock| lock.strong_count() != 0);
        if let Some(lock) = locks.get(&node).and_then(std::sync::Weak::upgrade) {
            return Ok(lock);
        }
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        locks.insert(node, Arc::downgrade(&lock));
        Ok(lock)
    }
    pub async fn run<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut S) -> Result<T, harmion_node::Error> + Send + 'static,
    ) -> Result<T, Error> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut store = inner
                .lock()
                .map_err(|_| harmion_node::Error::Storage("DB worker poisoned".into()))?;
            f(&mut *store)
        })
        .await
        .map_err(transport)?
        .map_err(Error::Node)
    }
}
