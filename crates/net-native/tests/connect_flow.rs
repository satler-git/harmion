use harmion_client::{Client, ProviderClient, ProviderEvent};
use harmion_domain::*;
use harmion_net_native::{SharedStore, SystemHost, app, tls::TlsIdentity};
use harmion_node::{application, oauth, storage::Identity};
use harmion_protocol::connect::{
    ConnectionApproval, ConnectionCompleteRequest, ConnectionOffer, ConnectionStatusRequest,
};
use harmion_protocol::interpreter::{
    CONTRACT_VERSION, DefaultQuery, ProviderId, ProviderRegistration,
};
use harmion_protocol::{self as protocol, *};
use harmion_store_sqlite::SqliteStore;
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tokio::{net::TcpListener, sync::watch};

const VERIFIER: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._~";

#[tokio::test]
async fn integrated_connection_approval_registers_provider_and_call_works() {
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
    let anonymous = Client::anonymous(info.clone()).unwrap();
    let offer = ConnectionOffer {
        version: 1,
        name: "AntiWebUI".into(),
        redirect_uri: "http://127.0.0.1:43123/callback".into(),
        web_origin: Some("http://127.0.0.1:43123".into()),
        client_id: None,
        challenge: oauth::pkce_challenge(VERIFIER).unwrap(),
        state: "state".into(),
        grants: GrantSpec {
            read: Some(FilterSpec::All),
            write: None,
            call: true,
            provide: true,
            provide_names: vec!["ledger/balance".into(), "ledger/audit".into()],
            manage_apps: false,
            manage_nodes: false,
        },
        provider: None,
        providers: vec![
            ProviderRegistration {
                name: "ledger/balance".into(),
                contract_version: CONTRACT_VERSION,
                input_schema: json!({"type": "object"}),
                output_schema: json!({"type": "object"}),
                default_query: Some(DefaultQuery {
                    kinds: vec!["ledger".into()],
                }),
            },
            ProviderRegistration {
                name: "ledger/audit".into(),
                contract_version: CONTRACT_VERSION,
                input_schema: json!({"type": "object"}),
                output_schema: json!({"type": "object"}),
                default_query: None,
            },
        ],
        provider_replace: false,
    };
    let started = anonymous.connect_start(&offer).await.unwrap();
    assert!(started.get("verification_url").is_none());
    let connection_id = started["connection_id"].as_str().unwrap().to_owned();
    let request_secret = started["request_secret"].as_str().unwrap().to_owned();
    let listed = admin_client.connect_list().await.unwrap();
    assert_eq!(listed["connections"][0]["name"], "AntiWebUI");
    assert_eq!(listed["connections"][0]["status"], "pending");
    assert_eq!(
        listed["connections"][0]["provider_names"],
        json!(["ledger/balance", "ledger/audit"])
    );
    let approved = admin_client
        .connect_approve(&ConnectionApproval {
            connection_id: connection_id.clone(),
            allow: true,
            replace: false,
        })
        .await
        .unwrap();
    assert_eq!(approved["status"], "approved");
    assert_eq!(
        approved["provider_names"],
        json!(["ledger/balance", "ledger/audit"])
    );
    assert_eq!(approved["provider_ids"].as_array().unwrap().len(), 2);
    let provider_id = approved["provider_id"].as_str().unwrap().to_owned();
    assert_eq!(approved["provider_ids"][0], provider_id);
    let status = anonymous
        .connect_status(&ConnectionStatusRequest {
            connection_id: connection_id.clone(),
            request_secret: request_secret.clone(),
        })
        .await
        .unwrap();
    assert_eq!(status["status"], "approved");
    assert_eq!(status["provider_name"], "ledger/balance");
    assert_eq!(
        status["provider_names"],
        json!(["ledger/balance", "ledger/audit"])
    );
    let complete_request = ConnectionCompleteRequest {
        connection_id,
        request_secret,
        code_verifier: VERIFIER.into(),
    };
    let completed = anonymous.connect_complete(&complete_request).await.unwrap();
    let retried = anonymous.connect_complete(&complete_request).await.unwrap();
    assert_eq!(retried["access_token"], completed["access_token"]);
    assert_eq!(retried["refresh_token"], completed["refresh_token"]);
    assert_eq!(completed["token_type"], "Bearer");
    assert_eq!(completed["provider"]["name"], "ledger/balance");
    assert!(completed["provider"].get("ticket").is_none());
    assert_eq!(
        completed["providers"],
        json!([
            {"provider_id": approved["provider_ids"][0], "name": "ledger/balance"},
            {"provider_id": approved["provider_ids"][1], "name": "ledger/audit"},
        ])
    );
    let access: [u8; 32] = protocol::unhex(completed["access_token"].as_str().unwrap()).unwrap();
    let app_client = Client::new(info.clone(), access).unwrap();
    let permissions = app_client.call("permissions", &[]).await.unwrap();
    assert_eq!(permissions["call"], true);
    assert_eq!(permissions["provide"], true);
    assert_eq!(
        permissions["provide_names"],
        json!(["ledger/audit", "ledger/balance"])
    );

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
    let view = app_client.get(content).await.unwrap();
    let mut provider = ProviderClient::connect(
        info.clone(),
        access,
        ProviderId(protocol::unhex(&provider_id).unwrap()),
    )
    .await
    .unwrap();
    let call = json!({
        "provider": "ledger/balance",
        "contents": [{"id": hex(&content.0), "token": view["edit_token"]}],
        "timeout_ms": 5000,
    });
    let body = serde_json::to_vec(&call).unwrap();
    let task = {
        let client = Client::new(info.clone(), access).unwrap();
        tokio::spawn(async move { client.call("interpreter-call", &body).await })
    };
    let event = provider.next().await.unwrap();
    let ProviderEvent::Invocation { invocation, params } = event else {
        panic!("expected invocation");
    };
    assert_eq!(params.contents[0].content["body"], "one");
    provider
        .succeed(invocation, json!({"balance": "1.00"}), None)
        .await
        .unwrap();
    let result = task.await.unwrap().unwrap();
    assert_eq!(result["status"], "ok");
    assert_eq!(result["data"]["balance"], "1.00");

    // Re-approving the same OAuth client without Provide must close the live
    // Provider session, not merely reject the next call.
    let client_id = completed["client_id"].as_str().unwrap().to_owned();
    let reduced = ConnectionOffer {
        version: 1,
        name: "AntiWebUI".into(),
        redirect_uri: "http://127.0.0.1:43123/callback".into(),
        web_origin: Some("http://127.0.0.1:43123".into()),
        client_id: Some(client_id),
        challenge: oauth::pkce_challenge(VERIFIER).unwrap(),
        state: "state-2".into(),
        grants: GrantSpec {
            read: Some(FilterSpec::All),
            write: None,
            call: false,
            provide: false,
            provide_names: Vec::new(),
            manage_apps: false,
            manage_nodes: false,
        },
        provider: None,
        providers: Vec::new(),
        provider_replace: false,
    };
    let started = anonymous.connect_start(&reduced).await.unwrap();
    admin_client
        .connect_approve(&ConnectionApproval {
            connection_id: started["connection_id"].as_str().unwrap().into(),
            allow: true,
            replace: false,
        })
        .await
        .unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(5), provider.next())
            .await
            .unwrap()
            .unwrap(),
        ProviderEvent::Closed
    ));
    tokio::time::sleep(Duration::from_millis(50)).await;
    stop.send(true).unwrap();
    server.await.unwrap().unwrap();
}
