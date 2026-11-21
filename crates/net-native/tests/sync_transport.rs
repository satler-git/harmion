//! Exercise the complete round with no socket, certificate parser or TLS identity.
use harmion_domain::*;
use harmion_net_native::{
    Error, SharedStore, SystemHost,
    sync::{self, PeerTransport},
};
use harmion_node::{
    access::Credential,
    application,
    membership::{self, Peer},
    storage::{Identity, Store, read_graph},
};
use harmion_protocol::*;
use harmion_store_sqlite::SqliteStore;

struct Node {
    store: SharedStore<SqliteStore>,
    identity: Identity,
    binding: Peer,
    admin: Credential,
}
impl Node {
    fn new(n: u8) -> Self {
        let identity = Identity {
            cluster: ClusterId([1; 32]),
            node: NodeId([n; 32]),
            epoch: LogEpoch([n; 32]),
        };
        let mut db = SqliteStore::open(":memory:", identity).unwrap();
        let (_, admin) = application::bootstrap_admin(&mut db, &mut SystemHost, i64::MAX).unwrap();
        Self {
            store: SharedStore::new(db),
            identity,
            binding: Peer {
                node: identity.node,
                key: membership::PeerKey::Iroh([n; 32]),
                endpoint: "in-process".into(),
                retired: false,
            },
            admin,
        }
    }
    async fn approve(&self, other: &Node) {
        let credential = self.admin.clone();
        let peer = other.binding.clone();
        let cluster = self.identity.cluster;
        self.store
            .run(move |s| membership::approve(s, 0, &credential, cluster, peer))
            .await
            .unwrap();
    }
    async fn status(&self) -> harmion_protocol::sync_status::SyncStatus {
        let credential = self.admin.clone();
        self.store
            .run(move |s| harmion_node::sync_status::get(s, &credential, 0, None))
            .await
            .unwrap()
    }
    async fn create(&self, content: u8) {
        let credential = self.admin.clone();
        self.store
            .run(move |s| {
                application::save(
                    s,
                    &mut SystemHost,
                    &credential,
                    &SaveRequest {
                        request_id: RequestId([content; 32]),
                        content_id: ContentId([content; 32]),
                        command: Command::Create {
                            fields: [(Field::Body, Value::Body(format!("content {content}")))]
                                .into(),
                        },
                    },
                )
            })
            .await
            .unwrap();
    }
}
struct InProcess<'a> {
    local: &'a Node,
    remote: &'a Node,
    retire_on_page: bool,
}

