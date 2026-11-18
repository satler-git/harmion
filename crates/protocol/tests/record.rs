use harmion_domain::*;
use harmion_protocol::*;
use std::collections::{BTreeMap, BTreeSet};

fn fixture() -> Update {
    let array = AttributeValue::Array(vec![
        AttributeValue::Null,
        AttributeValue::Bool(false),
        AttributeValue::Bool(true),
        AttributeValue::Number("9007199254740993".parse().unwrap()),
        AttributeValue::String(String::new()),
    ]);
    let data = AttributeValue::Object(BTreeMap::from([
        ("array".into(), array),
        ("object".into(), AttributeValue::Object(BTreeMap::new())),
    ]));
    Update {
        format_version: 1,
        cluster_id: ClusterId([1; 32]),
        content_id: ContentId([2; 32]),
        root_id: UpdateId([3; 32]),
        update_id: UpdateId([3; 32]),
        author_node_id: NodeId([4; 32]),
        edited_at: i64::MIN,
        action: Action::Create,
        basis: BTreeSet::new(),
        changes: BTreeMap::from([
            (
                Field::Body,
                Change {
                    value: Value::Body("本文\n".into()),
                    replaces: BTreeSet::new(),
                },
            ),
            (
                attr("data"),
                Change {
                    value: Value::Attribute(data),
                    replaces: BTreeSet::new(),
                },
            ),
            (
                attr("unset"),
                Change {
                    value: Value::Absent,
                    replaces: BTreeSet::new(),
                },
            ),
            (
                Field::Kind("wiki".into()),
                Change {
                    value: Value::Kind(true),
                    replaces: BTreeSet::new(),
                },
            ),
        ]),
    }
}
fn attr(key: &str) -> Field {
    Field::Attribute {
        namespace: "app".into(),
        key: key.into(),
    }
}
fn unhex(s: &str) -> Vec<u8> {
    s.trim()
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect()
}
fn raw_string(s: &str) -> Vec<u8> {
    let mut b = (s.len() as u32).to_be_bytes().to_vec();
    b.extend_from_slice(s.as_bytes());
    b
}
// A header with no basis and exactly one field, independent from Encoder internals.
fn header() -> Vec<u8> {
    let mut b = b"HUP\0".to_vec();
    b.extend_from_slice(&1u32.to_be_bytes());
    for n in [1, 2, 3, 3, 4] {
        b.extend_from_slice(&[n; 32]);
    }
    b.extend_from_slice(&i64::MIN.to_be_bytes());
    b.push(0);
    b.extend_from_slice(&0u32.to_be_bytes());
    b.extend_from_slice(&1u32.to_be_bytes());
    b
}
fn raw_attribute(value: &[u8]) -> Vec<u8> {
    let mut b = header();
    b.push(1);
    b.extend(raw_string("app"));
    b.extend(raw_string("data"));
    b.push(1);
    b.extend_from_slice(value);
    b.extend_from_slice(&0u32.to_be_bytes());
    b
}

#[test]
fn independent_fixture_bytes_and_digest_are_stable() {
    let bytes = unhex(include_str!("../../../contracts/fixtures/create-v1.hex"));
    let digest = unhex(include_str!("../../../contracts/fixtures/create-v1.sha256"));
    assert_eq!(encode(&fixture()).unwrap(), bytes);
    assert_eq!(decode(&bytes).unwrap(), fixture());
    let record = Record::from_bytes(&bytes).unwrap();
    assert_eq!(record, Record::from_update(fixture()).unwrap());
    assert_eq!(record.digest().as_slice(), digest);
    assert_eq!(record.bytes(), bytes);
    let mut graph = Graph::new(fixture().cluster_id);
    graph.accept(record.update().clone()).unwrap();
}

#[test]
fn all_actions_and_exact_values_roundtrip() {
    for action in [
        Action::Create,
        Action::Edit,
        Action::Delete,
        Action::Keep,
        Action::Restore,
    ] {
        let mut update = fixture();
        update.action = action;
        update.edited_at = i64::MAX;
        update.basis = BTreeSet::from([UpdateId([5; 32]), UpdateId([6; 32])]);
        update
            .changes
            .get_mut(&Field::Kind("wiki".into()))
            .unwrap()
            .value = Value::Kind(false);
        update.changes.get_mut(&Field::Body).unwrap().replaces = update.basis.clone();
        assert_eq!(decode(&encode(&update).unwrap()).unwrap(), update);
    }
    // Codec shape validation is separate from domain dependency/action validation.
    for number in [
        "0",
        "-1",
        "1e-9223372036854775808",
        "1e9223372036854775807",
        "123456789012345678901234567890",
    ] {
        let mut u = fixture();
        u.changes.get_mut(&attr("data")).unwrap().value =
            Value::Attribute(AttributeValue::Number(number.parse().unwrap()));
        assert_eq!(decode(&encode(&u).unwrap()).unwrap(), u);
    }
}

