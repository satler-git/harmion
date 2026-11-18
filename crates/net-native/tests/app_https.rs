use harmion_client::Client;
use harmion_domain::*;
use harmion_net_native::{SharedStore, SystemHost, app, tls::TlsIdentity};
use harmion_node::{application, storage::Identity};
use harmion_protocol::*;
use harmion_store_sqlite::SqliteStore;
use std::sync::Arc;
use tokio::{net::TcpListener, sync::watch};

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
    let server = tokio::spawn(app::serve(listener, store, tls, registration, receiver));
    let client = Client::new(info.clone(), *admin.expose_bytes()).unwrap();
    let capabilities = client.call("capabilities", &[]).await.unwrap();
    assert_eq!(capabilities["oauth"], false);
    let content = ContentId([4; 32]);
    let request = SaveRequest {
        request_id: RequestId([5; 32]),
        content_id: content,
        command: Command::Create {
            fields: [(Field::Body, Value::Body("initial".into()))].into(),
        },
    };
    let saved = client.save(&request).await.unwrap();
    assert_eq!(saved, client.save(&request).await.unwrap());
    let read = client.get(content).await.unwrap();
    assert_eq!(read["content"]["body"], "initial");
    assert!(read.get("basis").is_none());
    assert!(read["content"].get("root").is_none());
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
    assert_eq!(
        reader.get(content).await.unwrap()["content"]["body"],
        "initial"
    );
    assert!(matches!(
        reader.save(&changed).await,
        Err(harmion_client::Error::Http(403))
    ));
    assert!(matches!(
        reader.call("sync", &[7; 32]).await,
        Err(harmion_client::Error::Http(403))
    ));
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
    stop.send(true).unwrap();
    server.await.unwrap().unwrap();
}
