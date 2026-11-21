//! Local-only TLS terminator for Workerd probes.
//!
//! This is a test utility, not a deployable proxy. It serves a throwaway CA and
//! `localhost` certificate, forwards HTTP/1.1 to a local Workerd socket, and
//! writes the CA DER file for the native client's `--signed-https-root`.
use http_body_util::{BodyExt, Full};
use hyper::{
    Request, Response, body::Bytes, header::HOST, server::conn::http1, service::service_fn,
};
use hyper_util::rt::TokioIo;
use std::{convert::Infallible, sync::Arc};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

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
        .expect("usage: tls_forward <workerd-url> <listen-port> <ca-out-path>");
    let port: u16 = args
        .get(2)
        .expect("listen port")
        .parse()
        .expect("numeric port");
    let ca_out = args.get(3).cloned().expect("CA output path");

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
    let listener = TcpListener::bind(("127.0.0.1", port)).await.unwrap();
    let actual = listener.local_addr().unwrap().port();
    std::fs::write(&ca_out, ca.der()).unwrap();
    println!(
        "{}",
        serde_json::json!({
            "origin": format!("https://localhost:{actual}"),
            "port": actual,
            "ca": ca_out,
        })
    );
    use std::io::Write as _;
    std::io::stdout().flush().unwrap();

    let client: hyper_util::client::legacy::Client<_, Full<Bytes>> =
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            // Workerd closes idle connections; a pooled one would make the first
            // request after a daemon restart fail with 502.
            .pool_max_idle_per_host(0)
            .build_http();
    let base = workerd.trim_end_matches('/').to_owned();
    let authority = format!("localhost:{actual}");
    loop {
        let Ok((tcp, _)) = listener.accept().await else {
            break;
        };
        let acceptor = acceptor.clone();
        let client = client.clone();
        let base = base.clone();
        let authority = authority.clone();
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
}
