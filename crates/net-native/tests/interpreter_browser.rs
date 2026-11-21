use base64::Engine;
use bytes::Bytes;
use harmion_client::Client;
use harmion_domain::*;
use harmion_local_web::BrowserTls;
use harmion_net_native::{SharedStore, SystemHost, app, oauth_browser, tls::TlsIdentity};
use harmion_node::{access::Grants, application, storage::Identity};
use harmion_protocol::interpreter::*;
use harmion_protocol::websocket::{Frame, Opcode, encode_frame, parse_frame};
use harmion_protocol::{self as protocol, *};
use harmion_store_sqlite::SqliteStore;
use http_body_util::Full;
use hyper::{Request, header, upgrade::Upgraded};
use hyper_util::rt::TokioIo;
use rustls::pki_types::ServerName;
use serde_json::{Value as Json, json};
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::watch,
    time::timeout,
};
use tokio_rustls::TlsConnector;

struct BrowserProvider {
    io: TokioIo<Upgraded>,
    buffer: Vec<u8>,
    mask: [u8; 4],
    connection: tokio::task::JoinHandle<()>,
}
impl Drop for BrowserProvider {
    fn drop(&mut self) {
        self.connection.abort();
    }
}
impl BrowserProvider {
    async fn connect(url: &str, root: &[u8], provider_id: &str) -> Self {
        let parsed = url::Url::parse(url).unwrap();
        let host = parsed.host_str().unwrap().to_owned();
        let port = parsed.port().unwrap();
        let query = parsed.query().unwrap_or("").to_owned();
        let path = format!("{}?{query}", parsed.path());
        let tcp = TcpStream::connect((host.as_str(), port)).await.unwrap();
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(rustls::pki_types::CertificateDer::from(root.to_vec()))
            .unwrap();
        let config = rustls::ClientConfig::builder_with_provider(
            rustls::crypto::ring::default_provider().into(),
        )
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
        let tls = TlsConnector::from(Arc::new(config))
            .connect(ServerName::try_from(host.clone()).unwrap(), tcp)
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
            .uri(path)
            .header(header::HOST, format!("{host}:{port}"))
            .header(header::UPGRADE, "websocket")
            .header(header::CONNECTION, "Upgrade")
            .header(header::SEC_WEBSOCKET_VERSION, "13")
            .header(header::SEC_WEBSOCKET_KEY, key)
            .body(Full::new(Bytes::new()))
            .unwrap();
        let response = sender.send_request(request).await.unwrap();
        assert_eq!(response.status(), hyper::StatusCode::SWITCHING_PROTOCOLS);
        let upgraded = hyper::upgrade::on(response).await.unwrap();
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
    async fn read_json(&mut self) -> Json {
        timeout(Duration::from_secs(5), async {
            loop {
                if let Some((frame, used)) = parse_frame(&self.buffer, false).unwrap() {
                    self.buffer.drain(..used);
                    match frame.opcode {
                        Opcode::Text => return serde_json::from_slice(&frame.payload).unwrap(),
                        Opcode::Ping => {
                            let encoded = encode_frame(
                                &Frame::new(true, Opcode::Pong, frame.payload),
                                Some(self.mask),
                            )
                            .unwrap();
                            self.io.write_all(&encoded).await.unwrap();
                        }
                        Opcode::Pong => {}
                        Opcode::Close => panic!("provider closed"),
                        _ => panic!("unexpected frame"),
                    }
                } else {
                    let mut chunk = [0u8; 8192];
                    let read = self.io.read(&mut chunk).await.unwrap();
                    assert_ne!(read, 0);
                    self.buffer.extend_from_slice(&chunk[..read]);
                }
            }
        })
        .await
        .unwrap()
    }
}

#[tokio::test]
async fn browser_ticket_connects_provider_over_oauth_origin() {
    let identity = Identity {
        cluster: ClusterId([1; 32]),
        node: NodeId([2; 32]),
        epoch: LogEpoch([3; 32]),
    };
    let mut db = SqliteStore::open(":memory:", identity).unwrap();
    let (_, admin) = application::bootstrap_admin(&mut db, &mut SystemHost, i64::MAX).unwrap();
    let (_, provider_credential) = application::issue_app(
        &mut db,
        &mut SystemHost,
        &admin,
        Grants {
            provide: true,
            ..Grants::default()
        },
        i64::MAX,
    )
    .unwrap();
    let (_, caller_credential) = application::issue_app(
        &mut db,
        &mut SystemHost,
        &admin,
        Grants {
            read: Some(harmion_node::access::Filter::All),
            call: true,
            ..Grants::default()
        },
        i64::MAX,
    )
    .unwrap();
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
    let (oauth_stop, oauth_receiver) = watch::channel(false);
    let browser = BrowserTls::generate("Admin").unwrap();
    let browser_root = browser.root_der.clone();
    let oauth_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let oauth_origin =
        oauth_browser::configure(&store, oauth_listener.local_addr().unwrap()).unwrap();
    assert!(oauth_origin.starts_with("https://127.0.0.1:"));
    let server = tokio::spawn(app::serve(
        listener,
        store.clone(),
        tls,
        registration,
        receiver,
    ));
    let oauth_server = tokio::spawn(oauth_browser::serve(
        oauth_listener,
        store.clone(),
        browser,
        oauth_receiver,
    ));

    let admin_client = Client::new(info.clone(), *admin.expose_bytes()).unwrap();
    let provider_client = Client::new(info.clone(), *provider_credential.expose_bytes()).unwrap();
    let caller_client = Client::new(info.clone(), *caller_credential.expose_bytes()).unwrap();
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
    admin_client
        .call(
            "interpreter-approve",
            &serde_json::to_vec(&json!({"provider_id": provider_id})).unwrap(),
        )
        .await
        .unwrap();
    let ticket = provider_client
        .interpreter_ticket(ProviderId(protocol::unhex(&provider_id).unwrap()))
        .await
        .unwrap();
    let url = ticket["websocket_url"].as_str().unwrap().to_owned();
    let mut provider = BrowserProvider::connect(&url, &browser_root, &provider_id).await;
    let attached = provider.read_json().await;
    assert_eq!(attached["result"]["ok"], true);

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
    let call = serde_json::to_vec(&json!({
        "provider": "ledger/balance",
        "contents": [{"id": hex(&content.0), "token": view["edit_token"]}],
        "timeout_ms": 5000,
    }))
    .unwrap();
    let task = {
        let client = Client::new(info.clone(), *caller_credential.expose_bytes()).unwrap();
        tokio::spawn(async move { client.call("interpreter-call", &call).await })
    };
    let invoke = provider.read_json().await;
    assert_eq!(invoke["method"], RPC_INVOKE);
    assert_eq!(invoke["params"]["contents"][0]["content"]["body"], "one");
    provider
        .send_json(&rpc_result(
            &invoke["id"],
            json!({"status": "ok", "data": {"balance": "1.00"}}),
        ))
        .await;
    let result = task.await.unwrap().unwrap();
    assert_eq!(result["status"], "ok");
    assert_eq!(result["data"]["balance"], "1.00");
    drop(provider);
    stop.send(true).unwrap();
    oauth_stop.send(true).unwrap();
    server.await.unwrap().unwrap();
    oauth_server.await.unwrap().unwrap();
}
