use harmion_domain::*;
use harmion_protocol::*;

fn record() -> Record {
    let hex = include_str!("../../../contracts/fixtures/create-v1.hex").trim();
    let bytes: Vec<_> = hex
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect();
    Record::from_bytes(&bytes).unwrap()
}
fn page() -> Page {
    Page {
        epoch: LogEpoch([9; 32]),
        after: 0,
        through: 4,
        high: 4,
        entries: vec![LogEntry {
            seq: 4,
            record: record(),
        }],
    }
}

#[test]
fn peer_wire_roundtrips_exact_records_and_rejects_truncation() {
    let commands = [
        PeerCommand::Status,
        PeerCommand::Cursor {
            epoch: page().epoch,
        },
        PeerCommand::Page {
            epoch: page().epoch,
            after: 0,
            high: 4,
        },
        PeerCommand::Accept(page()),
    ];
    for command in commands {
        let request = PeerRequest {
            cluster: ClusterId([1; 32]),
            node: NodeId([2; 32]),
            command,
        };
        let bytes = encode_peer_request(&request).unwrap();
        assert_eq!(decode_peer_request(&bytes).unwrap(), request);
        for n in 0..bytes.len() {
            assert!(decode_peer_request(&bytes[..n]).is_err());
        }
        let mut trailing = bytes;
        trailing.push(0);
        assert_eq!(
            decode_peer_request(&trailing),
            Err(CodecError::TrailingBytes)
        );
    }
    for reply in [
        PeerReply::Status {
            epoch: page().epoch,
            high: 4,
        },
        PeerReply::Cursor {
            epoch: page().epoch,
            through: 4,
        },
        PeerReply::Page(page()),
        PeerReply::Ack {
            epoch: page().epoch,
            through: 4,
        },
        PeerReply::CursorMismatch {
            epoch: page().epoch,
            current: 4,
        },
    ] {
        let response = PeerResponse {
            cluster: ClusterId([1; 32]),
            node: NodeId([2; 32]),
            reply,
        };
        let bytes = encode_peer_response(&response).unwrap();
        assert_eq!(decode_peer_response(&bytes).unwrap(), response);
        for n in 0..bytes.len() {
            assert!(decode_peer_response(&bytes[..n]).is_err());
        }
    }
}

#[test]
fn incompatible_limits_oversized_pages_and_unknown_tags_fail_closed() {
    let response = PeerResponse {
        cluster: ClusterId([1; 32]),
        node: NodeId([2; 32]),
        reply: PeerReply::Status {
            epoch: page().epoch,
            high: 4,
        },
    };
    let mut bytes = encode_peer_response(&response).unwrap();
    *bytes.last_mut().unwrap() ^= 1;
    assert_eq!(decode_peer_response(&bytes), Err(CodecError::NonCanonical));
    let mut p = page();
    p.entries = vec![p.entries[0].clone(); MAX_PAGE_ENTRIES + 1];
    let request = PeerRequest {
        cluster: ClusterId([1; 32]),
        node: NodeId([2; 32]),
        command: PeerCommand::Accept(p),
    };
    assert_eq!(encode_peer_request(&request), Err(CodecError::TooManyItems));
    let mut request = encode_peer_request(&PeerRequest {
        cluster: ClusterId([1; 32]),
        node: NodeId([2; 32]),
        command: PeerCommand::Status,
    })
    .unwrap();
    request[72] = 255;
    assert_eq!(decode_peer_request(&request), Err(CodecError::InvalidTag));
    assert_eq!(
        decode_peer_request(&vec![0; MAX_PEER_BYTES + 1]),
        Err(CodecError::TooLarge)
    );
}

#[test]
fn status_wire_matches_independent_python_fixture() {
    fn bytes(text: &str) -> Vec<u8> {
        text.trim()
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
            .collect()
    }
    let request = PeerRequest {
        cluster: ClusterId([1; 32]),
        node: NodeId([2; 32]),
        command: PeerCommand::Status,
    };
    assert_eq!(
        encode_peer_request(&request).unwrap(),
        bytes(include_str!(
            "../../../contracts/fixtures/peer-status-request-v1.hex"
        ))
    );
    let response = PeerResponse {
        cluster: ClusterId([1; 32]),
        node: NodeId([2; 32]),
        reply: PeerReply::Status {
            epoch: LogEpoch([9; 32]),
            high: 4,
        },
    };
    assert_eq!(
        encode_peer_response(&response).unwrap(),
        bytes(include_str!(
            "../../../contracts/fixtures/peer-status-response-v1.hex"
        ))
    );
}

#[test]
fn rejected_record_id_and_reason_survive_the_wire() {
    for reason in [
        Rejection::MissingDependency,
        Rejection::IdMismatch,
        Rejection::InvalidUpdate,
    ] {
        let response = PeerResponse {
            cluster: ClusterId([1; 32]),
            node: NodeId([2; 32]),
            reply: PeerReply::Rejected {
                update_id: UpdateId([8; 32]),
                reason,
            },
        };
        let bytes = encode_peer_response(&response).unwrap();
        assert_eq!(decode_peer_response(&bytes).unwrap(), response);
    }
}
