//! Local probe client for the workerd DO bridge. Native HTTP/1.1 only; it signs
//! and verifies real Peer v2 frames with the same Rust code as production.
use harmion_domain::{ClusterId, NodeId};
use harmion_node::{https_peer::verify_frame, membership_state::MembershipSigner};
use harmion_protocol::{
    LogEntry, LogEpoch, Page, PeerCommand, PeerReply, PeerRequest, Record, encode_peer_request,
    https_peer::{
        Body, CHALLENGE_PATH, Commitment, Common, EXCHANGE_PATH, ExchangeContext, Frame,
        PayloadClass, ResponseResult,
    },
    membership::{Action, Binding, Certificate, Member, Statement},
};
use std::{
    io::{Read, Write},
    net::TcpStream,
};

const CLUSTER: ClusterId = ClusterId([1; 32]);
const HUB_ORIGIN: &str = "https://server.example";
const LEAF_ORIGIN: &str = "https://leaf.internal";
const HOST: &str = "server.example";

fn hub() -> u8 {
    3
}
fn leaf() -> u8 {
    4
}
fn signer(n: u8) -> MembershipSigner {
    MembershipSigner::from_bytes(&[n; 32])
}
fn node(n: u8) -> NodeId {
    NodeId([n; 32])
}
fn member(n: u8) -> Member {
    Member {
        node: node(n),
        signing_key: signer(n).public_key(),
        binding: Binding::SignedHttps(signer(n).public_key()),
        endpoint: if n == leaf() {
            LEAF_ORIGIN.to_owned()
        } else {
            format!("https://node{n}.example")
        },
    }
}
fn hub_identity() -> harmion_node::storage::Identity {
    harmion_node::storage::Identity {
        cluster: CLUSTER,
        node: node(hub()),
        epoch: LogEpoch([hub(); 32]),
    }
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
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
    format!(
        r#"{{"node":"{}","signing_key":"{}","binding":{{"transport":"signed_https","key":"{}"}},"endpoint":"{}"}}"#,
        hex(&member.node.0),
        hex(&member.signing_key),
        hex(&member.signing_key),
        member.endpoint
    )
}
fn manifest(own: &Member, evidence: &[Certificate], contacts: &[Member]) -> String {
    let evidence: Vec<String> = evidence
        .iter()
        .map(|certificate| format!("\"{}\"", hex(certificate.bytes())))
        .collect();
    let contacts: Vec<String> = contacts.iter().map(member_json).collect();
    format!(
        r#"{{"own":{},"root":"{}","evidence":[{}],"contacts":[{}]}}"#,
        member_json(own),
        hex(admit(2, member(2)).bytes()),
        evidence.join(","),
        contacts.join(",")
    )
}

struct Http {
    base: String,
}
impl Http {
    fn post(&self, path: &str, headers: &[(&str, &str)], body: &[u8]) -> (u16, Vec<u8>) {
        let address = self
            .base
            .trim_start_matches("http://")
            .trim_end_matches('/');
        let mut stream = TcpStream::connect(address).expect("connect workerd");
        let mut request = format!(
            "POST {path} HTTP/1.1\r\nHost: {HOST}\r\nConnection: close\r\nContent-Length: {}\r\n",
            body.len()
        );
        for (name, value) in headers {
            request.push_str(&format!("{name}: {value}\r\n"));
        }
        request.push_str("\r\n");
        stream.write_all(request.as_bytes()).unwrap();
        stream.write_all(body).unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        let split = response
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .expect("response headers");
        let head = String::from_utf8_lossy(&response[..split]).into_owned();
        let body = &response[split + 4..];
        let status = head
            .split_whitespace()
            .nth(1)
            .and_then(|value| value.parse().ok())
            .expect("status");
        let chunked = head
            .to_ascii_lowercase()
            .contains("transfer-encoding: chunked");
        (
            status,
            if chunked {
                dechunk(body)
            } else {
                body.to_vec()
            },
        )
    }
}
fn dechunk(mut body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(end) = body.windows(2).position(|window| window == b"\r\n") {
        let size =
            usize::from_str_radix(std::str::from_utf8(&body[..end]).unwrap_or("0").trim(), 16)
                .unwrap_or(0);
        body = &body[end + 2..];
        if size == 0 {
            break;
        }
        out.extend_from_slice(&body[..size.min(body.len())]);
        body = &body[(size + 2).min(body.len())..];
    }
    out
}
fn decode_peer_code(code: &str) -> serde_json::Value {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use std::io::Read;
    let encoded = code.strip_prefix("harmion-peer:v1:z:").expect("prefix");
    let compressed = URL_SAFE_NO_PAD.decode(encoded).expect("base64");
    let mut json = String::new();
    flate2::read::GzDecoder::new(compressed.as_slice())
        .read_to_string(&mut json)
        .expect("gzip");
    serde_json::from_str(&json).expect("json")
}

