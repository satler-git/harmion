use harmion_domain::*;
use harmion_net_native::{
    SharedStore, SystemHost,
    iroh_peer::{self, IrohTransport},
    sync::{self, PeerTransport},
};
use harmion_node::{
    application,
    membership::{self, Peer, PeerKey},
    storage::{Identity, Store, read_graph},
};
use harmion_protocol::*;
use harmion_store_sqlite::SqliteStore;
use iroh::{Endpoint, SecretKey, address_lookup::MemoryLookup};
use tokio::sync::watch;

#[tokio::test]
async fn sync_works_through_a_custom_relay_with_no_direct_ip_transport() {
    let mut config = iroh_relay::server::ServerConfig::default();
    config.relay = Some(iroh_relay::server::RelayConfig::new((
        std::net::Ipv4Addr::LOCALHOST,
        0,
    )));
    let relay = iroh_relay::server::Server::spawn(config).await.unwrap();
    let url: iroh::RelayUrl = format!("http://{}", relay.http_addr().unwrap())
        .parse()
        .unwrap();
    async fn relay_endpoint(n: u8, url: iroh::RelayUrl) -> Endpoint {
        iroh::endpoint::Builder::empty()
            .crypto_provider(std::sync::Arc::new(rustls::crypto::ring::default_provider()))
            .secret_key(SecretKey::from_bytes(&[n; 32]))
            .alpns(vec![iroh_peer::ALPN.to_vec()])
            .clear_ip_transports()
            .relay_mode(iroh::RelayMode::custom([url]))
            .bind()
            .await
            .unwrap()
    }
    let a = relay_endpoint(12, url.clone()).await;
    let b = relay_endpoint(13, url.clone()).await;
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        a.online().await;
        b.online().await;
    })
    .await
    .unwrap();
    assert_eq!(a.addr().ip_addrs().count(), 0);
    assert_eq!(b.addr().ip_addrs().count(), 0);
    let (sa, ia, admin_a) = store(12);
    let (sb, ib, admin_b) = store(13);
    let pa = Peer {
        node: ia.node,
        key: PeerKey::Iroh(*a.id().as_bytes()),
        endpoint: url.to_string(),
        retired: false,
    };
    let pb = Peer {
        node: ib.node,
        key: PeerKey::Iroh(*b.id().as_bytes()),
        endpoint: url.to_string(),
        retired: false,
    };
    sa.run(move |s| membership::approve(s, 0, &admin_a, ia.cluster, pb))
        .await
        .unwrap();
    let credential = admin_b.clone();
    sb.run(move |s| membership::approve(s, 0, &credential, ib.cluster, pa))
        .await
        .unwrap();
    sb.run(move |s| {
        application::save(
            s,
            &mut SystemHost,
            &admin_b,
            &SaveRequest {
                request_id: RequestId([15; 32]),
                content_id: ContentId([15; 32]),
                command: Command::Create {
                    fields: [(Field::Body, Value::Body("relay only".into()))].into(),
                },
            },
        )
    })
    .await
    .unwrap();
    let (stop, rx) = watch::channel(false);
    let task = tokio::spawn(iroh_peer::serve(b.clone(), sb, rx));
    let transport = IrohTransport {
        endpoint: a.clone(),
    };
    assert_eq!(
        sync::sync_once(&sa, &transport, ib.node)
            .await
            .unwrap()
            .downloaded,
        1
    );
    sa.run(|s| {
        s.transaction(|tx| {
            assert_eq!(
                read_graph(tx)?
                    .snapshot(ContentId([15; 32]))
                    .unwrap()
                    .content
                    .body,
                "relay only"
            );
            Ok(())
        })
    })
    .await
    .unwrap();
    stop.send(true).unwrap();
    task.await.unwrap().unwrap();
    a.close().await;
    b.close().await;
    relay.shutdown().await.unwrap();
}