#[tokio::test]
async fn an_unresponsive_peer_does_not_lock_out_another_peer() {
    let a = Node::new(2);
    let stalled = Node::new(3);
    let online = Node::new(4);
    a.approve(&stalled).await;
    a.approve(&online).await;
    online.approve(&a).await;
    online.create(5).await;
    struct Routes<'a> {
        stalled: NodeId,
        entered: tokio::sync::Notify,
        online: InProcess<'a>,
    }
    impl PeerTransport for Routes<'_> {
        async fn exchange(
            &self,
            local: Identity,
            peer: &Peer,
            command: PeerCommand,
        ) -> Result<PeerReply, Error> {
            if peer.node == self.stalled {
                self.entered.notify_one();
                std::future::pending().await
            } else {
                self.online.exchange(local, peer, command).await
            }
        }
    }
    let routes = Routes {
        stalled: stalled.identity.node,
        entered: tokio::sync::Notify::new(),
        online: InProcess {
            local: &a,
            remote: &online,
            retire_on_page: false,
        },
    };
    tokio::select! {
        _=sync::sync_once(&a.store,&routes,stalled.identity.node) => panic!("stalled peer returned"),
        result=async {
            routes.entered.notified().await;
            tokio::time::timeout(std::time::Duration::from_secs(2),sync::sync_once(&a.store,&routes,online.identity.node)).await
        } => {assert_eq!(result.expect("another peer was blocked").unwrap().downloaded,1);},
    }
}
impl PeerTransport for InProcess<'_> {
    async fn exchange(
        &self,
        local: Identity,
        peer: &Peer,
        command: PeerCommand,
    ) -> Result<PeerReply, Error> {
        assert_eq!(peer, &self.remote.binding);
        let certificate = self.local.binding.key.clone();
        let is_page = matches!(command, PeerCommand::Page { .. });
        let response = self
            .remote
            .store
            .run(move |s| {
                membership::handle(
                    s,
                    &certificate,
                    &PeerRequest {
                        cluster: local.cluster,
                        node: local.node,
                        command,
                    },
                )
            })
            .await?;
        assert_eq!(response.node, peer.node);
        assert_eq!(response.cluster, local.cluster);
        if self.retire_on_page && is_page {
            let credential = self.local.admin.clone();
            let node = peer.node;
            self.local
                .store
                .run(move |s| membership::retire(s, 0, &credential, node))
                .await?;
        }
        Ok(response.reply)
    }
}
#[tokio::test]
async fn same_round_runs_without_a_tls_host_and_stops_echoing() {
    let a = Node::new(2);
    let b = Node::new(3);
    a.approve(&b).await;
    b.approve(&a).await;
    a.create(4).await;
    b.create(5).await;
    let transport = InProcess {
        local: &a,
        remote: &b,
        retire_on_page: false,
    };
    let unknown = a.status().await;
    assert!(unknown.peers[0].last_success_at.is_none());
    assert_eq!(unknown.peers[0].unconfirmed_local, "1");
    let first = sync::sync_once(&a.store, &transport, b.identity.node)
        .await
        .unwrap();
    assert_eq!((first.uploaded, first.downloaded), (1, 1));
    let status = a.status().await;
    assert!(status.peers[0].last_success_at.is_some());
    assert_eq!(status.peers[0].acknowledged_through.as_deref(), Some("1"));
    // The pulled update was accepted after the frozen upload high. A successful
    // round does not imply every entry currently in our log was acknowledged.
    assert_eq!(status.local_high, "2");
    assert_eq!(status.peers[0].unconfirmed_local, "1");
    // B received a push but did not initiate a round: don't invent a B-side proof.
    assert!(b.status().await.peers[0].last_success_at.is_none());
    sync::sync_once(&a.store, &transport, b.identity.node)
        .await
        .unwrap();
    let quiet = sync::sync_once(&a.store, &transport, b.identity.node)
        .await
        .unwrap();
    assert_eq!((quiet.uploaded, quiet.downloaded), (0, 0));
    assert_eq!(a.status().await.peers[0].unconfirmed_local, "0");
    for node in [&a, &b] {
        node.store
            .run(|s| {
                s.transaction(|tx| {
                    let graph = read_graph(tx)?;
                    assert!(graph.snapshot(ContentId([4; 32])).is_some());
                    assert!(graph.snapshot(ContentId([5; 32])).is_some());
                    assert_eq!(tx.entries()?.len(), 2);
                    Ok(())
                })
            })
            .await
            .unwrap();
    }
}
#[tokio::test]
async fn failed_round_preserves_proof_and_successful_retry_clears_failure() {
    let a = Node::new(2);
    let b = Node::new(3);
    a.approve(&b).await;
    b.approve(&a).await;
    a.create(4).await;
    let transport = InProcess {
        local: &a,
        remote: &b,
        retire_on_page: false,
    };
    sync::sync_once(&a.store, &transport, b.identity.node)
        .await
        .unwrap();
    let before = a.status().await;
    a.create(5).await;
    struct Offline;
    impl PeerTransport for Offline {
        async fn exchange(
            &self,
            _: Identity,
            _: &Peer,
            _: PeerCommand,
        ) -> Result<PeerReply, Error> {
            Err(Error::Transport(
                "sensitive transport detail must not be persisted".into(),
            ))
        }
    }
    assert!(
        sync::sync_once(&a.store, &Offline, b.identity.node)
            .await
            .is_err()
    );
    let failed = a.status().await;
    assert_eq!(
        failed.peers[0].last_success_at,
        before.peers[0].last_success_at
    );
    assert_eq!(
        failed.peers[0].acknowledged_through,
        before.peers[0].acknowledged_through
    );
    assert_eq!(failed.peers[0].unconfirmed_local, "1");
    assert_eq!(
        failed.peers[0].last_failure.as_ref().unwrap().code,
        "transport_error"
    );
    assert!(
        !serde_json::to_string(&failed)
            .unwrap()
            .contains("sensitive")
    );
    sync::sync_once(&a.store, &transport, b.identity.node)
        .await
        .unwrap();
    let retried = a.status().await;
    assert!(retried.peers[0].last_failure.is_none());
    assert_eq!(retried.peers[0].unconfirmed_local, "0");
}