#[allow(clippy::too_many_arguments)]
fn exchange(
    http: &Http,
    sender: u8,
    audience: u8,
    origin: &str,
    prefix: &str,
    command: PeerCommand,
    class: PayloadClass,
) -> PeerReply {
    let command_debug = format!("{command:?}");
    let label = format!("{prefix} {command:?}");
    let inner = encode_peer_request(&PeerRequest {
        cluster: CLUSTER,
        node: node(sender),
        command,
    })
    .unwrap();
    let client = signer(sender);
    let common = Common {
        cluster: CLUSTER,
        sender: node(sender),
        audience: node(audience),
        origin: origin.to_owned(),
    };
    let commitment = Commitment::from_request(&inner).unwrap();
    let request = client
        .sign_challenge_request(common.clone(), [4; 32], commitment.clone())
        .unwrap_or_else(|error| {
            panic!(
                "sign challenge failed: inner_len={} class={:?} error={error:?} command={command_debug}",
                inner.len(),
                commitment.class
            )
        });
    let (status, body) = http.post(
        &format!("{prefix}{CHALLENGE_PATH}"),
        &[],
        &request.encode().unwrap(),
    );
    assert_eq!(status, 200, "challenge status");
    let challenge = Frame::decode(&body).unwrap();
    verify_frame(&challenge, &signer(audience).public_key()).unwrap();
    let Body::Challenge {
        challenge_request_hash,
        client_nonce,
        generation,
        challenge_nonce,
        ..
    } = &challenge.body
    else {
        panic!("challenge frame")
    };
    let exchange = client
        .sign_exchange(
            common,
            ExchangeContext {
                challenge_request_hash: *challenge_request_hash,
                challenge_hash: challenge.hash().unwrap(),
                client_nonce: *client_nonce,
                generation: *generation,
                challenge_nonce: *challenge_nonce,
            },
            class,
            inner,
        )
        .unwrap();
    let (status, body) = http.post(
        &format!("{prefix}{EXCHANGE_PATH}"),
        &[],
        &exchange.encode().unwrap(),
    );
    let response = Frame::decode(&body).unwrap();
    verify_frame(&response, &signer(audience).public_key()).unwrap();
    let Body::Response { result, .. } = &response.body else {
        panic!("response frame")
    };
    match result {
        ResponseResult::Peer(bytes) => {
            assert_eq!(status, 200, "exchange status");
            harmion_protocol::decode_peer_response(bytes).unwrap().reply
        }
        ResponseResult::Error(error) => {
            panic!("signed error {:?} for {label}", error.code)
        }
    }
}
fn exchange_hub(http: &Http, sender: u8, command: PeerCommand, class: PayloadClass) -> PeerReply {
    exchange(http, sender, hub(), HUB_ORIGIN, "", command, class)
}
fn exchange_leaf(http: &Http, sender: u8, command: PeerCommand, class: PayloadClass) -> PeerReply {
    exchange(http, sender, leaf(), LEAF_ORIGIN, "/leaf", command, class)
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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let base = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "http://127.0.0.1:8080".to_owned());
    let verify_only = args.iter().any(|arg| arg == "--verify");
    let http = Http { base };
    let admin = [("authorization", "Bearer local-test-admin-token")];
    let bootstrap = [("x-harmion-bootstrap", "local-test-bootstrap-token")];

    if !verify_only {
        let hub_manifest = manifest(
            &member(hub()),
            &[admit(2, member(hub())), admit(2, member(leaf()))],
            &[member(2)],
        );
        let (status, body) = http.post("/provision", &bootstrap, hub_manifest.as_bytes());
        assert_eq!(
            status,
            200,
            "provision hub: {}",
            String::from_utf8_lossy(&body)
        );
        println!("PASS: hub provisioning accepted on the DO SQL host");
        let (status, _) = http.post("/provision", &bootstrap, hub_manifest.as_bytes());
        assert_eq!(status, 409, "second provision must fail closed");
        println!("PASS: bootstrap route is disabled after provisioning");

        let leaf_manifest = manifest(
            &member(leaf()),
            &[admit(2, member(leaf())), admit(2, member(hub()))],
            &[member(2)],
        );
        let (status, body) = http.post("/leaf/provision", &bootstrap, leaf_manifest.as_bytes());
        assert_eq!(
            status,
            200,
            "provision leaf: {}",
            String::from_utf8_lossy(&body)
        );
        println!("PASS: leaf provisioning accepted through the hub proxy");

        let (status, body) = http.post("/admin/registration-code", &admin, b"{}");
        assert_eq!(status, 200);
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let code = value["code"].as_str().expect("registration code");
        assert!(code.starts_with("harmion-peer:v1:z:"), "{code}");
        assert_eq!(
            value["registration"]["signing_key"].as_str().unwrap(),
            hex(&signer(hub()).public_key())
        );
        assert_eq!(value["registration"]["binding"]["origin"], HUB_ORIGIN);
        let decoded = decode_peer_code(code);
        assert_eq!(decoded["node_id"].as_str().unwrap(), hex(&node(hub()).0));
        println!("PASS: registration code is native-admin compatible");

        let reply = exchange_hub(
            &http,
            2,
            PeerCommand::MembershipStatus,
            PayloadClass::Evidence,
        );
        assert!(matches!(reply, PeerReply::Status { .. }));
        println!("PASS: hub membership evidence exchange");

        // Membership relay: the root admits node 5 into the hub, and the hub
        // appends the certificate to its own log so another leaf can pull it.
        let admitted = admit(2, member(5));
        let page = Page {
            epoch: LogEpoch([2; 32]),
            after: 0,
            through: 1,
            high: 1,
            entries: vec![LogEntry {
                seq: 1,
                record: admitted.clone(),
            }],
        };
        let reply = exchange_hub(
            &http,
            2,
            PeerCommand::MembershipAccept(page),
            PayloadClass::Evidence,
        );
        assert!(
            matches!(reply, PeerReply::Ack { through: 1, .. }),
            "membership relay: {reply:?}"
        );
        let reply = exchange_hub(
            &http,
            2,
            PeerCommand::MembershipStatus,
            PayloadClass::Evidence,
        );
        let PeerReply::Status { epoch, high } = reply else {
            panic!("hub membership status")
        };
        assert!(high >= 3, "hub membership high: {high}");
        let reply = exchange_hub(
            &http,
            2,
            PeerCommand::MembershipPage {
                epoch,
                after: 0,
                high,
            },
            PayloadClass::Evidence,
        );
        let PeerReply::MembershipPage(page) = reply else {
            panic!("membership page")
        };
        assert!(
            page.entries
                .iter()
                .any(|entry| entry.record.id() == admitted.id()),
            "hub did not relay the new admission"
        );
        println!("PASS: hub relays membership certificates to another leaf");

        // Seed a record on the leaf through the hub proxy, then let the hub pull
        // it with its own outbound sync (leaf is not directly reachable).
        let record = fixture();
        let page = Page {
            epoch: LogEpoch([leaf(); 32]),
            after: 0,
            through: 1,
            high: 1,
            entries: vec![LogEntry { seq: 1, record }],
        };
        let reply = exchange_leaf(&http, 2, PeerCommand::Accept(page), PayloadClass::Data);
        assert!(
            matches!(reply, PeerReply::Ack { through: 1, .. }),
            "leaf accept"
        );
        println!("PASS: record seeded on the outbound-only leaf");

        let (status, body) = http.post("/internal/alarm", &admin, b"{}");
        assert_eq!(status, 200, "alarm: {}", String::from_utf8_lossy(&body));
        let report: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let results = report["results"].as_array().expect("results");
        assert!(
            results.iter().any(|result| {
                result["ok"] == true && result["downloaded"].as_u64().unwrap_or(0) >= 1
            }),
            "alarm results: {report}"
        );
        println!("PASS: scheduled alarm pulled the leaf record with outbound sync");
        let (status, body) = http.post("/internal/alarm", &admin, b"{}");
        assert_eq!(status, 200);
        let report: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(report["results"].as_array().unwrap().len(), 0, "{report}");
        println!("PASS: backoff skips peers until their next attempt");

        let reply = exchange_hub(
            &http,
            2,
            PeerCommand::Page {
                epoch: LogEpoch([hub(); 32]),
                after: 0,
                high: 1,
            },
            PayloadClass::Data,
        );
        let PeerReply::Page(page) = reply else {
            panic!("page")
        };
        assert_eq!(page.entries.len(), 1);
        println!("PASS: hub serves the pulled record to another peer");
    }

    let reply = exchange_hub(&http, 2, PeerCommand::Status, PayloadClass::Data);
    let PeerReply::Status { high, .. } = reply else {
        panic!("status")
    };
    assert_eq!(high, 1, "durable high");
    println!("PASS: hub durable log high={high}");
    let reply = exchange_hub(
        &http,
        2,
        PeerCommand::Page {
            epoch: LogEpoch([hub(); 32]),
            after: 0,
            high: 1,
        },
        PayloadClass::Data,
    );
    let PeerReply::Page(page) = reply else {
        panic!("page")
    };
    assert_eq!(page.entries.len(), 1);
    println!("PASS: pulled record survived restart");
    let (status, body) = http.post("/admin/overview", &admin, b"{}");
    assert_eq!(status, 200);
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains(r#""read_only":true"#), "overview: {text}");
    println!("PASS: admin overview is read-only");
    let _ = hub_identity();
}
