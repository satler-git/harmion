//! Local-only E2E: forward TLS to a workerd DO and run the native sync loop.
//!
//! This is a probe utility, not a deployable proxy. It terminates TLS with a
//! throwaway CA for `localhost`, forwards HTTP/1.1 to a local workerd instance,
//! provisions the DO, and runs `sync::sync_once` with the real NativeTransport.
use harmion_domain::{ClusterId, NodeId};
use harmion_net_native::{NativeTransport, SharedStore, sync, tls::TlsIdentity};
use harmion_node::{
    membership_state::MembershipSigner,
    storage::{Identity, Store, accept_records},
};
use harmion_protocol::{
    LogEpoch, PeerCommand, PeerReply, Record,
    membership::{Action, Binding, Certificate, Member, Statement},
};
use harmion_store_sqlite::SqliteStore;
use http_body_util::{BodyExt, Full};
use hyper::{
    Request, Response, body::Bytes, header::HOST, server::conn::http1, service::service_fn,
};
use hyper_util::rt::TokioIo;
use std::{convert::Infallible, sync::Arc, time::Duration};
use tokio::{net::TcpListener, time::timeout};
use tokio_rustls::TlsAcceptor;

const CLUSTER: ClusterId = ClusterId([1; 32]);
const BOOTSTRAP: &str = "local-test-bootstrap-token";

fn signer(n: u8) -> MembershipSigner {
    MembershipSigner::from_bytes(&[n; 32])
}
fn node(n: u8) -> NodeId {
    NodeId([n; 32])
}
fn member(n: u8, endpoint: String) -> Member {
    Member {
        node: node(n),
        signing_key: signer(n).public_key(),
        binding: Binding::SignedHttps(signer(n).public_key()),
        endpoint,
    }
}
fn admit(issuer: u8, target: Member) -> Certificate {
    signer(issuer)
        .sign(Statement {
            cluster: CLUSTER,
            issuer: node(issuer),
            signing_key: signer(issuer).public_key(),
            nonce: [target.node.0[0]; 32],
            action: Action::Admit(target),
        })
        .unwrap()
}
fn member_json(member: &Member) -> String {
    let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    format!(
        r#"{{"node":"{}","signing_key":"{}","binding":{{"transport":"signed_https","key":"{}"}},"endpoint":"{}"}}"#,
        hex(&member.node.0),
        hex(&member.signing_key),
        hex(&member.signing_key),
        member.endpoint
    )
}
fn fixture() -> Record {
    let hex = include_str!("../../../contracts/fixtures/create-v1.hex").trim();
    let bytes: Vec<u8> = hex
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect();
    Record::from_bytes(&bytes).unwrap()
}
fn provision(
    store: &mut SqliteStore,
    node_id: u8,
    own: Member,
    root: &Certificate,
    evidence: &[Certificate],
) {
    harmion_node::provisioning::provision(
        store,
        &signer(node_id),
        &harmion_node::provisioning::Manifest {
            own,
            root: root.clone(),
            evidence,
            contacts: &[],
        },
    )
    .unwrap();
}

