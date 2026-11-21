use super::*;
use harmion_domain::ClusterId;
use harmion_node::{membership::PeerKey, storage::Transaction};
use harmion_store_sqlite::SqliteStore;
use std::sync::{Arc, Condvar, Mutex};
use tokio::{
    sync::Notify,
    time::{Duration, timeout},
};

#[derive(Default)]
struct Gate {
    entered: Notify,
    released: Mutex<bool>,
    changed: Condvar,
}
impl Gate {
    fn wait(&self) {
        self.entered.notify_one();
        let mut released = self.released.lock().unwrap();
        while !*released {
            released = self.changed.wait(released).unwrap();
        }
    }
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.changed.notify_all();
    }
}

// Release blocking workers even if a test assertion panics.
struct ReleaseOnDrop(Arc<Gate>);
impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.0.release();
    }
}

struct PausedStore {
    inner: SqliteStore,
    transactions: usize,
    pause_at: usize,
    gate: Arc<Gate>,
}
impl Store for PausedStore {
    fn transaction<T>(
        &mut self,
        f: impl FnOnce(&mut dyn Transaction) -> Result<T, harmion_node::Error>,
    ) -> Result<T, harmion_node::Error> {
        self.transactions += 1;
        if self.transactions == self.pause_at {
            self.gate.wait();
        }
        self.inner.transaction(f)
    }
}

struct EmptyPeer {
    succeeds: bool,
}
impl PeerTransport for EmptyPeer {
    async fn exchange(
        &self,
        local: Identity,
        _: &Peer,
        command: PeerCommand,
    ) -> Result<PeerReply, Error> {
        if !self.succeeds {
            return Err(Error::Timeout);
        }
        match command {
            PeerCommand::Status => Ok(PeerReply::Status {
                epoch: LogEpoch([3; 32]),
                high: 0,
            }),
            PeerCommand::Cursor { epoch } => {
                assert_eq!(epoch, local.epoch);
                Ok(PeerReply::Cursor { epoch, through: 0 })
            }
            _ => panic!("an empty round needs only status and cursor"),
        }
    }
}

async fn cancelled_terminal_write_retains_round_lock(initial_success: bool) {
    let identity = Identity {
        cluster: ClusterId([1; 32]),
        node: NodeId([2; 32]),
        epoch: LogEpoch([2; 32]),
    };
    let node = NodeId([3; 32]);
    let mut inner = SqliteStore::open(":memory:", identity).unwrap();
    inner
        .transaction(|tx| {
            tx.put_peer(&Peer {
                node,
                key: PeerKey::Iroh([3; 32]),
                endpoint: String::new(),
                retired: false,
            })
        })
        .unwrap();
    let gate = Arc::new(Gate::default());
    let _release_on_panic = ReleaseOnDrop(gate.clone());
    let store = SharedStore::new(PausedStore {
        inner,
        transactions: 0,
        // Control, local high, then either failure or download cursor + success.
        pause_at: if initial_success { 4 } else { 3 },
        gate: gate.clone(),
    });
    let first_store = store.clone();
    let first = tokio::spawn(async move {
        sync_once(
            &first_store,
            &EmptyPeer {
                succeeds: initial_success,
            },
            node,
        )
        .await
    });
    timeout(Duration::from_secs(5), gate.entered.notified())
        .await
        .expect("terminal persistence did not reach the gate");
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());

    // The caller is gone, but its blocking terminal write still owns the lock.
    // Without the fix this succeeds, admitting a potentially overtaking retry.
    let round_lock = store.round_lock(node).unwrap();
    assert!(round_lock.try_lock().is_err());
    let retry_store = store.clone();
    let retry = tokio::spawn(async move {
        sync_once(
            &retry_store,
            &EmptyPeer {
                succeeds: !initial_success,
            },
            node,
        )
        .await
    });
    gate.release();
    let result = timeout(Duration::from_secs(5), retry)
        .await
        .expect("retry did not finish after releasing the old write")
        .unwrap();
    if initial_success {
        assert!(matches!(result, Err(Error::Timeout)));
    } else {
        assert!(result.is_ok());
    }
    store
        .run(move |s| {
            s.transaction(|tx| {
                assert!(tx.sync_observation(node)?.is_some());
                let failure = tx.sync_failure(node)?;
                if initial_success {
                    assert_eq!(failure.unwrap().code, "timeout");
                } else {
                    assert!(failure.is_none());
                }
                Ok(())
            })
        })
        .await
        .unwrap();
    assert!(round_lock.try_lock().is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_success_write_cannot_clear_a_newer_failure() {
    cancelled_terminal_write_retains_round_lock(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_failure_write_cannot_overwrite_a_newer_success() {
    cancelled_terminal_write_retains_round_lock(false).await;
}
