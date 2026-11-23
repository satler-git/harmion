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

async fn stream_frame(client: &Client, frame: Json) -> Result<Json, harmion_client::Error> {
    client
        .call(
            "interpreter-call-stream",
            &serde_json::to_vec(&frame).unwrap(),
        )
        .await
}

#[tokio::test]
async fn streaming_input_is_ordered_bounded_and_compatible_with_legacy_calls() {
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
                grants: provider_grants,
                expires_at: i64::MAX.to_string(),
            })
            .unwrap(),
        )
        .await
        .unwrap();
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
        name: "wiki/all".into(),
        contract_version: CONTRACT_VERSION,
        input_schema: json!({"type": "object"}),
        output_schema: json!({"type": "object"}),
        default_query: Some(DefaultQuery {
            kinds: vec!["wiki".into()],
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
    admin_client
        .call(
            "interpreter-approve",
            &serde_json::to_vec(&json!({"provider_id": provider_id})).unwrap(),
        )
        .await
        .unwrap();
    let (mut provider, attached) = connect_provider(
        &endpoint,
        tls.certificate(),
        &provider_credential,
        &provider_id,
    )
    .await;
    assert_eq!(attached["result"]["ok"], true);

    // 65 items exercise the removed 64-item cap. The last item is large enough
    // that the streaming planner must split it into a separate Provider chunk.
    let mut selections = Vec::new();
    for i in 1..=65u8 {
        let content = ContentId([i; 32]);
        let body = if i == 65 {
            "x".repeat(800 * 1024)
        } else {
            format!("body-{i}")
        };
        admin_client
            .save(&SaveRequest {
                request_id: RequestId([i; 32]),
                content_id: content,
                command: Command::Create {
                    fields: [(Field::Body, Value::Body(body))].into(),
                },
            })
            .await
            .unwrap();
        let view = caller_client.get(content).await.unwrap();
        selections.push(json!({"id": hex(&content.0), "token": view["edit_token"]}));
    }

    // Legacy non-streaming compatibility: one invoke contains all 65 items and
    // carries no stream descriptor.
    let legacy = {
        let body = serde_json::to_vec(&json!({
            "provider": "wiki/all",
            "input": {"surface": "legacy"},
            "contents": selections,
            "timeout_ms": 5_000,
        }))
        .unwrap();
        let client = Client::new(info.clone(), caller_credential).unwrap();
        tokio::spawn(async move { client.call("interpreter-call", &body).await })
    };
    let invoke = provider.read_json().await;
    assert_eq!(invoke["method"], RPC_INVOKE);
    assert!(invoke["params"].get("stream").is_none());
    assert_eq!(invoke["params"]["contents"].as_array().unwrap().len(), 65);
    assert_eq!(invoke["params"]["contents"][0]["content"]["body"], "body-1");
    provider
        .send_result(
            &invoke["id"],
            json!({"status": "ok", "data": {"legacy": true}}),
        )
        .await;
    let result = legacy.await.unwrap().unwrap();
    assert_eq!(result["status"], "ok");
    assert_eq!(result["data"]["legacy"], true);

    // Streamed call: two app chunks become ordered Provider chunk frames.
    let stream_id = "aa".repeat(32);
    let started = stream_frame(
        &caller_client,
        json!({
            "frame": "start",
            "stream_id": "aa".repeat(32),
            "provider": "wiki/all",
            "input": {"surface": "stream"},
            "timeout_ms": 5_000,
        }),
    )
    .await
    .unwrap();
    assert_eq!(started["status"], "open");
    assert_eq!(started["limit_bytes"], MAX_STREAM_INPUT_BYTES);
    let accepted = stream_frame(
        &caller_client,
        json!({
            "frame": "chunk",
            "stream_id": "aa".repeat(32),
            "seq": 0,
            "contents": selections[..40],
        }),
    )
    .await
    .unwrap();
    assert_eq!(accepted["status"], "accepted");
    assert_eq!(accepted["next_seq"], 1);
    assert_eq!(accepted["accepted_contents"], 40);
    let accepted = stream_frame(
        &caller_client,
        json!({
            "frame": "chunk",
            "stream_id": "aa".repeat(32),
            "seq": 1,
            "contents": selections[40..],
        }),
    )
    .await
    .unwrap();
    assert_eq!(accepted["status"], "accepted");
    assert_eq!(accepted["next_seq"], 2);
    assert_eq!(accepted["accepted_contents"], 65);

    let end_task = {
        let client = Client::new(info.clone(), caller_credential).unwrap();
        let frame = json!({"frame": "end", "stream_id": "aa".repeat(32)});
        tokio::spawn(async move { stream_frame(&client, frame).await })
    };
    let start = provider.read_json().await;
    assert_eq!(start["method"], RPC_INVOKE);
    assert_eq!(start["params"]["contents"], json!([]));
    assert_eq!(start["params"]["provider"], "wiki/all");
    assert_eq!(start["params"]["stream"]["version"], STREAM_INPUT_VERSION);
    assert_eq!(start["params"]["stream"]["chunks"], 2);
    assert_eq!(start["params"]["stream"]["total_contents"], 65);
    let chunk0 = provider.read_json().await;
    assert_eq!(chunk0["method"], RPC_INVOKE_CHUNK);
    assert_eq!(chunk0["params"]["seq"], 0);
    assert_eq!(chunk0["params"]["contents"].as_array().unwrap().len(), 64);
    assert_eq!(chunk0["params"]["contents"][0]["content"]["body"], "body-1");
    let chunk1 = provider.read_json().await;
    assert_eq!(chunk1["method"], RPC_INVOKE_CHUNK);
    assert_eq!(chunk1["params"]["seq"], 1);
    assert_eq!(chunk1["params"]["contents"].as_array().unwrap().len(), 1);
    assert_eq!(
        chunk1["params"]["contents"][0]["content"]["body"]
            .as_str()
            .unwrap()
            .len(),
        800 * 1024
    );
    let end = provider.read_json().await;
    assert_eq!(end["method"], RPC_INVOKE_END);
    assert_eq!(end["params"]["seq"], 2);
    assert_eq!(end["params"]["total_contents"], 65);
    assert_eq!(
        end["params"]["total_bytes"],
        start["params"]["stream"]["total_bytes"]
    );
    provider
        .send_result(
            &start["id"],
            json!({"status": "ok", "data": {"streamed": true}, "display": {"media_type": "text/plain", "text": "ok"}}),
        )
        .await;
    let result = end_task.await.unwrap().unwrap();
    assert_eq!(result["stream_id"], stream_id);
    assert_eq!(result["status"], "ok");
    assert_eq!(result["data"]["streamed"], true);
    assert_eq!(result["display"]["text"], "ok");

    // Ordering, duplicate, unknown-stream, and malformed-frame errors are
    // explicit and never reach the Provider.
    let bad_seq = stream_frame(
        &caller_client,
        json!({
            "frame": "start",
            "stream_id": "bb".repeat(32),
            "provider": "wiki/all",
            "timeout_ms": 5_000,
        }),
    )
    .await
    .unwrap();
    assert_eq!(bad_seq["status"], "open");
    let error = stream_frame(
        &caller_client,
        json!({
            "frame": "chunk",
            "stream_id": "bb".repeat(32),
            "seq": 1,
            "contents": selections[..1],
        }),
    )
    .await
    .unwrap();
    assert_eq!(error["status"], "error");
    assert_eq!(error["error"]["code"], "stream_sequence");
    assert_eq!(error["error"]["expected_seq"], 0);
    let accepted = stream_frame(
        &caller_client,
        json!({
            "frame": "chunk",
            "stream_id": "bb".repeat(32),
            "seq": 0,
            "contents": selections[..1],
        }),
    )
    .await
    .unwrap();
    assert_eq!(accepted["status"], "accepted");
    let error = stream_frame(
        &caller_client,
        json!({
            "frame": "chunk",
            "stream_id": "bb".repeat(32),
            "seq": 1,
            "contents": selections[..1],
        }),
    )
    .await
    .unwrap();
    assert_eq!(error["error"]["code"], "stream_duplicate_content");
    let canceled = stream_frame(
        &caller_client,
        json!({"frame": "cancel", "stream_id": "bb".repeat(32)}),
    )
    .await
    .unwrap();
    assert_eq!(canceled["status"], "canceled");
    assert_eq!(canceled["canceled"], true);
    assert!(
        timeout(Duration::from_millis(150), provider.read_json())
            .await
            .is_err(),
        "canceled input must not reach the Provider"
    );
    let ended = stream_frame(
        &caller_client,
        json!({"frame": "end", "stream_id": "bb".repeat(32)}),
    )
    .await
    .unwrap();
    assert_eq!(ended["error"]["code"], "stream_not_found");
    assert!(matches!(
        caller_client.call("interpreter-call-stream", b"{").await,
        Err(harmion_client::Error::Http(400))
    ));

    // Invocation timeout applies after the upload ends; the Provider receives
    // the normal cancel notification.
    stream_frame(
        &caller_client,
        json!({
            "frame": "start",
            "stream_id": "cc".repeat(32),
            "provider": "wiki/all",
            "timeout_ms": 150,
        }),
    )
    .await
    .unwrap();
    stream_frame(
        &caller_client,
        json!({
            "frame": "chunk",
            "stream_id": "cc".repeat(32),
            "seq": 0,
            "contents": selections[..1],
        }),
    )
    .await
    .unwrap();
    let timeout_task = {
        let client = Client::new(info.clone(), caller_credential).unwrap();
        let frame = json!({"frame": "end", "stream_id": "cc".repeat(32)});
        tokio::spawn(async move { stream_frame(&client, frame).await })
    };
    let start = provider.read_json().await;
    assert_eq!(start["params"]["stream"]["chunks"], 1);
    assert_eq!(provider.read_json().await["method"], RPC_INVOKE_CHUNK);
    assert_eq!(provider.read_json().await["method"], RPC_INVOKE_END);
    assert!(matches!(
        timeout_task.await.unwrap(),
        Err(harmion_client::Error::Http(504))
    ));
    let cancel = provider.read_json().await;
    assert_eq!(cancel["method"], RPC_CANCEL);
    assert_eq!(cancel["params"]["invocation_id"], start["id"]);

    stop.send(true).unwrap();
    server.await.unwrap().unwrap();
}
