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
