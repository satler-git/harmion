use harmion_protocol::domain::{ClusterId, NodeId};
use harmion_protocol::{membership::*, *};

fn fixture(retire: bool) -> Vec<u8> {
    unhex_bytes(
        if retire {
            include_str!("../../../contracts/fixtures/membership-retire-v1.hex")
        } else {
            include_str!("../../../contracts/fixtures/membership-admit-v1.hex")
        }
        .trim(),
        MAX_CERTIFICATE_BYTES,
    )
    .unwrap()
}

#[test]
fn canonical_membership_bytes_match_independent_fixtures() {
    for retire in [false, true] {
        let action = if retire {
            Action::Retire {
                target: NodeId([5; 32]),
                retained: [[8; 32], [9; 32]].into(),
            }
        } else {
            Action::Admit(Member {
                node: NodeId([5; 32]),
                signing_key: [6; 32],
                binding: Binding::Iroh([7; 32]),
                endpoint: String::new(),
            })
        };
        let statement = Statement {
            cluster: ClusterId([1; 32]),
            issuer: NodeId([2; 32]),
            signing_key: [3; 32],
            nonce: [4; 32],
            action,
        };
        let certificate = Certificate::new(statement, [0; 64]).unwrap();
        assert_eq!(certificate.bytes(), fixture(retire));
        assert_eq!(
            Certificate::from_bytes(certificate.bytes()).unwrap(),
            certificate
        );
        for end in 0..certificate.bytes().len() {
            assert!(Certificate::from_bytes(&certificate.bytes()[..end]).is_err());
        }
        let mut extra = certificate.bytes().to_vec();
        extra.push(0);
        assert!(matches!(
            Certificate::from_bytes(&extra),
            Err(CodecError::TrailingBytes)
        ));
    }
    let mut unordered = fixture(true);
    // Header 136 bytes, tag + target + count; retained IDs must be strictly sorted.
    unordered[173..205].fill(9);
    unordered[205..237].fill(8);
    assert!(matches!(
        Certificate::from_bytes(&unordered),
        Err(CodecError::NonCanonical)
    ));
}

#[test]
fn control_pages_roundtrip_in_a_separate_command_space() {
    let certificate = Certificate::from_bytes(&fixture(false)).unwrap();
    let page = Page {
        epoch: LogEpoch([9; 32]),
        after: 0,
        through: 1,
        high: 1,
        entries: vec![LogEntry {
            seq: 1,
            record: certificate,
        }],
    };
    for command in [
        PeerCommand::MembershipStatus,
        PeerCommand::MembershipCursor { epoch: page.epoch },
        PeerCommand::MembershipPage {
            epoch: page.epoch,
            after: 0,
            high: 1,
        },
        PeerCommand::MembershipAccept(page.clone()),
    ] {
        let request = PeerRequest {
            cluster: ClusterId([1; 32]),
            node: NodeId([2; 32]),
            command,
        };
        let encoded = encode_peer_request(&request).unwrap();
        assert_eq!(decode_peer_request(&encoded).unwrap(), request);
        for end in 0..encoded.len() {
            assert!(decode_peer_request(&encoded[..end]).is_err());
        }
    }
    let response = PeerResponse {
        cluster: ClusterId([1; 32]),
        node: NodeId([2; 32]),
        reply: PeerReply::MembershipPage(page),
    };
    assert_eq!(
        decode_peer_response(&encode_peer_response(&response).unwrap()).unwrap(),
        response
    );
}
