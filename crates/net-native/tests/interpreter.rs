use base64::Engine;
use bytes::Bytes;
use harmion_client::Client;
use harmion_domain::*;
use harmion_net_native::{SharedStore, SystemHost, app, tls::TlsIdentity};
use harmion_node::{application, storage::Identity};
use harmion_protocol::interpreter::*;
use harmion_protocol::websocket::{Frame, Opcode, encode_frame, parse_frame};
use harmion_protocol::{self as protocol, *};
use harmion_store_sqlite::SqliteStore;
use http_body_util::Full;
use hyper::{
    Request, header,
    upgrade::{Upgraded, on},
};
use hyper_util::rt::TokioIo;
use serde_json::{Value as Json, json};
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::watch,
    time::timeout,
};
use tokio_rustls::TlsConnector;

struct TestProvider {
    io: TokioIo<Upgraded>,
    buffer: Vec<u8>,
    mask: [u8; 4],
    connection: tokio::task::JoinHandle<()>,
}
impl Drop for TestProvider {
    fn drop(&mut self) {
        self.connection.abort();
    }
}
impl TestProvider {
    async fn connect(
        endpoint: &str,
        certificate: &[u8],
        credential: &[u8; 32],
        provider_id: &str,
    ) -> Self {
        let tcp = TcpStream::connect(endpoint).await.unwrap();
        let tls = TlsConnector::from(Arc::new(
            harmion_net_native::tls::app_client_config(certificate).unwrap(),
        ))
        .connect(harmion_protocol::TLS_NAME.try_into().unwrap(), tcp)
        .await
        .unwrap();
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(tls))
            .await
            .unwrap();
        let connection = tokio::spawn(async move {
            let _ = connection.with_upgrades().await;
        });
        let key = base64::engine::general_purpose::STANDARD.encode([7u8; 16]);
        let request = Request::builder()
            .method(hyper::Method::GET)
            .uri("/provider/v1/connect")
            .header(header::HOST, harmion_protocol::TLS_NAME)
            .header(header::AUTHORIZATION, format!("Bearer {}", hex(credential)))
            .header(header::UPGRADE, "websocket")
            .header(header::CONNECTION, "Upgrade")
            .header(header::SEC_WEBSOCKET_VERSION, "13")
            .header(header::SEC_WEBSOCKET_KEY, key)
            .body(Full::new(Bytes::new()))
            .unwrap();
        let response = sender.send_request(request).await.unwrap();
        assert_eq!(response.status(), hyper::StatusCode::SWITCHING_PROTOCOLS);
        let upgraded = on(response).await.unwrap();
        let mut provider = Self {
            io: TokioIo::new(upgraded),
            buffer: Vec::new(),
            mask: [9, 8, 7, 6],
            connection,
        };
        provider
            .send_json(&rpc_request(
                &json!("attach"),
                RPC_ATTACH,
                json!({"provider_id": provider_id}),
            ))
            .await;
        provider
    }
    async fn send_json(&mut self, value: &Json) {
        let text = serde_json::to_string(value).unwrap();
        let encoded = encode_frame(&Frame::text(&text), Some(self.mask)).unwrap();
        self.io.write_all(&encoded).await.unwrap();
        self.io.flush().await.unwrap();
    }
    async fn send_result(&mut self, id: &Json, result: Json) {
        self.send_json(&rpc_result(id, result)).await;
    }
    async fn read_close(&mut self) {
        timeout(Duration::from_secs(5), async {
            loop {
                if let Some((frame, used)) = parse_frame(&self.buffer, false).unwrap() {
                    self.buffer.drain(..used);
                    match frame.opcode {
                        Opcode::Close => return,
                        Opcode::Ping => {
                            let encoded = encode_frame(
                                &Frame::new(true, Opcode::Pong, frame.payload),
                                Some(self.mask),
                            )
                            .unwrap();
                            self.io.write_all(&encoded).await.unwrap();
                        }
                        _ => {}
                    }
                } else {
                    let mut chunk = [0u8; 8192];
                    let read = self.io.read(&mut chunk).await.unwrap();
                    assert_ne!(read, 0, "provider connection closed without a close frame");
                    self.buffer.extend_from_slice(&chunk[..read]);
                }
            }
        })
        .await
        .unwrap();
    }
    async fn read_json(&mut self) -> Json {
        timeout(Duration::from_secs(5), async {
            loop {
                if let Some((frame, used)) = parse_frame(&self.buffer, false).unwrap() {
                    self.buffer.drain(..used);
                    match frame.opcode {
                        Opcode::Text => {
                            return serde_json::from_slice(&frame.payload).unwrap();
                        }
                        Opcode::Ping => {
                            let encoded = encode_frame(
                                &Frame::new(true, Opcode::Pong, frame.payload),
                                Some(self.mask),
                            )
                            .unwrap();
                            self.io.write_all(&encoded).await.unwrap();
                        }
                        Opcode::Pong => {}
                        Opcode::Close => panic!("provider received close"),
                        _ => panic!("unexpected provider frame"),
                    }
                } else {
                    let mut chunk = [0u8; 8192];
                    let read = self.io.read(&mut chunk).await.unwrap();
                    assert_ne!(read, 0, "provider connection closed");
                    self.buffer.extend_from_slice(&chunk[..read]);
                }
            }
        })
        .await
        .unwrap()
    }
}

