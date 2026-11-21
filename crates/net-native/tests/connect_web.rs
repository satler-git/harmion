use bytes::Bytes;
use harmion_client::Client;
use harmion_domain::*;
use harmion_local_web::BrowserTls;
use harmion_net_native::{SharedStore, SystemHost, app, oauth_browser, tls::TlsIdentity};
use harmion_node::{application, oauth, storage::Identity};
use harmion_protocol::connect::*;
use harmion_protocol::*;
use harmion_store_sqlite::SqliteStore;
use http_body_util::{BodyExt, Full};
use hyper::{Request, StatusCode, header};
use hyper_util::rt::TokioIo;
use rustls::pki_types::ServerName;
use serde_json::json;
use std::sync::Arc;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::watch,
};
use tokio_rustls::TlsConnector;

const VERIFIER: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._~";

struct Web {
    host: String,
    port: u16,
    root: Vec<u8>,
}
impl Web {
    async fn request(
        &self,
        method: &str,
        path: &str,
        origin: Option<&str>,
        body: &[u8],
    ) -> (StatusCode, hyper::HeaderMap, Bytes) {
        self.request_inner(method, path, origin, None, body).await
    }
    async fn request_auth(
        &self,
        method: &str,
        path: &str,
        origin: Option<&str>,
        authorization: &str,
        body: &[u8],
    ) -> (StatusCode, hyper::HeaderMap, Bytes) {
        self.request_inner(method, path, origin, Some(authorization), body)
            .await
    }
    async fn request_inner(
        &self,
        method: &str,
        path: &str,
        origin: Option<&str>,
        authorization: Option<&str>,
        body: &[u8],
    ) -> (StatusCode, hyper::HeaderMap, Bytes) {
        let tcp = TcpStream::connect((self.host.as_str(), self.port))
            .await
            .unwrap();
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(rustls::pki_types::CertificateDer::from(self.root.clone()))
            .unwrap();
        let config = rustls::ClientConfig::builder_with_provider(
            rustls::crypto::ring::default_provider().into(),
        )
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
        let tls = TlsConnector::from(Arc::new(config))
            .connect(ServerName::try_from(self.host.clone()).unwrap(), tcp)
            .await
            .unwrap();
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(tls))
            .await
            .unwrap();
        let task = tokio::spawn(async move {
            let _ = connection.await;
        });
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, format!("{}:{}", self.host, self.port));
        if let Some(origin) = origin {
            builder = builder.header(header::ORIGIN, origin);
        }
        if let Some(authorization) = authorization {
            builder = builder.header(header::AUTHORIZATION, authorization);
        }
        if !body.is_empty() {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
        }
        if method == "OPTIONS" {
            builder = builder.header("access-control-request-method", "POST");
        }
        let response = sender
            .send_request(
                builder
                    .body(Full::new(Bytes::copy_from_slice(body)))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        task.abort();
        (status, headers, bytes)
    }
}

