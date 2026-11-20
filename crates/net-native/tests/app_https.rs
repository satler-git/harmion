use harmion_client::{Client, Subscription, SubscriptionNotification as Notice};
use harmion_domain::*;
use harmion_net_native::{SharedStore, SystemHost, app, tls::TlsIdentity};
use harmion_node::{application, storage::Identity};
use harmion_protocol::*;
use harmion_store_sqlite::SqliteStore;
use std::{sync::Arc, time::Duration};
use tokio::{net::TcpListener, sync::watch, time::timeout};

#[tokio::test]
async fn pinned_app_api_save_replay_permissions_and_revocation() {
    let identity = Identity {
        cluster: ClusterId([1; 32]),
        node: NodeId([2; 32]),
        epoch: LogEpoch([3; 32]),
    };
    let mut db = SqliteStore::open(":memory:", identity).unwrap();
    let (_, admin) = application::bootstrap_admin(&mut db, &mut SystemHost, i64::MAX).unwrap();
    let store = SharedStore::new(db);
    let tls = Arc::new(TlsIdentity::generate().unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = listener.local_addr().unwrap().to_string();
    let info = ConnectionInfo {
        version: 1,
        cluster_id: hex(&identity.cluster.0),
        node_id: hex(&identity.node.0),
        certificate: hex(tls.certificate()),
        endpoint: endpoint.clone(),
    };
    let registration = PeerRegistration {
        signing_key: None,
        cluster_root: None,
        version: 2,
        cluster_id: info.cluster_id.clone(),
        node_id: info.node_id.clone(),
        binding: PeerBinding::Https {
            certificate: info.certificate.clone(),
            endpoint,
        },
    };
    let (stop, receiver) = watch::channel(false);
    let server = tokio::spawn(app::serve(
        listener,
        store.clone(),
        tls,
        registration,
        receiver,
    ));
    let client = Client::new(info.clone(), *admin.expose_bytes()).unwrap();
    let capabilities = client.call("capabilities", &[]).await.unwrap();
    assert_eq!(capabilities["oauth"], false);
    assert_eq!(capabilities["query"], true);
    let content = ContentId([4; 32]);
    let request = SaveRequest {
        request_id: RequestId([5; 32]),
        content_id: content,
        command: Command::Create {
            fields: [(Field::Body, Value::Body("initial".into()))].into(),
        },
    };
    let saved = client.save(&request).await.unwrap();
    timeout(Duration::from_secs(1), store.local_save_notified())
        .await
        .unwrap();
    assert_eq!(saved, client.save(&request).await.unwrap());
    let read = client.get(content).await.unwrap();
    assert_eq!(read["content"]["body"], "initial");
    assert!(read.get("basis").is_none());
    assert!(read["content"].get("root").is_none());
    let second = SaveRequest {
        content_id: ContentId([6; 32]),
        request_id: RequestId([7; 32]),
        ..request.clone()
    };
    client.save(&second).await.unwrap();
    timeout(Duration::from_secs(1), store.local_save_notified())
        .await
        .unwrap();
    let page = client
        .query(query::Condition::And(vec![]), 1)
        .await
        .unwrap();
    assert_eq!(page.items[0]["content"]["id"], hex(&content.0));
    let tail = client.query_next(page.next.unwrap(), 1).await.unwrap();
    assert_eq!(tail.items[0]["content"]["id"], hex(&second.content_id.0));
    assert!(tail.next.is_none());
    let changed = SaveRequest {
        command: Command::Create {
            fields: [(Field::Body, Value::Body("changed".into()))].into(),
        },
        ..request
    };
    assert!(matches!(
        client.save(&changed).await,
        Err(harmion_client::Error::Http(409))
    ));
    let wrong = Client::new(info.clone(), [99; 32]).unwrap();
    assert!(matches!(
        wrong.get(content).await,
        Err(harmion_client::Error::Http(401))
    ));
    let mut wrong_pin = info.clone();
    wrong_pin.certificate = hex(TlsIdentity::generate().unwrap().certificate());
    assert!(matches!(
        Client::new(wrong_pin, *admin.expose_bytes())
            .unwrap()
            .get(content)
            .await,
        Err(harmion_client::Error::Transport)
    ));
    let issued = client
        .call(
            "issue-app",
            &serde_json::to_vec(&IssueAppRequest {
                grants: GrantSpec {
                    read: Some(FilterSpec::All),
                    write: None,
                    manage_apps: false,
                    manage_nodes: false,
                },
                expires_at: i64::MAX.to_string(),
            })
            .unwrap(),
        )
        .await
        .unwrap();
    let reader = Client::new(info, unhex(issued["credential"].as_str().unwrap()).unwrap()).unwrap();
    assert!(matches!(
        reader.call("metrics", &[]).await,
        Err(harmion_client::Error::Http(403))
    ));
    let metrics = client.call("metrics", &[]).await.unwrap();
    assert_eq!(metrics["contents"], "2");
    assert_eq!(metrics["visible_contents"], "2");
    assert_eq!(metrics["accepted_updates"], "2");
    let mut export_records = 0;
    let export = reader
        .export(|_| {
            export_records += 1;
            Ok(())
        })
        .await;
    assert!(matches!(export, Err(harmion_client::Error::Http(403))));
    assert_eq!(export_records, 0);
    assert_eq!(
        reader.get(content).await.unwrap()["content"]["body"],
        "initial"
    );
    assert!(matches!(
        reader.save(&changed).await,
        Err(harmion_client::Error::Http(403))
    ));
    assert!(
        timeout(Duration::from_millis(50), store.local_save_notified())
            .await
            .is_err(),
        "replayed or rejected save triggered synchronization"
    );
    assert!(matches!(
        reader.call("sync", &[7; 32]).await,
        Err(harmion_client::Error::Http(403))
    ));
    assert_eq!(capabilities["subscriptions"], true);
    let mut sub = reader
        .subscribe(query::Condition::And(vec![]))
        .await
        .unwrap();
    assert!(sub.contents().is_none());
    assert_eq!(
        reader.subscription_next(&mut sub).await.unwrap(),
        Notice::SnapshotChunk
    );
    assert!(sub.contents().is_none());
    assert_eq!(
        reader.subscription_next(&mut sub).await.unwrap(),
        Notice::SnapshotEnd
    );
    let frozen_edit = sub.contents().unwrap()[&content]["edit_token"].clone();
    let checkpoint = sub.checkpoint().unwrap();
    let mut sub = Subscription::from_checkpoint(checkpoint).unwrap();
    let update = SaveRequest {
        request_id: RequestId([8; 32]),
        content_id: content,
        command: Command::Edit {
            token: EditToken(unhex(read["edit_token"].as_str().unwrap()).unwrap()),
            fields: [(Field::Body, Value::Body("during long poll".into()))].into(),
        },
    };
    let (change, _) = tokio::join!(
        timeout(Duration::from_secs(3), reader.subscription_next(&mut sub)),
        async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            client.save(&update).await.unwrap();
        },
    );
    assert_eq!(
        change.unwrap().unwrap(),
        Notice::Changes {
            upserted: vec![content],
            removed: vec![]
        }
    );
    let latest = &sub.contents().unwrap()[&content];
    assert_eq!(latest["content"]["body"], "during long poll");
    assert_ne!(latest["edit_token"], frozen_edit);
    client
        .call(
            "change-grants",
            &serde_json::to_vec(&ChangeGrantsRequest {
                app_id: issued["app_id"].as_str().unwrap().into(),
                grants: GrantSpec {
                    read: Some(FilterSpec::All),
                    write: None,
                    manage_apps: false,
                    manage_nodes: false,
                },
                revoked: true,
            })
            .unwrap(),
        )
        .await
        .unwrap();
    assert!(matches!(
        reader.get(content).await,
        Err(harmion_client::Error::Http(401))
    ));
    assert!(matches!(
        reader.subscription_next(&mut sub).await,
        Err(harmion_client::Error::Http(401))
    ));
    assert!(sub.contents().is_none());
    let mut owner_sub = client
        .subscribe(query::Condition::Or(vec![]))
        .await
        .unwrap();
    client.unsubscribe(&mut owner_sub).await.unwrap();
    assert_eq!(
        client.subscription_next(&mut owner_sub).await.unwrap(),
        Notice::ResetRequired
    );
    let oversized = ContentId([10; 32]);
    let create = SaveRequest {
        request_id: RequestId([11; 32]),
        content_id: oversized,
        command: Command::Create {
            fields: [(Field::Body, Value::Body("\u{1}".repeat(800_000)))].into(),
        },
    };
    let created = client.save(&create).await.unwrap();
    let edit = SaveRequest {
        request_id: RequestId([12; 32]),
        content_id: oversized,
        command: Command::Edit {
            token: EditToken(unhex(created["view"]["edit_token"].as_str().unwrap()).unwrap()),
            fields: [(
                Field::Attribute {
                    namespace: "test".into(),
                    key: "large".into(),
                },
                Value::Attribute(AttributeValue::String("\u{2}".repeat(800_000))),
            )]
            .into(),
        },
    };
    let receipt = client.save(&edit).await.unwrap();
    assert_eq!(receipt["saved"], true);
    assert!(receipt["view"].is_null());
    assert_eq!(receipt["view_error"], "response_too_large");
    assert_eq!(client.save(&edit).await.unwrap(), receipt);
    assert!(matches!(
        client.get(oversized).await,
        Err(harmion_client::Error::Http(413))
    ));
    stop.send(true).unwrap();
    server.await.unwrap().unwrap();
}