#[tokio::test]
async fn lost_ack_does_not_fabricate_confirmation_and_cursor_retry_recovers_it() {
    let a = Node::new(2);
    let b = Node::new(3);
    a.approve(&b).await;
    b.approve(&a).await;
    a.create(4).await;
    struct LoseAck<'a>(InProcess<'a>);
    impl PeerTransport for LoseAck<'_> {
        async fn exchange(
            &self,
            local: Identity,
            peer: &Peer,
            command: PeerCommand,
        ) -> Result<PeerReply, Error> {
            let accepting = matches!(&command, PeerCommand::Accept(_));
            let reply = self.0.exchange(local, peer, command).await?;
            if accepting {
                Err(Error::Timeout)
            } else {
                Ok(reply)
            }
        }
    }
    let transport = InProcess {
        local: &a,
        remote: &b,
        retire_on_page: false,
    };
    assert!(
        sync::sync_once(&a.store, &LoseAck(transport), b.identity.node)
            .await
            .is_err()
    );
    assert_eq!(b.status().await.local_high, "1"); // Data arrived; ACK did not.
    let failed = a.status().await;
    assert!(failed.peers[0].acknowledged_through.is_none());
    assert_eq!(failed.peers[0].unconfirmed_local, "1");
    assert_eq!(
        failed.peers[0].last_failure.as_ref().unwrap().code,
        "timeout"
    );
    let transport = InProcess {
        local: &a,
        remote: &b,
        retire_on_page: false,
    };
    let report = sync::sync_once(&a.store, &transport, b.identity.node)
        .await
        .unwrap();
    assert_eq!(report.uploaded, 0);
    assert_eq!(a.status().await.peers[0].unconfirmed_local, "0");
}

#[tokio::test]
async fn invalid_remote_cursor_is_not_recorded_as_a_confirmation() {
    let a = Node::new(2);
    let b = Node::new(3);
    a.approve(&b).await;
    b.approve(&a).await;
    a.create(4).await;
    struct BadCursor<'a>(InProcess<'a>);
    impl PeerTransport for BadCursor<'_> {
        async fn exchange(
            &self,
            local: Identity,
            peer: &Peer,
            command: PeerCommand,
        ) -> Result<PeerReply, Error> {
            if let PeerCommand::Cursor { epoch } = command {
                Ok(PeerReply::Cursor {
                    epoch,
                    through: u64::MAX,
                })
            } else {
                self.0.exchange(local, peer, command).await
            }
        }
    }
    let transport = BadCursor(InProcess {
        local: &a,
        remote: &b,
        retire_on_page: false,
    });
    assert!(matches!(
        sync::sync_once(&a.store, &transport, b.identity.node).await,
        Err(Error::InvalidResponse)
    ));
    let status = a.status().await;
    assert!(status.peers[0].acknowledged_through.is_none());
    assert_eq!(
        status.peers[0].last_failure.as_ref().unwrap().code,
        "invalid_response"
    );
}

#[tokio::test]
async fn retirement_while_response_is_in_flight_rejects_page_and_cursor() {
    let a = Node::new(2);
    let b = Node::new(3);
    a.approve(&b).await;
    b.approve(&a).await;
    b.create(5).await;
    let transport = InProcess {
        local: &a,
        remote: &b,
        retire_on_page: true,
    };
    assert!(matches!(
        sync::sync_once(&a.store, &transport, b.identity.node).await,
        Err(Error::Node(harmion_node::Error::PeerRetired))
    ));
    let status = a.status().await;
    assert!(status.peers[0].last_success_at.is_none());
    assert_eq!(
        status.peers[0].last_failure.as_ref().unwrap().code,
        "peer_retired"
    );
    let identity = b.identity;
    a.store
        .run(move |s| {
            s.transaction(|tx| {
                assert!(tx.entries()?.is_empty());
                assert_eq!(tx.cursor(identity.node, identity.epoch)?, 0);
                Ok(())
            })
        })
        .await
        .unwrap();
}
