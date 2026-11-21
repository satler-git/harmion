use harmion_domain::*;
use harmion_net_native::{
    SharedStore, SystemHost, peer,
    tls::{SERVER_NAME, TlsIdentity},
};
use harmion_node::{
    access::{Credential, Filter, Grants},
    application::{self as app, Host},
    membership::{self, Peer},
    replication,
    storage::{Identity, Store, read_graph},
};
use harmion_protocol::*;
use harmion_store_sqlite::SqliteStore;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::{net::TcpListener, sync::watch};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "harmion-tls-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
struct Node {
    identity: Identity,
    store: SharedStore<SqliteStore>,
    tls: Arc<TlsIdentity>,
    admin: Credential,
    endpoint: String,
    stop: watch::Sender<bool>,
    task: tokio::task::JoinHandle<Result<(), harmion_net_native::Error>>,
}
impl Node {
    async fn start(dir: &Directory, n: u8) -> Self {
        let identity = Identity {
            cluster: ClusterId([1; 32]),
            node: NodeId([n; 32]),
            epoch: LogEpoch([n; 32]),
        };
        let mut db = SqliteStore::open(dir.0.join(format!("{n}.db")), identity).unwrap();
        let (_, admin) = app::bootstrap_admin(&mut db, &mut SystemHost, i64::MAX).unwrap();
        let store = SharedStore::new(db);
        let tls = Arc::new(TlsIdentity::generate().unwrap());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = listener.local_addr().unwrap().to_string();
        let (stop, receiver) = watch::channel(false);
        let task = tokio::spawn(peer::serve(listener, store.clone(), tls.clone(), receiver));
        Self {
            identity,
            store,
            tls,
            admin,
            endpoint,
            stop,
            task,
        }
    }
    fn binding(&self) -> Peer {
        Peer {
            node: self.identity.node,
            key: membership::PeerKey::TlsCertificate(self.tls.certificate().to_vec()),
            endpoint: self.endpoint.clone(),
            retired: false,
        }
    }
    async fn approve(&self, other: &Node) {
        peer::approve(
            &self.store,
            self.admin.clone(),
            SystemHost.now_micros(),
            self.identity.cluster,
            other.binding(),
        )
        .await
        .unwrap();
    }
    async fn save(&self, n: u8, command: Command) -> app::SaveResult {
        let credential = self.admin.clone();
        self.store
            .run(move |s| {
                app::save(
                    s,
                    &mut SystemHost,
                    &credential,
                    &SaveRequest {
                        request_id: RequestId([n; 32]),
                        content_id: ContentId([1; 32]),
                        command,
                    },
                )
            })
            .await
            .unwrap()
    }
    async fn get(&self) -> app::ReadView {
        let credential = self.admin.clone();
        self.store
            .run(move |s| app::get(s, &mut SystemHost, &credential, ContentId([1; 32])))
            .await
            .unwrap()
    }
    async fn snapshot(&self) -> ContentSnapshot {
        self.store
            .run(|s| s.transaction(|tx| Ok(read_graph(tx)?.snapshot(ContentId([1; 32])).unwrap())))
            .await
            .unwrap()
    }
    async fn count(&self) -> usize {
        self.store
            .run(|s| s.transaction(|tx| Ok(tx.entries()?.len())))
            .await
            .unwrap()
    }
    async fn finish(self) {
        self.stop.send(true).unwrap();
        self.task.await.unwrap().unwrap();
    }
}
fn fields(text: &str) -> BTreeMap<Field, Value> {
    BTreeMap::from([(Field::Body, Value::Body(text.into()))])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn peer_push_and_pull_wake_an_app_subscription() {
    use harmion_client::{Client, SubscriptionNotification as Notice};
    use std::time::Duration;

    let dir = Directory::new();
    let a = Node::start(&dir, 1).await;
    let b = Node::start(&dir, 2).await;
    a.approve(&b).await;
    b.approve(&a).await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let app_endpoint = listener.local_addr().unwrap();
    let client = Client::new(
        ConnectionInfo {
            version: 1,
            cluster_id: hex(&b.identity.cluster.0),
            node_id: hex(&b.identity.node.0),
            certificate: hex(b.tls.certificate()),
            endpoint: app_endpoint.to_string(),
            oauth_origin: None,
        },
        *b.admin.expose_bytes(),
    )
    .unwrap();
    let registration = PeerRegistration {
        version: 2,
        signing_key: None,
        cluster_root: None,
        cluster_id: hex(&b.identity.cluster.0),
        node_id: hex(&b.identity.node.0),
        binding: PeerBinding::Https {
            certificate: hex(b.tls.certificate()),
            endpoint: b.endpoint.clone(),
        },
    };
    let (stop, receiver) = watch::channel(false);
    let app_task = tokio::spawn(harmion_net_native::app::serve(
        listener,
        b.store.clone(),
        b.tls.clone(),
        registration.clone(),
        receiver,
    ));
    let id = ContentId([1; 32]);
    let mut subscription = client
        .subscribe(query::Condition::And(vec![]))
        .await
        .unwrap();
    assert_eq!(
        client.subscription_next(&mut subscription).await.unwrap(),
        Notice::SnapshotChunk
    );
    assert_eq!(
        client.subscription_next(&mut subscription).await.unwrap(),
        Notice::SnapshotEnd
    );
    assert!(subscription.contents().unwrap().is_empty());

    let token = a
        .save(
            1,
            Command::Create {
                fields: fields("pushed"),
            },
        )
        .await
        .view
        .unwrap()
        .edit_token;
    let (notice, _) = tokio::join!(
        tokio::time::timeout(
            Duration::from_secs(3),
            client.subscription_next(&mut subscription)
        ),
        async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            peer::sync_once(&a.store, &a.tls, b.identity.node)
                .await
                .unwrap();
        },
    );
    assert_eq!(
        notice.unwrap().unwrap(),
        Notice::Changes {
            upserted: vec![id],
            removed: vec![]
        }
    );
    assert_eq!(
        subscription.contents().unwrap()[&id]["content"]["body"],
        "pushed"
    );

    a.save(
        2,
        Command::Edit {
            token,
            fields: fields("pulled"),
        },
    )
    .await;
    let (notice, _) = tokio::join!(
        tokio::time::timeout(
            Duration::from_secs(3),
            client.subscription_next(&mut subscription)
        ),
        async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            peer::sync_once(&b.store, &b.tls, a.identity.node)
                .await
                .unwrap();
        },
    );
    assert_eq!(
        notice.unwrap().unwrap(),
        Notice::Changes {
            upserted: vec![id],
            removed: vec![]
        }
    );
    assert_eq!(
        subscription.contents().unwrap()[&id]["content"]["body"],
        "pulled"
    );
    stop.send(true).unwrap();
    app_task.await.unwrap().unwrap();
    let listener = TcpListener::bind(app_endpoint).await.unwrap();
    let (stop, receiver) = watch::channel(false);
    let app_task = tokio::spawn(harmion_net_native::app::serve(
        listener,
        b.store.clone(),
        b.tls.clone(),
        registration,
        receiver,
    ));
    assert_eq!(
        client.subscription_next(&mut subscription).await.unwrap(),
        Notice::ResetRequired
    );
    assert!(subscription.contents().is_none());
    stop.send(true).unwrap();
    app_task.await.unwrap().unwrap();
    a.finish().await;
    b.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_mutual_tls_sync_preserves_partitioned_edits_and_deletion_conflicts() {
    let dir = Directory::new();
    let a = Node::start(&dir, 1).await;
    let b = Node::start(&dir, 2).await;
    a.approve(&b).await;
    b.approve(&a).await;
    a.save(
        1,
        Command::Create {
            fields: fields("base"),
        },
    )
    .await;
    peer::sync_once(&a.store, &a.tls, b.identity.node)
        .await
        .unwrap();
    let at = a.get().await.edit_token;
    let bt = b.get().await.edit_token;
    a.save(
        2,
        Command::Edit {
            token: at,
            fields: fields("A offline"),
        },
    )
    .await;
    b.save(
        1,
        Command::Edit {
            token: bt,
            fields: fields("B offline"),
        },
    )
    .await;
    let report = peer::sync_once(&a.store, &a.tls, b.identity.node)
        .await
        .unwrap();
    assert!(report.uploaded > 0 && report.downloaded > 0);
    assert_eq!(a.snapshot().await, b.snapshot().await);
    assert_eq!(a.snapshot().await.fields[&Field::Body].candidates.len(), 2);
    let at = a.get().await.edit_token;
    let bt = b.get().await.edit_token;
    a.save(3, Command::Delete { token: at }).await;
    b.save(
        2,
        Command::Edit {
            token: bt,
            fields: fields("concurrent with deletion"),
        },
    )
    .await;
    peer::sync_once(&b.store, &b.tls, a.identity.node)
        .await
        .unwrap();
    assert_eq!(a.snapshot().await, b.snapshot().await);
    assert_eq!(a.snapshot().await.life, Life::Conflict);
    for _ in 0..3 {
        peer::sync_once(&a.store, &a.tls, b.identity.node)
            .await
            .unwrap();
    }
    assert_eq!(a.count().await, 5);
    assert_eq!(b.count().await, 5);
    let settled = peer::sync_once(&a.store, &a.tls, b.identity.node)
        .await
        .unwrap();
    assert_eq!((settled.uploaded, settled.downloaded), (0, 0));
    a.finish().await;
    b.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_certificate_wrong_cluster_and_node_claims_cannot_sync() {
    let dir = Directory::new();
    let a = Node::start(&dir, 1).await;
    let b = Node::start(&dir, 2).await;
    a.approve(&b).await;
    b.approve(&a).await;
    let rogue = TlsIdentity::generate().unwrap();
    assert!(
        peer::exchange(&rogue, a.identity, &b.binding(), PeerCommand::Status)
            .await
            .is_err()
    );
    let wrong_cluster = Identity {
        cluster: ClusterId([9; 32]),
        ..a.identity
    };
    assert!(
        peer::exchange(&a.tls, wrong_cluster, &b.binding(), PeerCommand::Status)
            .await
            .is_err()
    );
    let wrong_node = Identity {
        node: NodeId([9; 32]),
        ..a.identity
    };
    assert!(
        peer::exchange(&a.tls, wrong_node, &b.binding(), PeerCommand::Status)
            .await
            .is_err()
    );
    let mut impostor = b.binding();
    impostor.key = membership::PeerKey::TlsCertificate(rogue.certificate().to_vec());
    assert!(
        peer::exchange(&a.tls, a.identity, &impostor, PeerCommand::Status)
            .await
            .is_err()
    );
    // All Read/Write rights still do not grant ManageNodes.
    let admin = a.admin.clone();
    let (_, limited) = a
        .store
        .run(move |s| {
            app::issue_app(
                s,
                &mut SystemHost,
                &admin,
                Grants {
                    read: Some(Filter::All),
                    write: Some(Filter::All),
                    ..Grants::default()
                },
                i64::MAX,
            )
        })
        .await
        .unwrap();
    assert!(
        peer::approve(
            &a.store,
            limited,
            SystemHost.now_micros(),
            a.identity.cluster,
            b.binding()
        )
        .await
        .is_err()
    );
    assert_eq!(a.count().await, 0);
    assert_eq!(b.count().await, 0);
    a.finish().await;
    b.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lost_ack_and_restart_keep_durable_progress() {
    let dir = Directory::new();
    let a = Node::start(&dir, 1).await;
    let b = Node::start(&dir, 2).await;
    a.approve(&b).await;
    b.approve(&a).await;
    a.save(
        1,
        Command::Create {
            fields: fields("durable"),
        },
    )
    .await;
    let epoch = a.identity.epoch;
    let page = a
        .store
        .run(move |s| {
            s.transaction(|tx| {
                let high = replication::high(tx)?;
                replication::page(tx, epoch, 0, high)
            })
        })
        .await
        .unwrap();
    let reply = peer::exchange(
        &a.tls,
        a.identity,
        &b.binding(),
        PeerCommand::Accept(page.clone()),
    )
    .await
    .unwrap();
    assert!(matches!(reply, PeerReply::Ack { through: 1, .. }));
    // Reopen the receiver DB without losing node ID, epoch, membership or cursor.
    let id = b.identity;
    let path = dir.0.join("2.db");
    b.store
        .run(move |s| {
            *s = SqliteStore::open(path, id)?;
            Ok(())
        })
        .await
        .unwrap();
    let reply = peer::exchange(&a.tls, a.identity, &b.binding(), PeerCommand::Accept(page))
        .await
        .unwrap();
    assert!(matches!(
        reply,
        PeerReply::CursorMismatch { current: 1, .. }
    ));
    peer::sync_once(&a.store, &a.tls, b.identity.node)
        .await
        .unwrap();
    assert_eq!(b.count().await, 1);
    assert_eq!(a.snapshot().await, b.snapshot().await);
    a.finish().await;
    b.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retirement_revokes_existing_tls_connection_and_node_id_cannot_rejoin() {
    use bytes::Bytes;
    use http_body_util::{BodyExt, Full};
    use hyper::Request;
    use hyper_util::rt::TokioIo;
    use tokio_rustls::TlsConnector;
    let dir = Directory::new();
    let a = Node::start(&dir, 1).await;
    let b = Node::start(&dir, 2).await;
    a.approve(&b).await;
    b.approve(&a).await;
    let tcp = tokio::net::TcpStream::connect(&b.endpoint).await.unwrap();
    let tls = TlsConnector::from(Arc::new(a.tls.peer_client_config(&b.binding()).unwrap()))
        .connect(SERVER_NAME.try_into().unwrap(), tcp)
        .await
        .unwrap();
    let (mut client, connection) = hyper::client::conn::http1::handshake(TokioIo::new(tls))
        .await
        .unwrap();
    let task = tokio::spawn(connection);
    let request = || {
        Request::post("/peer/v1")
            .header("host", SERVER_NAME)
            .body(Full::new(Bytes::from(
                encode_peer_request(&PeerRequest {
                    cluster: a.identity.cluster,
                    node: a.identity.node,
                    command: PeerCommand::Status,
                })
                .unwrap(),
            )))
            .unwrap()
    };
    let response = client.send_request(request()).await.unwrap();
    assert_eq!(response.status(), 200);
    response.into_body().collect().await.unwrap();
    let admin = b.admin.clone();
    let node = a.identity.node;
    b.store
        .run(move |s| membership::retire(s, SystemHost.now_micros(), &admin, node))
        .await
        .unwrap();
    let response = client.send_request(request()).await.unwrap();
    assert_eq!(response.status(), 403);
    response.into_body().collect().await.unwrap();
    assert!(
        peer::approve(
            &b.store,
            b.admin.clone(),
            SystemHost.now_micros(),
            b.identity.cluster,
            a.binding()
        )
        .await
        .is_err()
    );
    drop(client);
    task.await.unwrap().unwrap();
    assert!(
        peer::exchange(&a.tls, a.identity, &b.binding(), PeerCommand::Status)
            .await
            .is_err()
    );
    a.finish().await;
    b.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn both_peers_can_initiate_sync_without_cursor_races() {
    let dir = Directory::new();
    let a = Node::start(&dir, 1).await;
    let b = Node::start(&dir, 2).await;
    a.approve(&b).await;
    b.approve(&a).await;
    a.save(
        1,
        Command::Create {
            fields: fields("base"),
        },
    )
    .await;
    peer::sync_once(&a.store, &a.tls, b.identity.node)
        .await
        .unwrap();
    for n in 1..=5 {
        let at = a.get().await.edit_token;
        let bt = b.get().await.edit_token;
        a.save(
            n + 1,
            Command::Edit {
                token: at,
                fields: fields("A"),
            },
        )
        .await;
        b.save(
            n,
            Command::Edit {
                token: bt,
                fields: fields("B"),
            },
        )
        .await;
        let (left, right) = tokio::join!(
            peer::sync_once(&a.store, &a.tls, b.identity.node),
            peer::sync_once(&b.store, &b.tls, a.identity.node)
        );
        left.unwrap();
        right.unwrap();
        assert_eq!(a.snapshot().await, b.snapshot().await);
    }
    assert_eq!(a.count().await, 11);
    assert_eq!(b.count().await, 11);
    a.finish().await;
    b.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejected_update_reports_id_and_reason_after_rolling_back_page_prefix() {
    let dir = Directory::new();
    let a = Node::start(&dir, 1).await;
    let b = Node::start(&dir, 2).await;
    a.approve(&b).await;
    b.approve(&a).await;
    a.save(
        1,
        Command::Create {
            fields: fields("base"),
        },
    )
    .await;
    let first = a
        .store
        .run(|s| s.transaction(|tx| Ok(tx.entries()?.remove(0))))
        .await
        .unwrap();
    let mut bad = first.record.update().clone();
    bad.update_id = UpdateId([8; 32]);
    bad.action = Action::Edit;
    bad.basis = std::collections::BTreeSet::from([UpdateId([9; 32])]);
    let page = Page {
        epoch: a.identity.epoch,
        after: 0,
        through: 2,
        high: 2,
        entries: vec![
            first,
            LogEntry {
                seq: 2,
                record: Record::from_update(bad).unwrap(),
            },
        ],
    };
    let result = peer::exchange(&a.tls, a.identity, &b.binding(), PeerCommand::Accept(page)).await;
    assert!(matches!(
        result,
        Err(harmion_net_native::Error::RemoteRejected {
            update_id: UpdateId([8, ..]),
            reason: Rejection::MissingDependency,
        })
    ));
    assert_eq!(b.count().await, 0);
    let sender = a.identity;
    assert_eq!(
        b.store
            .run(move |s| s.transaction(|tx| tx.cursor(sender.node, sender.epoch)))
            .await
            .unwrap(),
        0
    );
    peer::sync_once(&a.store, &a.tls, b.identity.node)
        .await
        .unwrap();
    assert_eq!(b.count().await, 1);
    a.finish().await;
    b.finish().await;
}