async fn endpoint(key: u8, ip: &str, lookup: MemoryLookup) -> Endpoint {
    iroh::endpoint::Builder::empty()
        .crypto_provider(std::sync::Arc::new(rustls::crypto::ring::default_provider()))
        .secret_key(SecretKey::from_bytes(&[key; 32]))
        .alpns(vec![iroh_peer::ALPN.to_vec()])
        .clear_ip_transports()
        .bind_addr(format!("{ip}:0"))
        .unwrap()
        .address_lookup(lookup)
        .bind()
        .await
        .unwrap()
}
fn store(
    n: u8,
) -> (
    SharedStore<SqliteStore>,
    Identity,
    harmion_node::access::Credential,
) {
    let identity = Identity {
        cluster: ClusterId([1; 32]),
        node: NodeId([n; 32]),
        epoch: LogEpoch([n; 32]),
    };
    let mut db = SqliteStore::open(":memory:", identity).unwrap();
    let (_, admin) = application::bootstrap_admin(&mut db, &mut SystemHost, i64::MAX).unwrap();
    (SharedStore::new(db), identity, admin)
}
#[tokio::test]
async fn endpoint_identity_survives_ip_change_without_reapproval() {
    let lookup = MemoryLookup::default();
    let a = endpoint(2, "127.0.0.1", lookup.clone()).await;
    let b = endpoint(3, "127.0.0.1", lookup.clone()).await;
    lookup.set_endpoint_info(a.addr());
    lookup.set_endpoint_info(b.addr());
    let (sa, ia, admin_a) = store(2);
    let (sb, ib, admin_b) = store(3);
    let pa = Peer {
        node: ia.node,
        key: PeerKey::Iroh(*a.id().as_bytes()),
        endpoint: String::new(),
        retired: false,
    };
    let pb = Peer {
        node: ib.node,
        key: PeerKey::Iroh(*b.id().as_bytes()),
        endpoint: String::new(),
        retired: false,
    };
    let approve_b = pb.clone();
    let approve_a = pa.clone();
    sa.run(move |s| membership::approve(s, 0, &admin_a, ia.cluster, approve_b))
        .await
        .unwrap();
    let credential_b = admin_b.clone();
    sb.run(move |s| membership::approve(s, 0, &credential_b, ib.cluster, approve_a))
        .await
        .unwrap();
    let (stop, rx) = watch::channel(false);
    let task = tokio::spawn(iroh_peer::serve(b.clone(), sb.clone(), rx));
    let transport = IrohTransport {
        endpoint: a.clone(),
    };
    // Populate the responder then pull using only the stable Endpoint ID.
    let credential = admin_b.clone();
    sb.run(move |s| {
        application::save(
            s,
            &mut SystemHost,
            &credential,
            &SaveRequest {
                request_id: RequestId([4; 32]),
                content_id: ContentId([4; 32]),
                command: Command::Create {
                    fields: [(Field::Body, Value::Body("first".into()))].into(),
                },
            },
        )
    })
    .await
    .unwrap();
    assert_eq!(
        sync::sync_once(&sa, &transport, ib.node)
            .await
            .unwrap()
            .downloaded,
        1
    );
    let old_addr = b.addr();
    let old_id = b.id();
    stop.send(true).unwrap();
    task.await.unwrap().unwrap();
    b.close().await;
    let b = endpoint(3, "127.0.0.2", lookup.clone()).await;
    assert_eq!(b.id(), old_id);
    assert_ne!(b.addr(), old_addr);
    lookup.set_endpoint_info(b.addr());
    let (stop, rx) = watch::channel(false);
    let task = tokio::spawn(iroh_peer::serve(b.clone(), sb.clone(), rx));
    sb.run(move |s| {
        application::save(
            s,
            &mut SystemHost,
            &admin_b,
            &SaveRequest {
                request_id: RequestId([5; 32]),
                content_id: ContentId([5; 32]),
                command: Command::Create {
                    fields: [(Field::Body, Value::Body("after IP change".into()))].into(),
                },
            },
        )
    })
    .await
    .unwrap();
    assert_eq!(
        sync::sync_once(&sa, &transport, ib.node)
            .await
            .unwrap()
            .downloaded,
        1
    );
    sa.run(move |s| {
        s.transaction(|tx| {
            assert_eq!(tx.cursor(ib.node, ib.epoch)?, 2);
            assert_eq!(tx.entries()?.len(), 2);
            assert!(read_graph(tx)?.snapshot(ContentId([5; 32])).is_some());
            Ok(())
        })
    })
    .await
    .unwrap();
    // The authenticated key must not authorize another claimed Harmion Node ID.
    let forged = Identity {
        node: NodeId([99; 32]),
        ..ia
    };
    assert!(
        transport
            .exchange(forged, &pb, PeerCommand::Status)
            .await
            .is_err()
    );
    stop.send(true).unwrap();
    task.await.unwrap().unwrap();
    a.close().await;
    b.close().await;
}
