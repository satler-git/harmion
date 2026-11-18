use harmion_protocol::{domain::*, *};
use std::collections::{BTreeMap, BTreeSet};

#[test]
fn json_view_keeps_arbitrary_object_keys_and_exact_numbers() {
    let value = AttributeValue::Object(BTreeMap::from([(
        "$serde_json::private::Number".into(),
        AttributeValue::String("123".into()),
    )]));
    let view = ReadView {
        content: Content {
            id: ContentId([1; 32]),
            body: "body".into(),
            attributes: BTreeMap::from([
                (
                    AttributeKey {
                        namespace: "test".into(),
                        key: "object".into(),
                    },
                    value,
                ),
                (
                    AttributeKey {
                        namespace: "test".into(),
                        key: "number".into(),
                    },
                    AttributeValue::Number("9007199254740993123456789".parse().unwrap()),
                ),
            ]),
            kinds: BTreeSet::new(),
        },
        edit_token: EditToken([2; 32]),
        conflicts: BTreeSet::new(),
        deletion_conflict: false,
    };
    let response = read_view_json(&view);
    let encoded = serde_json::to_vec(&response).unwrap();
    assert_eq!(read_view_size(&view, encoded.len()), Some(encoded.len()));
    assert_eq!(read_view_size(&view, encoded.len() - 1), None);
    let decoded = decode_app_json(&encoded).unwrap();
    let values = decoded["content"]["attributes"].as_array().unwrap();
    let object = &values.iter().find(|v| v["key"] == "object").unwrap()["value"];
    assert!(object.is_object(), "attribute object became {object}");
    assert_eq!(object["$serde_json::private::Number"], "123");
    assert_eq!(
        values.iter().find(|v| v["key"] == "number").unwrap()["value"].to_string(),
        "9007199254740993123456789"
    );
}

#[test]
fn export_preserves_attribute_types_and_kind_membership() {
    let cluster = ClusterId([1; 32]);
    let content = ContentId([2; 32]);
    let update = UpdateId([3; 32]);
    let fields = [
        (Field::Body, Value::Body("normal\r\n".into())),
        (Field::Kind("wiki".into()), Value::Kind(true)),
        (
            Field::Attribute {
                namespace: "app".into(),
                key: "count".into(),
            },
            Value::Attribute(AttributeValue::Number("9007199254740993".parse().unwrap())),
        ),
        (
            Field::Attribute {
                namespace: "app".into(),
                key: "present".into(),
            },
            Value::Attribute(AttributeValue::Null),
        ),
    ];
    let mut graph = Graph::new(cluster);
    graph
        .accept(Update {
            format_version: 1,
            cluster_id: cluster,
            content_id: content,
            root_id: update,
            update_id: update,
            author_node_id: NodeId([4; 32]),
            edited_at: -1,
            action: Action::Create,
            basis: BTreeSet::new(),
            changes: fields
                .into_iter()
                .map(|(field, value)| {
                    (
                        field,
                        Change {
                            value,
                            replaces: BTreeSet::new(),
                        },
                    )
                })
                .collect(),
        })
        .unwrap();
    let mut buffer = Vec::new();
    let mut writer = export::ExportWriter::start(&mut buffer).unwrap();
    writer.content(&graph, content).unwrap();
    writer.finish().unwrap();
    let records: Vec<_> = buffer
        .strip_suffix(b"\n")
        .unwrap()
        .split(|byte| *byte == b'\n')
        .map(|line| decode_app_json(line).unwrap())
        .collect();
    let field = |label: &str, key: &str| {
        records
            .iter()
            .find(|line| line["type"] == "field" && line["field"][label] == key)
            .unwrap()
    };
    assert_eq!(
        field("key", "count")["value"]["value"].to_string(),
        "9007199254740993"
    );
    assert!(field("key", "present")["value"]["value"].is_null());
    assert_eq!(field("name", "wiki")["value"]["value"], true);
    assert_eq!(
        records.last().unwrap()["records"].as_u64(),
        Some(records.len() as u64 - 2)
    );
}
