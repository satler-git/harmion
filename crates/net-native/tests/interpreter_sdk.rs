use harmion_client::{Client, ProviderClient, ProviderEvent};
use harmion_domain::*;
use harmion_net_native::{SharedStore, SystemHost, app, tls::TlsIdentity};
use harmion_node::{application, storage::Identity};
use harmion_protocol::interpreter::*;
use harmion_protocol::{self as protocol, *};
use harmion_store_sqlite::SqliteStore;
use serde_json::json;
use std::sync::Arc;
use tokio::{net::TcpListener, sync::watch};

#[tokio::test]
async fn provider_sdk_attaches_invokes_and_observes_timeout_cancel() {
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
        oauth_origin: None,
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
    let admin_client = Client::new(info.clone(), *admin.expose_bytes()).unwrap();
    let issue = |grants: GrantSpec| {
        let body = serde_json::to_vec(&IssueAppRequest {
            grants,
            expires_at: i64::MAX.to_string(),
        })
        .unwrap();
        let admin_client = &admin_client;
        async move { admin_client.call("issue-app", &body).await.unwrap() }
    };
    let provider = issue(GrantSpec {
        read: None,
        write: None,
        call: false,
        provide: true,
        provide_names: Vec::new(),
        manage_apps: false,
        manage_nodes: false,
    })
    .await;
    let provider_credential: [u8; 32] =
        protocol::unhex(provider["credential"].as_str().unwrap()).unwrap();
    let caller = issue(GrantSpec {
        read: Some(FilterSpec::All),
        write: None,
        call: true,
        provide: false,
        provide_names: Vec::new(),
        manage_apps: false,
        manage_nodes: false,
    })
    .await;
    let caller_credential: [u8; 32] =
        protocol::unhex(caller["credential"].as_str().unwrap()).unwrap();
    let provider_client = Client::new(info.clone(), provider_credential).unwrap();
    let caller_client = Client::new(info.clone(), caller_credential).unwrap();
    let registered = provider_client
        .call(
            "interpreter-register",
            &serde_json::to_vec(&ProviderRegistration {
                name: "ledger/balance".into(),
                contract_version: CONTRACT_VERSION,
                input_schema: json!({}),
                output_schema: json!({}),
                default_query: Some(DefaultQuery {
                    kinds: vec!["ledger".into()],
                }),
            })
            .unwrap(),
        )
        .await
        .unwrap();
    let provider_id: [u8; 32] =
        protocol::unhex(registered["provider_id"].as_str().unwrap()).unwrap();
    assert!(matches!(
        ProviderClient::connect(info.clone(), provider_credential, ProviderId(provider_id)).await,
        Err(harmion_client::Error::ProviderAttachRejected {
            code: RPC_NOT_APPROVED
        })
    ));
    admin_client
        .call(
            "interpreter-approve",
            &serde_json::to_vec(&json!({"provider_id": hex(&provider_id)})).unwrap(),
        )
        .await
        .unwrap();
    let mut sdk =
        ProviderClient::connect(info.clone(), provider_credential, ProviderId(provider_id))
            .await
            .unwrap();

    let content = ContentId([4; 32]);
    admin_client
        .save(&SaveRequest {
            request_id: RequestId([5; 32]),
            content_id: content,
            command: Command::Create {
                fields: [(Field::Body, Value::Body("one".into()))].into(),
            },
        })
        .await
        .unwrap();
    let view = caller_client.get(content).await.unwrap();
    let call = json!({
        "provider": "ledger/balance",
        "contents": [{"id": hex(&content.0), "token": view["edit_token"]}],
        "timeout_ms": 5000,
    });
    let body = serde_json::to_vec(&call).unwrap();
    let task = {
        let client = Client::new(info.clone(), caller_credential).unwrap();
        tokio::spawn(async move { client.call("interpreter-call", &body).await })
    };
    let event = sdk.next().await.unwrap();
    let ProviderEvent::Invocation { invocation, params } = event else {
        panic!("expected invocation");
    };
    assert_eq!(params.provider, "ledger/balance");
    assert_eq!(params.contents[0].content["body"], "one");
    sdk.succeed(invocation, json!({"balance": "1.00"}), None)
        .await
        .unwrap();
    let result = task.await.unwrap().unwrap();
    assert_eq!(result["status"], "ok");
    assert_eq!(result["data"]["balance"], "1.00");

    let stream_id = "ab".repeat(32);
    let started = caller_client
        .call(
            "interpreter-call-stream",
            &serde_json::to_vec(&json!({
                "frame": "start",
                "stream_id": "ab".repeat(32),
                "provider": "ledger/balance",
                "input": {"surface": "stream"},
                "timeout_ms": 5_000,
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(started["status"], "open");
    let accepted = caller_client
        .call(
            "interpreter-call-stream",
            &serde_json::to_vec(&json!({
                "frame": "chunk",
                "stream_id": "ab".repeat(32),
                "seq": 0,
                "contents": [{"id": hex(&content.0), "token": view["edit_token"]}],
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(accepted["status"], "accepted");
    let end_task = {
        let client = Client::new(info.clone(), caller_credential).unwrap();
        tokio::spawn(async move {
            client
                .call(
                    "interpreter-call-stream",
                    &serde_json::to_vec(&json!({"frame": "end", "stream_id": "ab".repeat(32)}))
                        .unwrap(),
                )
                .await
        })
    };
    let ProviderEvent::Invocation { invocation, params } = sdk.next().await.unwrap() else {
        panic!("expected streamed invocation");
    };
    assert_eq!(params.provider, "ledger/balance");
    assert_eq!(params.contents.len(), 1);
    assert_eq!(params.contents[0].content["body"], "one");
    assert!(params.stream.is_none());
    sdk.succeed(invocation, json!({"streamed": true}), None)
        .await
        .unwrap();
    let result = end_task.await.unwrap().unwrap();
    assert_eq!(result["stream_id"], stream_id);
    assert_eq!(result["status"], "ok");
    assert_eq!(result["data"]["streamed"], true);

    let replacement =
        ProviderClient::connect(info.clone(), provider_credential, ProviderId(provider_id))
            .await
            .unwrap();
    assert!(matches!(sdk.next().await.unwrap(), ProviderEvent::Closed));
    sdk.reconnect().await.unwrap();
    drop(replacement);

    let short = serde_json::to_vec(&json!({
        "provider": "ledger/balance",
        "contents": [{"id": hex(&content.0), "token": view["edit_token"]}],
        "timeout_ms": 150,
    }))
    .unwrap();
    let task = {
        let client = Client::new(info.clone(), caller_credential).unwrap();
        tokio::spawn(async move { client.call("interpreter-call", &short).await })
    };
    assert!(matches!(
        sdk.next().await.unwrap(),
        ProviderEvent::Invocation { .. }
    ));
    assert!(matches!(
        sdk.next().await.unwrap(),
        ProviderEvent::Cancel { .. }
    ));
    assert!(matches!(
        task.await.unwrap(),
        Err(harmion_client::Error::Http(504))
    ));
    sdk.close().await.unwrap();
    stop.send(true).unwrap();
    server.await.unwrap().unwrap();
}
