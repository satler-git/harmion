//! One-process node runtime: initialization, listeners, scheduling and stop.
//!
//! Ownership is explicit. Callers construct exactly one runtime per node
//! directory and keep it for as long as any UI, bound client or work request
//! needs the core. Stopping the runtime closes the listeners and drops the
//! scheduler; the database stays durable and reopens without replay.
use crate::{
    Error, Result,
    config::{ConfigOverrides, NodeConfig},
    scheduler::{
        self, BlockReason, PolicyGate, SchedulerState, SyncControl, SyncGate, SyncTrigger,
    },
    status::{RuntimeStatus, StorageStatus, SyncStatusView},
    storage,
};
use futures_util::FutureExt;
use harmion_domain::{ClusterId, NodeId};
use harmion_local::ProtectedDir;
use harmion_net_native::{
    NativeTransport, SharedStore, SystemHost, app, iroh_peer, oauth_browser, peer, recovery, sync,
};
use harmion_node::{
    access::{Credential, Grants},
    application::Host,
    membership_state::{self, MembershipSigner},
    storage::Store,
};
use harmion_protocol::{LogEpoch, PeerBinding, PeerRegistration, hex};
use harmion_store_sqlite::SqliteStore;
use serde::Serialize;
use std::{
    fs::File,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{net::TcpListener, sync::watch, task::JoinSet};
/// Reclaim edit tokens and request receipts older than this.
pub const TOKEN_RETENTION_MICROS: i64 = 30 * 24 * 60 * 60 * 1_000_000;
pub const TOKEN_GC_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
#[derive(Clone, Debug, Serialize)]
pub struct ReadyInfo {
    pub app_endpoint: String,
    pub peer: PeerRegistration,
    pub oauth_origin: Option<String>,
    /// PEM trust root for the browser OAuth listener, when enabled.
    pub oauth_trust_root: Option<String>,
}
#[derive(Clone, Default)]
pub struct RuntimeOptions {
    pub overrides: ConfigOverrides,
    /// Additional DER trust anchors for signed-HTTPS peers (private CA).
    pub signed_https_roots: Vec<Vec<u8>>,
    /// `None` uses the host policy gate owned by this runtime.
    pub sync_gate: Option<Arc<dyn SyncGate>>,
    /// Host policy evaluated before startup. When blocked, the scheduler's first
    /// round cannot run before the host applies its policy, closing the window
    /// between `start` and the first `set_sync_policy`.
    pub sync_blocked: Option<BlockReason>,
}
#[derive(Clone, Debug, Default, Serialize)]
pub struct SyncReportSummary {
    pub peers: Vec<PeerSyncOutcome>,
    pub uploaded: u64,
    pub downloaded: u64,
}
#[derive(Clone, Debug, Serialize)]
pub struct PeerSyncOutcome {
    pub node: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uploaded: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub downloaded: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
struct Parts {
    stop: watch::Sender<bool>,
    tasks: JoinSet<Result<()>>,
    background: JoinSet<Result<()>>,
    iroh: Option<iroh_peer::IrohTransport>,
    _lock: File,
}
pub struct NodeRuntime {
    directory: ProtectedDir,
    config: NodeConfig,
    ready: ReadyInfo,
    started_at: i64,
    store: SharedStore<SqliteStore>,
    transport: Arc<NativeTransport>,
    policy: Arc<PolicyGate>,
    gate: Arc<dyn SyncGate>,
    control: Arc<SyncControl>,
    state: Arc<Mutex<SchedulerState>>,
    failures: tokio::sync::watch::Receiver<Option<String>>,
    parts: Mutex<Option<Parts>>,
}

/// Restore the host admin credential's management grants.
///
/// `admin.secret` is the host's root App credential and is never managed by the
/// UI. Older Android builds exposed that hidden app in the grant editor and let
/// the owner remove `manage_apps`/`manage_nodes` from it, after which Apps,
/// Peers and the approval screen failed with HTTP 403. Reconcile it at startup
/// so such a node recovers without losing data.
async fn reconcile_host_admin(store: &SharedStore<SqliteStore>, secret: [u8; 32]) -> Result<()> {
    store
        .run(move |store| {
            store.transaction(|tx| {
                let credential = Credential::from_bytes(secret);
                let Some(mut app) = tx.app_by_verifier(credential.verifier())? else {
                    return Err(harmion_node::Error::Unauthenticated);
                };
                if app.grants.is_administrator() && !app.revoked && app.expires_at == i64::MAX {
                    return Ok(());
                }
                tracing::warn!("restoring the host admin credential's management grants");
                app.grants = Grants::administrator();
                app.revoked = false;
                app.expires_at = i64::MAX;
                tx.update_app(&app)?;
                Ok(())
            })
        })
        .await?;
    Ok(())
}

impl NodeRuntime {
    /// Opens the node directory and starts every listener, the scheduler and the
    /// maintenance loop. The runtime takes the directory lock: a second start
    /// on the same directory fails instead of opening a second writer.
    pub async fn start(root: impl AsRef<Path>, options: RuntimeOptions) -> Result<Self> {
        let directory = storage::open_node(root)?;
        let lock = directory.lock()?;
        let mut config = storage::read_config(&directory)?;
        config.apply(&options.overrides);
        config.validate()?;
        let identity = config.identity()?;
        let tls = Arc::new(storage::tls_identity(&directory)?);
        storage::verify_database_files(&directory)?;
        let store = SharedStore::new(SqliteStore::open(
            directory.path().join(crate::config::DATABASE_FILE),
            identity,
        )?);
        reconcile_host_admin(&store, storage::admin_secret(&directory)?).await?;
        let app_listener = TcpListener::bind(config.app_listen)
            .await
            .map_err(|error| Error::Start(format!("app listener: {error}")))?;
        let mut oauth_trust_root = None;
        let oauth_listener = if let Some(address) = config.oauth_listen {
            let (tls, trust_root) = storage::oauth_tls(&directory)?;
            oauth_trust_root = Some(trust_root);
            let listener = TcpListener::bind(address)
                .await
                .map_err(|error| Error::Start(format!("oauth listener: {error}")))?;
            let actual = listener.local_addr()?;
            let origin = oauth_browser::configure(&store, actual)?;
            config.oauth_listen = Some(actual);
            Some((listener, tls, origin))
        } else {
            None
        };
        let peer_listener = if config.use_iroh {
            None
        } else {
            Some(
                TcpListener::bind(config.peer_listen)
                    .await
                    .map_err(|error| Error::Start(format!("peer listener: {error}")))?,
            )
        };
        let membership_signer = Arc::new(MembershipSigner::from_bytes(&storage::iroh_secret(
            &directory,
        )?));
        let iroh_endpoint = if config.use_iroh {
            Some(iroh_peer::IrohTransport {
                endpoint: iroh_peer::bind(
                    storage::iroh_secret(&directory)?,
                    config.relay_urls.clone(),
                    config.public_lookup,
                )
                .await
                .map_err(|error| Error::Start(format!("iroh endpoint: {error}")))?,
            })
        } else {
            None
        };
        config.app_listen = app_listener.local_addr()?;
        if let Some(listener) = &peer_listener {
            config.peer_listen = listener.local_addr()?;
        }
        let connection = harmion_protocol::ConnectionInfo {
            version: 1,
            cluster_id: config.cluster_id.clone(),
            node_id: config.node_id.clone(),
            certificate: hex(tls.certificate()),
            endpoint: config.app_listen.to_string(),
            oauth_origin: oauth_listener.as_ref().map(|(_, _, origin)| origin.clone()),
        };
        let mut registration = PeerRegistration {
            version: 3,
            signing_key: Some(hex(&membership_signer.public_key())),
            cluster_root: None,
            cluster_id: config.cluster_id.clone(),
            node_id: config.node_id.clone(),
            binding: match &iroh_endpoint {
                Some(iroh) => PeerBinding::Iroh {
                    endpoint_id: hex(iroh.endpoint.id().as_bytes()),
                    relay_url: config
                        .relay_urls
                        .as_ref()
                        .and_then(|urls| urls.first())
                        .cloned(),
                },
                None => PeerBinding::Https {
                    certificate: hex(tls.certificate()),
                    endpoint: config.peer_listen.to_string(),
                },
            },
        };
        let own_member = harmion_protocol::membership::Member {
            node: identity.node,
            signing_key: membership_signer.public_key(),
            binding: match &iroh_endpoint {
                Some(iroh) => {
                    harmion_protocol::membership::Binding::Iroh(*iroh.endpoint.id().as_bytes())
                }
                None => harmion_protocol::membership::Binding::Https(tls.certificate().to_vec()),
            },
            endpoint: match &registration.binding {
                PeerBinding::Https { endpoint, .. } => endpoint.clone(),
                PeerBinding::Iroh { relay_url, .. } => relay_url.clone().unwrap_or_default(),
                PeerBinding::SignedHttps { origin } => origin.clone(),
            },
        };
        let signer = membership_signer.clone();
        let founder = config.founder;
        store
            .run(move |s| membership_state::initialize(s, &signer, own_member, founder))
            .await?;
        registration.cluster_root = store
            .run(|s| s.transaction(|tx| Ok(tx.membership_root()?.map(|root| hex(root.bytes())))))
            .await?;
        storage::write_config(&directory, &config)?;
        directory.replace(
            crate::config::CONNECTION_FILE,
            &serde_json::to_vec(&connection)?,
        )?;
        directory.replace(
            crate::config::PEER_FILE,
            &serde_json::to_vec(&registration)?,
        )?;
        let ready = ReadyInfo {
            app_endpoint: config.app_listen.to_string(),
            peer: registration.clone(),
            oauth_origin: connection.oauth_origin.clone(),
            oauth_trust_root,
        };
        let (stop, receiver) = watch::channel(false);
        let (failure_tx, failure_rx) = watch::channel::<Option<String>>(None);
        let mut tasks: JoinSet<Result<()>> = JoinSet::new();
        if let Some((listener, oauth_tls, _origin)) = oauth_listener {
            spawn_server(&mut tasks, &failure_tx, &receiver, {
                let store = store.clone();
                let stop = receiver.clone();
                async move {
                    oauth_browser::serve(listener, store, oauth_tls, stop)
                        .await
                        .map_err(Error::Transport)
                }
            });
        }
        let transport = Arc::new(NativeTransport {
            membership_signer: Some(membership_signer),
            tls: tls.clone(),
            iroh: iroh_endpoint.clone(),
            signed_https_roots: options.signed_https_roots,
        });
        spawn_server(&mut tasks, &failure_tx, &receiver, {
            let store = store.clone();
            let transport = transport.clone();
            let registration = registration.clone();
            let stop = receiver.clone();
            async move {
                app::serve_with_transport(app_listener, store, transport, registration, stop)
                    .await
                    .map_err(Error::Transport)
            }
        });
        if let Some(listener) = peer_listener {
            spawn_server(&mut tasks, &failure_tx, &receiver, {
                let store = store.clone();
                let stop = receiver.clone();
                async move {
                    peer::serve(listener, store, tls, stop)
                        .await
                        .map_err(Error::Transport)
                }
            });
        }
        if let Some(iroh) = &iroh_endpoint {
            spawn_server(&mut tasks, &failure_tx, &receiver, {
                let store = store.clone();
                let stop = receiver.clone();
                let endpoint = iroh.endpoint.clone();
                async move {
                    iroh_peer::serve(endpoint, store, stop)
                        .await
                        .map_err(Error::Transport)
                }
            });
        }
        let policy = Arc::new(PolicyGate::new());
        if let Some(reason) = options.sync_blocked.clone() {
            policy.set(false, Some(reason));
        }
        let control = Arc::new(SyncControl::default());
        let seconds = config.sync_seconds;
        let state = Arc::new(Mutex::new(SchedulerState {
            enabled: config.sync_seconds != 0,
            permitted: options.sync_blocked.is_none(),
            blocked: options.sync_blocked.clone(),
            ..SchedulerState::default()
        }));
        let mut background: JoinSet<Result<()>> = JoinSet::new();
        // Edit-token maintenance. Detached from the HTTP services: a failure
        // must not stop them.
        background.spawn({
            let store = store.clone();
            async move {
                let mut ticker = tokio::time::interval(TOKEN_GC_INTERVAL);
                loop {
                    ticker.tick().await;
                    let cutoff = SystemHost.now_micros() - TOKEN_RETENTION_MICROS;
                    match store.run(move |s| s.gc_tokens(cutoff)).await {
                        Ok(deleted) if deleted > 0 => {
                            tracing::info!(deleted, "reclaimed expired edit tokens");
                        }
                        Ok(_) => {}
                        Err(error) => tracing::warn!(%error, "edit token cleanup failed"),
                    }
                }
            }
        });
        let gate: Arc<dyn SyncGate> = match &options.sync_gate {
            Some(gate) => gate.clone(),
            None => policy.clone(),
        };
        if config.sync_seconds != 0 {
            let (recovery_tx, recovery_rx) = watch::channel(None);
            background.spawn({
                let endpoint = iroh_endpoint.as_ref().map(|iroh| iroh.endpoint.clone());
                async move {
                    recovery::monitor(recovery_tx, endpoint).await;
                    Ok(())
                }
            });
            let gate = gate.clone();
            background.spawn({
                let store = store.clone();
                let transport = transport.clone();
                let control = control.clone();
                let state = state.clone();
                let stop = stop.subscribe();
                async move {
                    scheduler::Scheduler {
                        store,
                        transport,
                        seconds,
                        gate,
                        control,
                        state,
                        recovery: recovery_rx,
                        stop,
                    }
                    .run()
                    .await;
                    Ok(())
                }
            });
        }
        Ok(Self {
            directory,
            config,
            ready,
            started_at: SystemHost.now_micros(),
            store,
            transport,
            policy,
            gate,
            control,
            state,
            failures: failure_rx,
            parts: Mutex::new(Some(Parts {
                stop,
                tasks,
                background,
                iroh: iroh_endpoint,
                _lock: lock,
            })),
        })
    }
    pub fn ready(&self) -> &ReadyInfo {
        &self.ready
    }
    pub fn config(&self) -> &NodeConfig {
        &self.config
    }
    pub fn root(&self) -> &Path {
        self.directory.path()
    }
    pub fn policy_gate(&self) -> &Arc<PolicyGate> {
        &self.policy
    }
    /// Applies a host policy decision. A blocked gate stops new rounds and aborts
    /// in-flight ones; the durable cursor is the resume point.
    pub fn set_sync_policy(&self, permitted: bool, reason: Option<BlockReason>) {
        self.policy.set(permitted, reason);
        self.control.request(SyncTrigger::NetworkReturn);
    }
    /// Non-blocking request for an early scheduler pass. The trigger is
    /// diagnostic; the gate still decides whether a round may run.
    pub fn request_sync(&self, trigger: SyncTrigger) {
        self.control.request(trigger);
    }
    pub fn sync_state(&self) -> SchedulerState {
        self.state.lock().expect("scheduler state").clone()
    }
    /// Manual sync. The caller is expected to have obtained explicit user
    /// confirmation; this is the one path that ignores the *scheduling* policy.
    /// Membership, authorization and transport limits still apply.
    pub async fn sync_now(&self) -> Result<SyncReportSummary> {
        self.sync_peers(SyncTrigger::Manual).await
    }
    /// Sync that obeys the host policy. Background jobs use this so a periodic
    /// wake-up cannot bypass Wi-Fi, metering, roaming or battery rules.
    pub async fn sync_gated(&self) -> Result<SyncReportSummary> {
        if let Err(reason) = self.gate.permit() {
            return Err(Error::SyncBlocked(reason.code));
        }
        self.sync_peers(SyncTrigger::Periodic).await
    }
    async fn sync_peers(&self, trigger: SyncTrigger) -> Result<SyncReportSummary> {
        if self.is_stopped() {
            return Err(Error::Stopped);
        }
        let peers = self
            .store
            .run(|s| {
                s.transaction(|tx| {
                    Ok(tx
                        .peers()?
                        .into_iter()
                        .map(|peer| peer.node)
                        .collect::<Vec<_>>())
                })
            })
            .await?;
        self.control.request(trigger);
        let mut summary = SyncReportSummary::default();
        for node in peers {
            if let Err(reason) = self.gate.permit() {
                return Err(Error::SyncBlocked(reason.code));
            }
            let result = sync::sync_once(&self.store, self.transport.as_ref(), node).await;
            match result {
                Ok(report) => {
                    summary.uploaded = summary.uploaded.saturating_add(report.uploaded);
                    summary.downloaded = summary.downloaded.saturating_add(report.downloaded);
                    summary.peers.push(PeerSyncOutcome {
                        node: hex(&node.0),
                        uploaded: Some(report.uploaded),
                        downloaded: Some(report.downloaded),
                        error: None,
                    });
                }
                Err(harmion_net_native::Error::Node(
                    harmion_node::Error::PeerRetired | harmion_node::Error::MembershipConflict,
                )) => {
                    summary.peers.push(PeerSyncOutcome {
                        node: hex(&node.0),
                        uploaded: None,
                        downloaded: None,
                        error: Some("membership".into()),
                    });
                }
                Err(error) => {
                    summary.peers.push(PeerSyncOutcome {
                        node: hex(&node.0),
                        uploaded: None,
                        downloaded: None,
                        error: Some(scheduler::failure_code(&error).to_owned()),
                    });
                }
            }
        }
        Ok(summary)
    }
    /// Full status for the management UI. Reads membership and peers synchronously
    /// through the store mutex; no network I/O.
    pub async fn status(&self) -> Result<RuntimeStatus> {
        let state = self.sync_state();
        let peers = self
            .store
            .run(|s| {
                s.transaction(|tx| Ok(tx.peers()?.into_iter().map(|p| p.node).collect::<Vec<_>>()))
            })
            .await?;
        let membership = self
            .store
            .run(|s| {
                s.transaction(|tx| {
                    let state = membership_state::state(tx)?;
                    Ok(state
                        .map(|membership| {
                            if membership
                                .nodes
                                .get(&tx.identity().node)
                                .is_some_and(|status| {
                                    matches!(status, membership_state::Status::Active(_))
                                })
                            {
                                "active"
                            } else {
                                "unconfirmed"
                            }
                        })
                        .unwrap_or("unconfirmed")
                        .to_owned())
                })
            })
            .await?;
        let database_bytes = std::fs::metadata(self.directory.path().join("node.db"))
            .map(|meta| meta.len())
            .unwrap_or(0);
        Ok(RuntimeStatus {
            running: !self.is_stopped(),
            started_at: self.started_at,
            cluster_id: self.config.cluster_id.clone(),
            node_id: self.config.node_id.clone(),
            founder: self.config.founder,
            app_endpoint: self.config.app_listen.to_string(),
            peer: self.ready.peer.clone(),
            oauth_origin: self.ready.oauth_origin.clone(),
            membership,
            peers: peers.len(),
            sync: state.view(),
            storage: StorageStatus {
                database_bytes,
                free_bytes: None,
            },
        })
    }
    pub fn sync_status_view(&self) -> SyncStatusView {
        self.sync_state().view()
    }
    pub fn is_stopped(&self) -> bool {
        self.parts
            .lock()
            .map(|parts| parts.is_none())
            .unwrap_or(true)
    }
    /// Resolves when a listener task fails or stops before shutdown. A daemon
    /// uses this instead of monitoring a `JoinSet` itself; on shutdown the
    /// future is simply dropped.
    pub async fn failure(&self) -> String {
        let mut failures = self.failures.clone();
        loop {
            if let Some(error) = failures.borrow_and_update().clone() {
                return error;
            }
            if failures.changed().await.is_err() {
                // The runtime is shutting down; callers select on shutdown too.
                std::future::pending::<()>().await;
            }
        }
    }
    /// Stops listeners and background tasks. Idempotent; durable state is
    /// already committed, so no flush is required for correctness.
    pub async fn shutdown(&self) -> Result<()> {
        let Some(parts) = self.parts.lock().expect("runtime parts").take() else {
            return Ok(());
        };
        let Parts {
            stop,
            mut tasks,
            mut background,
            iroh,
            _lock,
        } = parts;
        let _ = stop.send(true);
        background.abort_all();
        while background.join_next().await.is_some() {}
        let mut first_error = None;
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    first_error.get_or_insert(error);
                }
                Err(error) => {
                    first_error.get_or_insert(Error::Start(error.to_string()));
                }
            }
        }
        if let Some(iroh) = iroh {
            iroh.endpoint.close().await;
        }
        drop(_lock);
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}
/// Identity of an initialized node directory, readable before the runtime starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IdentifiedCluster {
    pub cluster: ClusterId,
    pub node: NodeId,
}
impl NodeRuntime {
    /// Reads the persisted identity without opening a database or listeners.
    pub fn identifiers(root: impl AsRef<Path>) -> Result<IdentifiedCluster> {
        let directory = storage::open_node(root)?;
        let config = storage::read_config(&directory)?;
        let identity = config.identity()?;
        Ok(IdentifiedCluster {
            cluster: identity.cluster,
            node: identity.node,
        })
    }
}
/// Cluster, node and log epoch of an initialized node directory.
pub fn cluster_identity(root: impl AsRef<Path>) -> Result<(ClusterId, NodeId, LogEpoch)> {
    let directory = storage::open_node(root)?;
    let config = storage::read_config(&directory)?;
    let identity = config.identity()?;
    Ok((identity.cluster, identity.node, identity.epoch))
}
/// Path for the node directory under a host-provided base (Android files dir).
pub fn node_directory(base: impl AsRef<Path>) -> PathBuf {
    base.as_ref().join("node")
}
/// Spawns a listener task and records why it ended. A clean return without a
/// shutdown request is reported as an unexpected stop, matching the daemon's
/// historical behavior.
fn spawn_server(
    tasks: &mut JoinSet<Result<()>>,
    failures: &watch::Sender<Option<String>>,
    stop: &watch::Receiver<bool>,
    future: impl std::future::Future<Output = Result<()>> + Send + 'static,
) {
    let failures = failures.clone();
    let stop = stop.clone();
    tasks.spawn(async move {
        // A panic in a listener must not leave the process "ready" while the
        // service is dead: report it through the same failure channel.
        match std::panic::AssertUnwindSafe(future).catch_unwind().await {
            Ok(Ok(())) if !*stop.borrow() => {
                let _ = failures.send(Some("service stopped unexpectedly".to_owned()));
                Ok(())
            }
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => {
                let _ = failures.send(Some(error.to_string()));
                Err(error)
            }
            Err(_) => {
                let _ = failures.send(Some("service panicked".to_owned()));
                Err(Error::Start("service panicked".to_owned()))
            }
        }
    });
}
/// Result of enrolling the first owner key. The recovery code is returned once
/// and never persisted in plaintext.
#[derive(Clone)]
pub struct OwnerEnrollment {
    pub fingerprint: String,
    pub label: String,
    pub recovery_code: String,
}
impl std::fmt::Debug for OwnerEnrollment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnerEnrollment")
            .field("fingerprint", &self.fingerprint)
            .field("label", &self.label)
            .field("recovery_code", &"<redacted>")
            .finish()
    }
}
impl NodeRuntime {
    /// Enrolls the first owner credential through the running store.
    ///
    /// Enrollment is host-only and requires the node to be running: the runtime
    /// holds the directory lock and the single SQLite writer, so a second opener
    /// is refused by design.
    pub async fn enroll_owner(&self, public_key: &str, label: &str) -> Result<OwnerEnrollment> {
        let public_key = public_key.to_owned();
        let label = label.to_owned();
        // Owner enrollment is a host-only operation, but it still requires the
        // administrative credential: a restored node has zero credentials, and
        // an embedded host must not bypass the same check the daemon performs.
        let secret = storage::admin_secret(&self.directory)?;
        self.store
            .run(move |store| {
                let mut host = SystemHost;
                let now = host.now_micros();
                let credential = harmion_node::access::Credential::from_bytes(secret);
                if !harmion_node::access::permissions(store, &credential, now)?.is_administrator() {
                    return Err(harmion_node::Error::Forbidden.into());
                }
                let recovery = host.random_bytes()?;
                let id = host.random_bytes()?;
                let enrolled = store.transaction(|tx| {
                    harmion_node::owner::enroll_initial(tx, &public_key, &label, &recovery, id, now)
                })?;
                Ok(OwnerEnrollment {
                    fingerprint: enrolled.fingerprint,
                    label: enrolled.label,
                    recovery_code: harmion_protocol::hex(&recovery),
                })
            })
            .await
            .map_err(|error| match error {
                harmion_net_native::Error::Node(error) => Error::Node(error),
                error => Error::Transport(error),
            })
    }
}
