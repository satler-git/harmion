use harmion_domain::{ClusterId, NodeId};
use harmion_protocol::{
    CodecError, LogEntry, LogEpoch, MAX_PAGE_ENTRIES, MAX_PEER_BYTES, Page, PeerCommand, PeerReply,
    PeerRequest, PeerResponse, Record, encode_peer_request, encode_peer_response, https_peer::*,
    membership::Certificate,
};

fn hex(text: &str) -> Vec<u8> {
    let text = text.trim();
    assert_eq!(text.len() % 2, 0);
    text.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}
fn common(server: bool) -> Common {
    Common {
        cluster: ClusterId([1; 32]),
        sender: NodeId([if server { 3 } else { 2 }; 32]),
        audience: NodeId([if server { 2 } else { 3 }; 32]),
        origin: "https://replica.example".into(),
    }
}
fn request(command: PeerCommand) -> Vec<u8> {
    encode_peer_request(&PeerRequest {
        cluster: ClusterId([1; 32]),
        node: NodeId([2; 32]),
        command,
    })
    .unwrap()
}
fn frame(server: bool, body: Body) -> Frame {
    Frame::new(common(server), body, [11; 64]).unwrap()
}
fn exchange_context() -> ExchangeContext {
    let frames = fixtures();
    match &frames[2].body {
        Body::Exchange { context, .. } => context.clone(),
        _ => unreachable!(),
    }
}
fn fixtures() -> Vec<Frame> {
    let request = request(PeerCommand::Status);
    let commitment = Commitment::from_request(&request).unwrap();
    let q = frame(
        false,
        Body::ChallengeRequest {
            client_nonce: [4; 32],
            commitment: commitment.clone(),
        },
    );
    let c = frame(
        true,
        Body::Challenge {
            challenge_request_hash: q.hash().unwrap(),
            client_nonce: [4; 32],
            generation: [5; 32],
            challenge_nonce: [6; 32],
            ttl_ms: 60000,
            commitment,
        },
    );
    let x = frame(
        false,
        Body::Exchange {
            context: ExchangeContext {
                challenge_request_hash: q.hash().unwrap(),
                challenge_hash: c.hash().unwrap(),
                client_nonce: [4; 32],
                generation: [5; 32],
                challenge_nonce: [6; 32],
            },
            class: PayloadClass::Data,
            request,
        },
    );
    let context = ResponseContext {
        exchange_hash: x.hash().unwrap(),
        challenge_hash: c.hash().unwrap(),
        client_nonce: [4; 32],
        generation: [5; 32],
        challenge_nonce: [6; 32],
    };
    let r = frame(
        true,
        Body::Response {
            context: context.clone(),
            result: ResponseResult::Peer(
                encode_peer_response(&PeerResponse {
                    cluster: ClusterId([1; 32]),
                    node: NodeId([3; 32]),
                    reply: PeerReply::Ack {
                        epoch: LogEpoch([9; 32]),
                        through: 4,
                    },
                })
                .unwrap(),
            ),
        },
    );
    let error = frame(
        true,
        Body::ChallengeError {
            challenge_request_hash: q.hash().unwrap(),
            client_nonce: [4; 32],
            error: TransportError {
                code: ErrorCode::Capacity,
                retry_after_ms: 250,
            },
        },
    );
    let response_error = frame(
        true,
        Body::Response {
            context,
            result: ResponseResult::Error(TransportError {
                code: ErrorCode::StoreUnavailable,
                retry_after_ms: 500,
            }),
        },
    );
    vec![q, c, x, r, error, response_error]
}
macro_rules! fixture {
    ($name:literal) => {
        (
            include_str!(concat!(
                "../../../contracts/fixtures/https-peer-v2-",
                $name,
                ".hex"
            )),
            include_str!(concat!(
                "../../../contracts/fixtures/https-peer-v2-",
                $name,
                "-signing.hex"
            )),
            include_str!(concat!(
                "../../../contracts/fixtures/https-peer-v2-",
                $name,
                ".sha256"
            )),
        )
    };
}
#[test]
fn all_kinds_match_independent_python_bytes_signing_inputs_and_hashes() {
    let expected = [
        fixture!("challenge-request"),
        fixture!("challenge"),
        fixture!("exchange"),
        fixture!("response"),
        fixture!("challenge-error"),
        fixture!("response-error"),
    ];
    for (frame, (encoded, signing, hash)) in fixtures().iter().zip(expected) {
        let bytes = hex(encoded);
        assert_eq!(frame.encode().unwrap(), bytes);
        assert_eq!(frame.signing_bytes().unwrap(), hex(signing));
        assert_eq!(frame.hash().unwrap().as_slice(), hex(hash));
        assert_eq!(Frame::decode(&bytes).unwrap(), *frame);
        for length in 0..bytes.len() {
            assert!(
                Frame::decode(&bytes[..length]).is_err(),
                "accepted truncation {length}"
            );
        }
        let mut trailing = bytes;
        trailing.push(0);
        assert_eq!(Frame::decode(&trailing), Err(CodecError::TrailingBytes));
    }
}
#[test]
fn origin_is_exact_standard_url_canonical_dns_origin_not_webpki_or_egress_approval() {
    for valid in [
        "https://replica.example",
        "https://xn--bcher-kva.example:8443",
        "https://localhost",
    ] {
        validate_origin(valid).unwrap();
    }
    for invalid in [
        "http://replica.example",
        "HTTPS://replica.example",
        "https://Replica.example",
        "https://replica.example/",
        "https://replica.example/path",
        "https://replica.example?x",
        "https://replica.example#x",
        "https://replica.example:443",
        "https://replica.example:0444",
        "https://replica.example:0",
        "https://replica.example:65536",
        "https://user@replica.example",
        "https://user:secret@replica.example",
        "https://@replica.example",
        "https://replica.example.",
        "https://replica..example",
        "https://-replica.example",
        "https://replica-.example",
        "https://replica_example",
        "https://bücher.example",
        "https://replica.example/%2e",
        "https://127.0.0.1",
        "https://127.1",
        "https://0x7f000001",
        "https://2130706433",
        "https://0177.0.0.1",
        "https://[::1]",
        "https://replica.example\\evil",
        "https://replica.example\n",
        "https://%72eplica.example",
        "https://",
        "",
    ] {
        assert!(validate_origin(invalid).is_err(), "accepted {invalid:?}");
    }
    assert!(validate_origin(&format!("https://{}.example", "a".repeat(64))).is_err());
    assert!(validate_origin(&"x".repeat(MAX_ORIGIN_BYTES + 1)).is_err());
}
#[test]
fn methods_paths_versions_tags_lengths_and_self_audience_fail_closed() {
    let q = fixtures().remove(0);
    let bytes = q.encode().unwrap();
    let route_start = 9 + 96 + 4 + q.common.origin.len();
    for (index, value) in [
        (0, b'X'),
        (7, 3),
        (8, 255),
        (route_start, 1),
        (route_start + 5, b'X'),
    ] {
        let mut changed = bytes.clone();
        changed[index] = value;
        assert!(Frame::decode(&changed).is_err(), "mutation at {index}");
    }
    let mut huge_origin = bytes.clone();
    huge_origin[105..109].copy_from_slice(&u32::MAX.to_be_bytes());
    assert_eq!(Frame::decode(&huge_origin), Err(CodecError::TooLarge));
    let mut self_id = q.clone();
    self_id.common.audience = self_id.common.sender;
    assert!(self_id.encode().is_err());
    let mut invalid_origin = q;
    invalid_origin.common.origin.push('/');
    assert!(invalid_origin.signing_bytes().is_err());
    assert_eq!(
        Frame::decode(&vec![0; MAX_FRAME_BYTES + 1]),
        Err(CodecError::TooLarge)
    );
    let mut oversized_metadata = vec![0; MAX_METADATA_BYTES + 1];
    oversized_metadata[..9].copy_from_slice(b"HHT\0\0\0\0\x02\0");
    assert_eq!(
        Frame::decode(&oversized_metadata),
        Err(CodecError::TooLarge)
    );
}
#[test]
fn inner_identity_class_and_raw_record_and_membership_bytes_are_checked() {
    let mut x = fixtures().remove(2);
    if let Body::Exchange { class, .. } = &mut x.body {
        *class = PayloadClass::Evidence;
    }
    assert!(x.encode().is_err());
    let mut x = fixtures().remove(2);
    x.common.sender = NodeId([10; 32]);
    assert!(x.encode().is_err());
    let mut x = fixtures().remove(2);
    x.common.cluster = ClusterId([10; 32]);
    assert!(x.encode().is_err());
    let mut r = fixtures().remove(3);
    r.common.sender = NodeId([10; 32]);
    assert!(r.encode().is_err());
    let record = Record::from_bytes(&hex(include_str!(
        "../../../contracts/fixtures/create-v1.hex"
    )))
    .unwrap();
    let certificate = Certificate::from_bytes(&hex(include_str!(
        "../../../contracts/fixtures/membership-admit-v1.hex"
    )))
    .unwrap();
    let commands = [
        PeerCommand::Accept(Page {
            epoch: LogEpoch([9; 32]),
            after: 0,
            through: 1,
            high: 1,
            entries: vec![LogEntry {
                seq: 1,
                record: record.clone(),
            }],
        }),
        PeerCommand::MembershipAccept(Page {
            epoch: LogEpoch([9; 32]),
            after: 0,
            through: 1,
            high: 1,
            entries: vec![LogEntry {
                seq: 1,
                record: certificate.clone(),
            }],
        }),
    ];
    for (command, class, raw) in [
        (commands[0].clone(), PayloadClass::Data, record.bytes()),
        (
            commands[1].clone(),
            PayloadClass::Evidence,
            certificate.bytes(),
        ),
    ] {
        let original = request(command);
        let frame = frame(
            false,
            Body::Exchange {
                context: exchange_context(),
                class,
                request: original.clone(),
            },
        );
        let encoded = frame.encode().unwrap();
        assert!(encoded.windows(raw.len()).any(|window| window == raw));
        match Frame::decode(&encoded).unwrap().body {
            Body::Exchange { request, .. } => assert_eq!(request, original),
            _ => unreachable!(),
        }
    }
    let too_many = PeerRequest {
        cluster: ClusterId([1; 32]),
        node: NodeId([2; 32]),
        command: PeerCommand::Accept(Page {
            epoch: LogEpoch([9; 32]),
            after: 0,
            through: 129,
            high: 129,
            entries: vec![LogEntry { seq: 1, record }; MAX_PAGE_ENTRIES + 1],
        }),
    };
    assert_eq!(
        encode_peer_request(&too_many),
        Err(CodecError::TooManyItems)
    );
    let mut oversized = fixtures().remove(2);
    if let Body::Exchange { request, .. } = &mut oversized.body {
        *request = vec![0; MAX_PEER_BYTES + 1];
    }
    assert_eq!(oversized.encode(), Err(CodecError::TooLarge));
}
#[test]
fn every_error_has_fixed_status_and_unknown_or_mismatched_wire_values_are_rejected() {
    let errors = [
        (ErrorCode::InvalidEnvelope, 400),
        (ErrorCode::NotAuthorized, 403),
        (ErrorCode::ChallengeUnavailable, 409),
        (ErrorCode::BindingMismatch, 400),
        (ErrorCode::UnsupportedVersion, 400),
        (ErrorCode::Capacity, 429),
        (ErrorCode::StoreUnavailable, 503),
        (ErrorCode::InternalFailure, 500),
    ];
    for (code, status) in errors {
        assert_eq!(code.http_status(), status);
        let mut f = fixtures().remove(5);
        if let Body::Response { result, .. } = &mut f.body {
            *result = ResponseResult::Error(TransportError {
                code,
                retry_after_ms: u32::MAX,
            });
        }
        let bytes = f.encode().unwrap();
        assert_eq!(Frame::decode(&bytes).unwrap(), f);
        // Errors have a fixed six-byte result body followed by the signature.
        let code_start = bytes.len() - 64 - 6;
        let mut unknown = bytes.clone();
        unknown[code_start..code_start + 2].copy_from_slice(&99u16.to_be_bytes());
        assert_eq!(Frame::decode(&unknown), Err(CodecError::InvalidTag));
        let status_start = code_start - 4 - 1 - 2;
        let mut mismatch = bytes;
        mismatch[status_start..status_start + 2].copy_from_slice(&200u16.to_be_bytes());
        assert_eq!(Frame::decode(&mismatch), Err(CodecError::InvalidFieldValue));
    }
    let f = fixtures().remove(4);
    let mut bytes = f.encode().unwrap();
    let status_start = bytes.len() - 64 - 6 - 2;
    bytes[status_start..status_start + 2].copy_from_slice(&200u16.to_be_bytes());
    assert!(Frame::decode(&bytes).is_err());
}
#[test]
fn signing_input_binds_identity_origin_all_context_and_payload_but_not_signature() {
    let original = fixtures().remove(2);
    let baseline = original.signing_bytes().unwrap();
    let mut variants = Vec::new();
    let mut f = original.clone();
    f.common.audience = NodeId([7; 32]);
    variants.push(f);
    let mut f = original.clone();
    f.common.origin = "https://other.example".into();
    variants.push(f);
    for field in 0..5 {
        let mut f = original.clone();
        if let Body::Exchange { context, .. } = &mut f.body {
            let value = match field {
                0 => &mut context.challenge_request_hash,
                1 => &mut context.challenge_hash,
                2 => &mut context.client_nonce,
                3 => &mut context.generation,
                _ => &mut context.challenge_nonce,
            };
            value[0] ^= 1;
        }
        variants.push(f);
    }
    let mut f = original.clone();
    if let Body::Exchange { request: bytes, .. } = &mut f.body {
        *bytes = request(PeerCommand::Cursor {
            epoch: LogEpoch([8; 32]),
        });
    }
    variants.push(f);
    for changed in variants {
        assert_ne!(changed.signing_bytes().unwrap(), baseline);
    }
    let mut f = original.clone();
    f.signature[0] ^= 1;
    assert_eq!(f.signing_bytes().unwrap(), baseline);
    assert_ne!(f.hash().unwrap(), original.hash().unwrap());
    // No attempt binding or signature validation: deliberately invented hashes
    // and all-zero signatures remain syntax, not authenticated state.
    f.signature = [0; 64];
    let bytes = f.encode().unwrap();
    assert_eq!(Frame::decode(&bytes).unwrap(), f);
    let q = fixtures().remove(0);
    assert!(
        q.signing_bytes()
            .unwrap()
            .starts_with(b"harmion/https-peer/v2/challenge-request\0HHT\0")
    );
}
#[test]
fn ttl_and_commitment_bounds_are_numeric_not_frozen_deployment_policy() {
    let mut f = fixtures().remove(1);
    if let Body::Challenge {
        ttl_ms, commitment, ..
    } = &mut f.body
    {
        *ttl_ms = u32::MAX;
        commitment.inner_length = MAX_PEER_BYTES as u32;
    }
    assert_eq!(Frame::decode(&f.encode().unwrap()).unwrap(), f);
    let mut zero_ttl = f.clone();
    if let Body::Challenge { ttl_ms, .. } = &mut zero_ttl.body {
        *ttl_ms = 0;
    }
    assert_eq!(zero_ttl.encode(), Err(CodecError::InvalidNumber));
    for length in [0, 72, MAX_PEER_BYTES as u32 + 1, u32::MAX] {
        let mut invalid = f.clone();
        if let Body::Challenge { commitment, .. } = &mut invalid.body {
            commitment.inner_length = length;
        }
        assert!(invalid.encode().is_err());
    }
}