async fn connect_provider(
    endpoint: &str,
    certificate: &[u8],
    credential: &[u8; 32],
    provider_id: &str,
) -> (TestProvider, Json) {
    let mut provider = TestProvider::connect(endpoint, certificate, credential, provider_id).await;
    let attached = provider.read_json().await;
    (provider, attached)
}

#[tokio::test]
async fn provider_attach_call_timeout_cancel_and_disconnect() {
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
            endpoint: endpoint.clone(),
        },
    };
    let (stop, receiver) = watch::channel(false);
    let server = tokio::spawn(app::serve(
        listener,
        store.clone(),
        tls.clone(),
        registration,
        receiver,
    ));
    let admin_client = Client::new(info.clone(), *admin.expose_bytes()).unwrap();
    let provider_grants = GrantSpec {
        read: None,
        write: None,
        call: false,
        provide: true,
        provide_names: Vec::new(),
        manage_apps: false,
        manage_nodes: false,
    };
    let issued = admin_client
        .call(
            "issue-app",
            &serde_json::to_vec(&IssueAppRequest {
                grants: provider_grants.clone(),
                expires_at: i64::MAX.to_string(),
            })
            .unwrap(),
        )
        .await
        .unwrap();
    let provider_id_app: [u8; 32] = protocol::unhex(issued["app_id"].as_str().unwrap()).unwrap();
    let provider_credential: [u8; 32] =
        protocol::unhex(issued["credential"].as_str().unwrap()).unwrap();
    let provider_client = Client::new(info.clone(), provider_credential).unwrap();
    let caller_issued = admin_client
        .call(
            "issue-app",
            &serde_json::to_vec(&IssueAppRequest {
                grants: GrantSpec {
                    read: Some(FilterSpec::All),
                    write: None,
                    call: true,
                    provide: false,
                    provide_names: Vec::new(),
                    manage_apps: false,
                    manage_nodes: false,
                },
                expires_at: i64::MAX.to_string(),
            })
            .unwrap(),
        )
        .await
        .unwrap();
    let caller_credential: [u8; 32] =
        protocol::unhex(caller_issued["credential"].as_str().unwrap()).unwrap();
    let caller_client = Client::new(info.clone(), caller_credential).unwrap();

    let manifest = ProviderRegistration {
        name: "ledger/balance".into(),
        contract_version: CONTRACT_VERSION,
        input_schema: json!({"type": "object"}),
        output_schema: json!({"type": "object"}),
        default_query: Some(DefaultQuery {
            kinds: vec!["ledger".into()],
        }),
    };
    let registered = provider_client
        .call(
            "interpreter-register",
            &serde_json::to_vec(&manifest).unwrap(),
        )
        .await
        .unwrap();
    let provider_id = registered["provider_id"].as_str().unwrap().to_owned();
    assert_eq!(registered["status"], "pending");
    let (provider, rejected) = connect_provider(
        &endpoint,
        tls.certificate(),
        &provider_credential,
        &provider_id,
    )
    .await;
    assert_eq!(rejected["error"]["code"], RPC_NOT_APPROVED);
    drop(provider);

    admin_client
        .call(
            "interpreter-approve",
            &serde_json::to_vec(&json!({"provider_id": provider_id})).unwrap(),
        )
        .await
        .unwrap();
    let described = caller_client
        .call(
            "interpreter-describe",
            &protocol::unhex::<32>(&provider_id).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(described["name"], "ledger/balance");
    assert_eq!(described["connected"], false);
    assert_eq!(described["default_query"]["kinds"], json!(["ledger"]));

    let (mut provider, attached) = connect_provider(
        &endpoint,
        tls.certificate(),
        &provider_credential,
        &provider_id,
    )
    .await;
    assert_eq!(attached["result"]["ok"], true);
    provider
        .send_json(&rpc_request(
            &json!("attach-2"),
            RPC_ATTACH,
            json!({"provider_id": provider_id}),
        ))
        .await;
    let duplicate = provider.read_json().await;
    assert_eq!(duplicate["error"]["code"], RPC_ALREADY_ATTACHED);
    let other_issued = admin_client
        .call(
            "issue-app",
            &serde_json::to_vec(&IssueAppRequest {
                grants: provider_grants.clone(),
                expires_at: i64::MAX.to_string(),
            })
            .unwrap(),
        )
        .await
        .unwrap();
    let other_credential: [u8; 32] =
        protocol::unhex(other_issued["credential"].as_str().unwrap()).unwrap();
    let (other_provider, mismatch) = connect_provider(
        &endpoint,
        tls.certificate(),
        &other_credential,
        &provider_id,
    )
    .await;
    assert_eq!(mismatch["error"]["code"], RPC_PROVIDER_MISMATCH);
    drop(other_provider);
    let listed = caller_client.call("interpreter-list", &[]).await.unwrap();
    assert_eq!(listed["providers"][0]["connected"], true);
    assert_eq!(listed["providers"][0]["name"], "ledger/balance");

    let content = ContentId([4; 32]);
    admin_client
        .save(&SaveRequest {
            request_id: RequestId([5; 32]),
            content_id: content,
            command: Command::Create {
                fields: [
                    (Field::Body, Value::Body("one".into())),
                    (Field::Kind("ledger".into()), Value::Kind(true)),
                ]
                .into(),
            },
        })
        .await
        .unwrap();
    let caller_view = caller_client.get(content).await.unwrap();
    let pinned = caller_view["edit_token"].as_str().unwrap().to_owned();
    let admin_view = admin_client.get(content).await.unwrap();
    admin_client
        .save(&SaveRequest {
            request_id: RequestId([6; 32]),
            content_id: content,
            command: Command::Edit {
                token: EditToken(
                    protocol::unhex(admin_view["edit_token"].as_str().unwrap()).unwrap(),
                ),
                fields: [(Field::Body, Value::Body("two".into()))].into(),
            },
        })
        .await
        .unwrap();
    let call = json!({
        "provider": "ledger/balance",
        "input": {"currency": "JPY"},
        "contents": [{"id": hex(&content.0), "token": pinned}],
        "timeout_ms": 5000,
    });
    let body = serde_json::to_vec(&call).unwrap();
    let call_client = Client::new(info.clone(), caller_credential).unwrap();
    let call_task = {
        let body = body.clone();
        tokio::spawn(async move { call_client.call("interpreter-call", &body).await })
    };
    let invoke = provider.read_json().await;
    assert_eq!(invoke["method"], RPC_INVOKE);
    assert_eq!(invoke["params"]["contents"][0]["content"]["body"], "one");
    assert_eq!(invoke["params"]["input"]["currency"], "JPY");
    provider
        .send_result(
            &invoke["id"],
            json!({"status": "ok", "data": {"balance": "12.30"}, "display": {"media_type": "text/plain", "text": "12.30"}}),
        )
        .await;
    let result = call_task.await.unwrap().unwrap();
    assert_eq!(result["status"], "ok");
    assert_eq!(result["data"]["balance"], "12.30");
    assert_eq!(result["display"]["text"], "12.30");

    let call_task = {
        let body = body.clone();
        let client = Client::new(info.clone(), caller_credential).unwrap();
        tokio::spawn(async move { client.call("interpreter-call", &body).await })
    };
    let invoke = provider.read_json().await;
    provider
        .send_result(
            &invoke["id"],
            json!({"status": "error", "error": {"code": "unsupported", "message": "no rate"}}),
        )
        .await;
    let result = call_task.await.unwrap().unwrap();
    assert_eq!(result["status"], "error");
    assert_eq!(result["error"]["code"], "unsupported");

    let short = json!({
        "provider": "ledger/balance",
        "contents": [{"id": hex(&content.0), "token": caller_view["edit_token"]}],
        "timeout_ms": 150,
    });
    let short_body = serde_json::to_vec(&short).unwrap();
    let call_task = {
        let client = Client::new(info.clone(), caller_credential).unwrap();
        tokio::spawn(async move { client.call("interpreter-call", &short_body).await })
    };
    let invoke = provider.read_json().await;
    let timed_out = call_task.await.unwrap();
    assert!(matches!(timed_out, Err(harmion_client::Error::Http(504))));
    let cancel = provider.read_json().await;
    assert_eq!(cancel["method"], RPC_CANCEL);
    assert_eq!(cancel["params"]["invocation_id"], invoke["id"]);

    let call_task = {
        let client = Client::new(info.clone(), caller_credential).unwrap();
        let body = body.clone();
        tokio::spawn(async move { client.call("interpreter-call", &body).await })
    };
    let invoke = provider.read_json().await;
    let invocation = protocol::unhex::<32>(invoke["id"].as_str().unwrap()).unwrap();
    let canceled = caller_client
        .call("interpreter-cancel", &invocation)
        .await
        .unwrap();
    assert_eq!(canceled["canceled"], true);
    assert!(matches!(
        call_task.await.unwrap(),
        Err(harmion_client::Error::Http(409))
    ));
    let cancel = provider.read_json().await;
    assert_eq!(cancel["method"], RPC_CANCEL);
    assert_eq!(cancel["params"]["invocation_id"], invoke["id"]);

    // Revoking Provide while a call is in flight closes the session and fails
    // the waiter instead of leaving it until timeout.
    let call_task = {
        let client = Client::new(info.clone(), caller_credential).unwrap();
        let body = body.clone();
        tokio::spawn(async move { client.call("interpreter-call", &body).await })
    };
    let _invoke = provider.read_json().await;
    admin_client
        .call(
            "change-grants",
            &serde_json::to_vec(&ChangeGrantsRequest {
                app_id: hex(&provider_id_app),
                grants: GrantSpec {
                    read: None,
                    write: None,
                    call: false,
                    provide: false,
                    provide_names: Vec::new(),
                    manage_apps: false,
                    manage_nodes: false,
                },
                revoked: false,
                expires_at: None,
            })
            .unwrap(),
        )
        .await
        .unwrap();
    assert!(matches!(
        call_task.await.unwrap(),
        Err(harmion_client::Error::Http(503))
    ));
    provider.read_close().await;
    drop(provider);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(matches!(
        caller_client.call("interpreter-call", &body).await,
        Err(harmion_client::Error::Http(503))
    ));

    let (provider, attached) = connect_provider(
        &endpoint,
        tls.certificate(),
        &provider_credential,
        &provider_id,
    )
    .await;
    assert_eq!(attached["error"]["code"], RPC_NOT_APPROVED);
    drop(provider);

    stop.send(true).unwrap();
    server.await.unwrap().unwrap();
}