#[test]
fn truncation_unknown_versions_tags_utf8_and_trailing_bytes_are_rejected() {
    let bytes = encode(&fixture()).unwrap();
    for len in 0..bytes.len() {
        assert!(decode(&bytes[..len]).is_err(), "prefix {len}");
    }
    let mut b = bytes.clone();
    b[0] = b'X';
    assert_eq!(decode(&b), Err(CodecError::BadMagic));
    let mut b = bytes.clone();
    b[7] = 2;
    assert_eq!(decode(&b), Err(CodecError::UnsupportedVersion(2)));
    let mut b = bytes.clone();
    b[176] = 255;
    assert_eq!(decode(&b), Err(CodecError::InvalidTag));
    let mut b = bytes.clone();
    b.push(0);
    assert_eq!(decode(&b), Err(CodecError::TrailingBytes));
    let mut b = header();
    b.extend([0, 0]);
    b.extend_from_slice(&1u32.to_be_bytes());
    b.push(255);
    b.extend_from_slice(&0u32.to_be_bytes());
    assert_eq!(decode(&b), Err(CodecError::InvalidUtf8));
    assert_eq!(decode(&raw_attribute(&[255])), Err(CodecError::InvalidTag));
    let mut u = fixture();
    u.changes.get_mut(&Field::Body).unwrap().value = Value::Absent;
    assert_eq!(encode(&u), Err(CodecError::InvalidFieldValue));
}

#[test]
fn duplicate_and_unsorted_maps_sets_and_number_aliases_are_rejected() {
    // Duplicate and reverse-ordered object keys.
    for (keys, expected) in [
        (["a", "a"], CodecError::Duplicate),
        (["b", "a"], CodecError::NonCanonical),
    ] {
        let mut value = vec![6];
        value.extend_from_slice(&2u32.to_be_bytes());
        for key in keys {
            value.extend(raw_string(key));
            value.push(0);
        }
        assert_eq!(decode(&raw_attribute(&value)), Err(expected));
    }
    for (values, expected) in [
        ([5, 5], CodecError::Duplicate),
        ([6, 5], CodecError::NonCanonical),
    ] {
        let mut b = header();
        b.truncate(177);
        b.extend_from_slice(&2u32.to_be_bytes());
        for v in values {
            b.extend_from_slice(&[v; 32]);
        }
        b.extend_from_slice(&0u32.to_be_bytes());
        assert_eq!(decode(&b), Err(expected));
    }
    let mut b = header();
    b[184] = 2;
    for _ in 0..2 {
        b.extend([0, 0]);
        b.extend(raw_string("body"));
        b.extend_from_slice(&0u32.to_be_bytes());
    }
    assert_eq!(decode(&b), Err(CodecError::Duplicate));
    for text in ["1.0", "-0", "1E2", "100"] {
        let mut value = vec![3];
        value.extend(raw_string(text));
        assert_eq!(
            decode(&raw_attribute(&value)),
            Err(CodecError::NonCanonical)
        );
    }
    let mut value = vec![3];
    value.extend(raw_string("NaN"));
    assert_eq!(
        decode(&raw_attribute(&value)),
        Err(CodecError::InvalidNumber)
    );
}

#[test]
fn resource_limits_apply_to_both_directions() {
    assert_eq!(
        decode(&vec![0; MAX_RECORD_BYTES + 1]),
        Err(CodecError::TooLarge)
    );
    let mut u = fixture();
    u.changes.get_mut(&Field::Body).unwrap().value = Value::Body("x".repeat(MAX_RECORD_BYTES));
    assert_eq!(encode(&u), Err(CodecError::TooLarge));
    let mut u = fixture();
    let mut nested = AttributeValue::Null;
    let mut raw = Vec::new();
    for _ in 0..=MAX_DEPTH {
        nested = AttributeValue::Array(vec![nested]);
        raw.push(5);
        raw.extend_from_slice(&1u32.to_be_bytes());
    }
    raw.push(0);
    u.changes.get_mut(&attr("data")).unwrap().value = Value::Attribute(nested);
    assert_eq!(encode(&u), Err(CodecError::TooDeep));
    assert_eq!(decode(&raw_attribute(&raw)), Err(CodecError::TooDeep));
    let mut raw = vec![5];
    raw.extend_from_slice(&u32::MAX.to_be_bytes());
    assert_eq!(decode(&raw_attribute(&raw)), Err(CodecError::TooManyItems));
    u.changes.get_mut(&attr("data")).unwrap().value =
        Value::Attribute(AttributeValue::Array(vec![AttributeValue::Null; MAX_ITEMS]));
    assert_eq!(encode(&u), Err(CodecError::TooManyItems));
}

#[test]
fn changed_bytes_never_silently_normalize() {
    let original = encode(&fixture()).unwrap();
    for offset in 0..original.len() {
        for mask in [1, 127, 255] {
            let mut bytes = original.clone();
            bytes[offset] ^= mask;
            if let Ok(update) = decode(&bytes) {
                assert_eq!(encode(&update).unwrap(), bytes);
            }
        }
    }
}