async fn forward(
    request: Request<hyper::body::Incoming>,
    client: hyper_util::client::legacy::Client<
        hyper_util::client::legacy::connect::HttpConnector,
        Full<Bytes>,
    >,
    base: String,
    authority: String,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let (parts, body) = request.into_parts();
    let bytes = match body.collect().await {
        Ok(value) => value.to_bytes(),
        Err(_) => return Ok(error_response(hyper::StatusCode::BAD_REQUEST)),
    };
    let path = parts
        .uri
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or("/");
    let mut builder = Request::builder()
        .method(parts.method)
        .uri(format!("{base}{path}"));
    for (name, value) in parts.headers.iter() {
        if name != HOST {
            builder = builder.header(name, value);
        }
    }
    builder = builder.header(HOST, &authority);
    let request = match builder.body(Full::new(bytes)) {
        Ok(value) => value,
        Err(_) => return Ok(error_response(hyper::StatusCode::BAD_REQUEST)),
    };
    match client.request(request).await {
        Ok(response) => {
            let (parts, body) = response.into_parts();
            let bytes = match body.collect().await {
                Ok(value) => value.to_bytes(),
                Err(_) => return Ok(error_response(hyper::StatusCode::BAD_GATEWAY)),
            };
            Ok(Response::from_parts(parts, Full::new(bytes)))
        }
        Err(_) => Ok(error_response(hyper::StatusCode::BAD_GATEWAY)),
    }
}
fn error_response(status: hyper::StatusCode) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .body(Full::new(Bytes::new()))
        .unwrap()
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let workerd = args
        .get(1)
        .cloned()
        .expect("usage: tls_forward_sync <workerd-url> <proxy-port>");
    let proxy_port: u16 = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| "0".into())
        .parse()
        .unwrap();
    let origin = format!("https://localhost:{proxy_port}");
    let authority = format!("localhost:{proxy_port}");

    // Throwaway CA and localhost server certificate.
    let ca_key = rcgen::KeyPair::generate().unwrap();
    let mut ca_params = rcgen::CertificateParams::new(Vec::new()).unwrap();
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca = rcgen::CertifiedIssuer::self_signed(ca_params, ca_key).unwrap();
    let leaf_key = rcgen::KeyPair::generate().unwrap();
    let mut leaf_params = rcgen::CertificateParams::new(vec!["localhost".to_owned()]).unwrap();
    leaf_params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];
    let leaf = leaf_params.signed_by(&leaf_key, &ca).unwrap();
    let server_config = rustls::ServerConfig::builder_with_provider(
        rustls::crypto::ring::default_provider().into(),
    )
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![leaf.der().to_vec().into()],
        rustls::pki_types::PrivateKeyDer::Pkcs8(leaf_key.serialize_der().into()),
    )
    .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(server_config));
    let listener = TcpListener::bind(("127.0.0.1", proxy_port)).await.unwrap();
    let client: hyper_util::client::legacy::Client<_, Full<Bytes>> =
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build_http();
    let base = workerd.trim_end_matches('/').to_owned();
    let forwarding_client = client.clone();
    let forwarding_base = base.clone();
    let forwarding_authority = authority.clone();
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else {
                break;
            };
            let acceptor = acceptor.clone();
            let client = forwarding_client.clone();
            let base = forwarding_base.clone();
            let authority = forwarding_authority.clone();
            tokio::spawn(async move {
                let Ok(tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let service = service_fn(move |request| {
                    forward(request, client.clone(), base.clone(), authority.clone())
                });
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(tls), service)
                    .await;
            });
        }
    });

    // Provision the DO as node 3 with the owner root (node 2).
    let root = admit(2, member(2, "https://node2.example".to_owned()));
    let evidence = [admit(2, member(3, origin.clone()))];
    let manifest = format!(
        r#"{{"own":{},"root":"{}","evidence":["{}"],"contacts":[]}}"#,
        member_json(&member(3, origin.clone())),
        root.bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
        evidence[0]
            .bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    let response = client
        .request(
            Request::builder()
                .method("POST")
                .uri(format!("{base}/provision"))
                .header("x-harmion-bootstrap", BOOTSTRAP)
                .body(Full::new(Bytes::from(manifest)))
                .unwrap(),
        )
        .await
        .expect("provision request");
    assert_eq!(response.status(), 200, "provision failed");

    // Client node 2 (the root) with its own store and one durable record.
    let mut client_db = SqliteStore::open(
        ":memory:",
        Identity {
            cluster: CLUSTER,
            node: node(2),
            epoch: LogEpoch([2; 32]),
        },
    )
    .unwrap();
    provision(
        &mut client_db,
        2,
        member(2, "https://node2.example".to_owned()),
        &root,
        &evidence,
    );
    client_db
        .transaction(|tx| accept_records(tx, &[fixture()]))
        .unwrap();
    let store = SharedStore::new(client_db);
    let transport = NativeTransport {
        membership_signer: Some(Arc::new(signer(2))),
        tls: Arc::new(TlsIdentity::generate().unwrap()),
        iroh: None,
        signed_https_roots: vec![ca.der().to_vec()],
    };
    let report = timeout(
        Duration::from_secs(30),
        sync::sync_once(&store, &transport, node(3)),
    )
    .await
    .expect("sync timeout")
    .expect("sync");
    assert_eq!(report.uploaded, 1, "uploaded: {report:?}");
    println!("PASS: native sync pushed a record to the workerd DO through TLS");

    let peer = harmion_node::membership::Peer {
        node: node(3),
        key: harmion_node::membership::PeerKey::SignedHttps(signer(3).public_key()),
        endpoint: origin.clone(),
        retired: false,
    };
    use harmion_net_native::sync::PeerTransport;
    let PeerReply::Status { high, .. } = transport
        .exchange(
            Identity {
                cluster: CLUSTER,
                node: node(2),
                epoch: LogEpoch([2; 32]),
            },
            &peer,
            PeerCommand::Status,
        )
        .await
        .expect("status")
    else {
        panic!("status")
    };
    assert_eq!(high, 1);
    println!("PASS: workerd DO high=1 after the native push");

    let report = timeout(
        Duration::from_secs(30),
        sync::sync_once(&store, &transport, node(3)),
    )
    .await
    .expect("sync timeout")
    .expect("sync");
    assert_eq!(report.downloaded, 1);
    println!("PASS: second native round pulled the durable record back");
}
