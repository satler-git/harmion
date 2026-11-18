use harmion_domain::*;
use harmion_protocol::*;
use std::collections::{BTreeMap, BTreeSet};

#[test]
fn save_commands_roundtrip_without_losing_request_identity() {
    let fields = BTreeMap::from([(Field::Body, Value::Body("本文".into()))]);
    let token = EditToken([7; 32]);
    let commands = [
        Command::Create {
            fields: fields.clone(),
        },
        Command::Edit {
            token,
            fields: fields.clone(),
        },
        Command::Resolve {
            token,
            changes: BTreeMap::from([(
                Field::Body,
                Change {
                    value: Value::Body("resolved".into()),
                    replaces: BTreeSet::from([UpdateId([2; 32]), UpdateId([3; 32])]),
                },
            )]),
        },
        Command::Delete { token },
        Command::Keep { token },
        Command::Restore { token, fields },
    ];
    for command in commands {
        let request = SaveRequest {
            request_id: RequestId([1; 32]),
            content_id: ContentId([2; 32]),
            command,
        };
        let bytes = encode_save_request(&request).unwrap();
        assert_eq!(decode_save_request(&bytes).unwrap(), request);
        for len in 0..bytes.len() {
            assert!(decode_save_request(&bytes[..len]).is_err());
        }
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert_eq!(
            decode_save_request(&trailing),
            Err(CodecError::TrailingBytes)
        );
        let mut changed = request.clone();
        changed.request_id = RequestId([9; 32]);
        assert_ne!(encode_save_request(&changed).unwrap(), bytes);
    }
}

#[test]
fn wire_cannot_smuggle_duplicate_fields_or_unknown_actions() {
    let request = SaveRequest {
        request_id: RequestId([1; 32]),
        content_id: ContentId([2; 32]),
        command: Command::Create {
            fields: BTreeMap::from([(Field::Body, Value::Body("x".into()))]),
        },
    };
    let bytes = encode_save_request(&request).unwrap();
    let mut unknown = bytes.clone();
    unknown[72] = 255;
    assert_eq!(decode_save_request(&unknown), Err(CodecError::InvalidTag));
    let mut duplicate = bytes.clone();
    duplicate[76] = 2;
    duplicate.extend_from_slice(&bytes[77..]);
    assert_eq!(decode_save_request(&duplicate), Err(CodecError::Duplicate));
    assert_eq!(format!("{:?}", EditToken([1; 32])), "EditToken([redacted])");
}