#[tokio::test]
async fn web_connection_start_status_complete_uses_cors_and_helper() {
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
    let (oauth_stop, oauth_receiver) = watch::channel(false);
    let browser = BrowserTls::generate("Admin").unwrap();
    let web = Web {
        host: "127.0.0.1".into(),
        port: 0,
        root: browser.root_der.clone(),
    };
    let oauth_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let oauth_port = oauth_listener.local_addr().unwrap().port();
    let web = Web {
        port: oauth_port,
        ..web
    };
    oauth_browser::configure(&store, oauth_listener.local_addr().unwrap()).unwrap();
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
    let origin = "http://127.0.0.1:43123".to_string();
    let offer = ConnectionOffer {
        version: 1,
        name: "Web app".into(),
        redirect_uri: "http://127.0.0.1:43123/harmion-callback".into(),
        web_origin: Some(origin.clone()),
        client_id: None,
        challenge: oauth::pkce_challenge(VERIFIER).unwrap(),
        state: "state".into(),
        grants: GrantSpec {
            read: Some(FilterSpec::All),
            write: None,
            call: true,
            provide: true,
            provide_names: vec!["ledger/balance".into()],
            manage_apps: false,
            manage_nodes: false,
        },
        provider: Some(harmion_protocol::interpreter::ProviderRegistration {
            name: "ledger/balance".into(),
            contract_version: harmion_protocol::interpreter::CONTRACT_VERSION,
            input_schema: json!({"type": "object"}),
            output_schema: json!({"type": "object"}),
            default_query: Some(harmion_protocol::interpreter::DefaultQuery {
                kinds: vec!["ledger".into()],
            }),
        }),
        provider_replace: false,
    };
    let body = serde_json::to_vec(&offer).unwrap();
    let (status, headers, bytes) = web
        .request("POST", "/connect/start", Some(&origin), &body)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .unwrap()
            .to_str()
            .unwrap(),
        origin
    );
    let started: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let connection_id = started["connection_id"].as_str().unwrap().to_owned();
    let secret = started["request_secret"].as_str().unwrap().to_owned();
    assert!(
        started["verification_url"]
            .as_str()
            .unwrap()
            .starts_with(&format!("https://127.0.0.1:{oauth_port}/connect#"))
    );

    let (status, _, bytes) = web
        .request(
            "OPTIONS",
            "/connect/start",
            Some(&origin),
            br#"{"connection_id":"","request_secret":""}"#,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let _ = bytes;
    let (status, _, _) = web
        .request(
            "POST",
            "/connect/start",
            Some("http://example.com"),
            br#"{}"#,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _, page) = web.request("GET", "/connect", None, &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&page).contains("Harmion connection"));

    admin_client
        .connect_approve(&ConnectionApproval {
            connection_id: connection_id.clone(),
            allow: true,
            replace: false,
        })
        .await
        .unwrap();
    let status_body = serde_json::to_vec(&json!({
        "connection_id": connection_id,
        "request_secret": secret,
    }))
    .unwrap();
    let (status, _, bytes) = web
        .request("POST", "/connect/status", Some(&origin), &status_body)
        .await;
    assert_eq!(status, StatusCode::OK);
    let view: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(view["status"], "approved");
    assert_eq!(view["web_origin"], origin);

    let complete_body = serde_json::to_vec(&json!({
        "connection_id": connection_id,
        "request_secret": secret,
        "code_verifier": VERIFIER,
    }))
    .unwrap();
    let (status, _, bytes) = web
        .request("POST", "/connect/complete", Some(&origin), &complete_body)
        .await;
    assert_eq!(status, StatusCode::OK);
    let completion: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(completion["token_type"], "Bearer");
    assert!(completion["access_token"].is_string());
    assert_eq!(completion["provider"]["name"], "ledger/balance");
    assert!(completion["provider"]["ticket"].is_string());

    let ticket_body = serde_json::to_vec(&json!({
        "provider_id": completion["provider"]["provider_id"],
    }))
    .unwrap();
    let authorization = format!("Bearer {}", completion["access_token"].as_str().unwrap());
    let (status, headers, _) = web
        .request_auth(
            "OPTIONS",
            "/connect/ticket",
            Some(&origin),
            &authorization,
            &ticket_body,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(
        headers
            .get(header::ACCESS_CONTROL_ALLOW_HEADERS)
            .unwrap()
            .to_str()
            .unwrap()
            .contains("authorization")
    );
    let (status, _, bytes) = web
        .request_auth(
            "POST",
            "/connect/ticket",
            Some(&origin),
            &authorization,
            &ticket_body,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let ticket: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(ticket["ticket"].is_string());
    assert!(ticket["websocket_url"].is_string());

    stop.send(true).unwrap();
    oauth_stop.send(true).unwrap();
    server.await.unwrap().unwrap();
    oauth_server.await.unwrap().unwrap();
}
