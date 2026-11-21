//! Native HTTP/TLS host adapters. Shared node semantics remain in harmion-node.
pub mod app;
mod export;
pub mod iroh_peer;
mod membership_sync;
pub mod oauth_browser;
pub mod peer;
pub mod peer_v2;
pub mod recovery;
mod subscriptions;
pub mod sync;
pub mod tls;

use harmion_node::{application::Host, storage::Store};
use std::sync::{Arc, Mutex};
type RoundLocks =
    std::collections::BTreeMap<harmion_domain::NodeId, std::sync::Weak<tokio::sync::Mutex<()>>>;

#[derive(Debug, thiserror::Error)]
#[error("{self:?}")]
pub enum Error {
    Node(#[from] harmion_node::Error),
    Io(#[from] std::io::Error),
    Transport(String),
    /// The peer's signed-HTTPS replica is at its provisional challenge/rate
    /// capacity; retrying after a delay is appropriate.
    Capacity,
    InvalidResponse,
    Timeout,
    RemoteRejected {
        update_id: harmion_domain::UpdateId,
        reason: harmion_protocol::Rejection,
    },
}
impl From<harmion_protocol::CodecError> for Error {
    fn from(e: harmion_protocol::CodecError) -> Self {
        Self::Node(e.into())
    }
}
/// Record transport failures without logging request headers or bodies.
fn trace_connection(
    service: &'static str,
    result: Result<Result<(), Error>, tokio::time::error::Elapsed>,
) {
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::debug!(service, %error, "connection failed"),
        Err(_) => tracing::debug!(service, "connection timed out"),
    }
}

fn transport(error: impl std::fmt::Display) -> Error {
    Error::Transport(error.to_string())
}

impl<S: Store + harmion_node::https_peer::ChallengeStore + Send + 'static> SharedStore<S> {
    /// Runs one synchronous challenge-store transaction off the async runtime.
    pub async fn run_challenge<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut S) -> Result<T, harmion_node::Error> + Send + 'static,
    ) -> Result<T, Error> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.run_blocking(f))
            .await
            .map_err(transport)?
            .map_err(Error::Node)
    }
}

pub struct SystemHost;

/// Host selection is separate from the shared synchronization state machine.
#[derive(Clone)]
pub struct NativeTransport {
    pub membership_signer: Option<Arc<harmion_node::membership_state::MembershipSigner>>,
    pub tls: Arc<tls::TlsIdentity>,
    pub iroh: Option<iroh_peer::IrohTransport>,
    /// Owner-supplied additional WebPKI trust anchors for signed-HTTPS targets
    /// (for example a private CA). Empty means the standard public roots.
    pub signed_https_roots: Vec<Vec<u8>>,
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
            harmion_node::membership::PeerKey::SignedHttps(_) => {
                let signer = self
                    .membership_signer
                    .clone()
                    .ok_or(Error::InvalidResponse)?;
                let client = peer_v2::SignedHttpsClient::new(
                    peer.endpoint.clone(),
                    signer,
                    &self.signed_https_roots,
                )?;
                client.exchange(local, peer, command).await
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
    local_save: Arc<tokio::sync::Notify>,
    changes: tokio::sync::watch::Sender<()>,
    oauth: Arc<std::sync::OnceLock<Arc<oauth_browser::Service>>>,
}
impl<S> Clone for SharedStore<S> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            sync: self.sync.clone(),
            local_save: self.local_save.clone(),
            changes: self.changes.clone(),
            oauth: self.oauth.clone(),
        }
    }
}
impl<S: Store + Send + 'static> SharedStore<S> {
    pub fn new(store: S) -> Self {
        Self {
            inner: Arc::new(Mutex::new(store)),
            sync: Arc::new(Mutex::new(RoundLocks::new())),
            local_save: Arc::new(tokio::sync::Notify::new()),
            changes: tokio::sync::watch::Sender::new(()),
            oauth: Arc::new(std::sync::OnceLock::new()),
        }
    }
    /// Wait for a committed App update. The single sync driver can coalesce saves.
    pub async fn local_save_notified(&self) {
        self.local_save.notified().await;
    }

    pub(crate) async fn run_notifying<T: Send + 'static>(
        &self,
        changed: bool,
        f: impl FnOnce(&mut S) -> Result<T, harmion_node::Error> + Send + 'static,
    ) -> Result<T, Error> {
        let changes = self.changes.clone();
        self.run(move |store| {
            let value = f(store)?;
            if changed {
                changes.send_replace(());
            }
            Ok(value)
        })
        .await
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
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.run_blocking(f))
            .await
            .map_err(transport)?
            .map_err(Error::Node)
    }

    pub(crate) fn run_blocking<T>(
        &self,
        f: impl FnOnce(&mut S) -> Result<T, harmion_node::Error>,
    ) -> Result<T, harmion_node::Error> {
        let mut store = self
            .inner
            .lock()
            .map_err(|_| harmion_node::Error::Storage("DB worker poisoned".into()))?;
        f(&mut *store)
    }
}
