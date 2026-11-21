use harmion_protocol::{domain::*, *};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

fn fixture() -> ContentSnapshot {
    let attr = Field::Attribute {
        namespace: "space\"\n".into(),
        key: "雪\u{1}".into(),
    };
    let special = AttributeValue::Object(BTreeMap::from([
        (
            "$serde_json::private::Number".into(),
            AttributeValue::String("123".into()),
        ),
        (
            "number".into(),
            AttributeValue::Number("9007199254740993123456789".parse().unwrap()),
        ),
        (
            "nested".into(),
            AttributeValue::Array(vec![AttributeValue::Null, AttributeValue::Bool(true)]),
        ),
    ]));
    let fields = [
        (
            Field::Body,
            vec![
                Value::Body("雪\"\n\u{1}".into()),
                Value::Body("other".into()),
            ],
        ),
        (
            attr,
            vec![
                Value::Attribute(special.clone()),
                Value::Absent,
                Value::Attribute(AttributeValue::Null),
            ],
        ),
        (
            Field::Kind("wiki\"\n".into()),
            vec![Value::Kind(true), Value::Kind(false)],
        ),
    ]
    .into_iter()
    .map(|(field, values)| {
        (
            field,
            FieldState {
                selected: UpdateId([1; 32]),
                candidates: values
                    .into_iter()
                    .enumerate()
                    .map(|(index, value)| Candidate {
                        value,
                        versions: BTreeSet::from([
                            UpdateId([index as u8 + 1; 32]),
                            UpdateId([index as u8 + 11; 32]),
                        ]),
                    })
                    .collect(),
            },
        )
    })
    .collect();
    ContentSnapshot {
        content: Content {
            id: ContentId([9; 32]),
            body: "雪\"\n\u{1}".into(),
            attributes: BTreeMap::from([(
                AttributeKey {
                    namespace: "space\"\n".into(),
                    key: "雪\u{1}".into(),
                },
                special,
            )]),
            kinds: BTreeSet::from(["wiki\"\n".into()]),
        },
        life: Life::Conflict,
        last_edited_at: 0,
        basis: BTreeSet::from([UpdateId([1; 32])]),
        fields,
    }
}
fn output(snapshot: &ContentSnapshot, include_kinds: bool) -> ConflictView {
    let visible = |field: &&Field| include_kinds || !matches!(field, Field::Kind(_));
    ConflictView {
        view: ReadView {
            content: snapshot.content.clone(),
            edit_token: EditToken([3; 32]),
            conflicts: snapshot
                .fields
                .iter()
                .filter(|(field, state)| visible(field) && state.is_conflicted())
                .map(|(field, _)| field.clone())
                .collect(),
            deletion_conflict: snapshot.life == Life::Conflict,
        },
        candidates: snapshot
            .fields
            .iter()
            .filter(|(field, _)| visible(field))
            .map(|(field, state)| (field.clone(), state.candidates.clone()))
            .collect(),
        deleted: snapshot.life == Life::Deleted,
    }
}
#[test]
fn snapshot_and_owned_sizes_match_exact_complete_wire_envelopes() {
    let mut snapshot = fixture();
    for life in [Life::Live, Life::Conflict, Life::Deleted] {
        snapshot.life = life;
        for include_kinds in [false, true] {
            let result = output(&snapshot, include_kinds);
            let read_bytes = serde_json::to_vec(&read_view_json(&result.view)).unwrap();
            let conflict_json = conflict_view_json(&result);
            let conflict_bytes = serde_json::to_vec(&conflict_json).unwrap();
            let n = read_bytes.len();
            assert_eq!(read_view_size(&result.view, n), Some(n));
            assert_eq!(read_view_size(&result.view, n - 1), None);
            assert_eq!(
                snapshot_read_view_size(&snapshot, include_kinds, n),
                Some(n)
            );
            assert_eq!(
                snapshot_read_view_size(&snapshot, include_kinds, n - 1),
                None
            );
            let n = conflict_bytes.len();
            assert_eq!(conflict_view_size(&result, n), Some(n));
            assert_eq!(conflict_view_size(&result, n - 1), None);
            assert_eq!(
                snapshot_conflict_view_size(&snapshot, include_kinds, n),
                Some(n)
            );
            assert_eq!(
                snapshot_conflict_view_size(&snapshot, include_kinds, n - 1),
                None
            );
            assert_eq!(
                snapshot_conflict_view_size(&snapshot, include_kinds, 0),
                None
            );
            assert_eq!(conflict_json["deleted"], life == Life::Deleted);
            assert_eq!(
                conflict_json["view"]["deletion_conflict"],
                life == Life::Conflict
            );
            assert_eq!(
                conflict_json["candidates"].as_array().unwrap().len(),
                if include_kinds { 3 } else { 2 }
            );
            // Assert the pre-existing wire shapes, not just two shared encoders.
            let body = &conflict_json["candidates"][0];
            assert_eq!(body["field"], json!({"type":"body"}));
            assert_eq!(
                body["candidates"][0]["value"],
                json!({"type":"body", "value":"雪\"\n\u{1}"})
            );
            assert_eq!(
                body["candidates"][0]["versions"],
                json!([hex(&[1; 32]), hex(&[11; 32])])
            );
            let attribute = &conflict_json["candidates"][1]["candidates"];
            assert_eq!(attribute[1]["value"], json!({"type":"absent"}));
            assert_eq!(
                attribute[2]["value"],
                json!({"type":"attribute", "value":null})
            );
            let decoded = decode_app_json(&conflict_bytes).unwrap();
            let object = &decoded["candidates"][1]["candidates"][0]["value"]["value"];
            assert_eq!(object["$serde_json::private::Number"], "123");
            assert_eq!(object["number"].to_string(), "9007199254740993123456789");
            if include_kinds {
                assert_eq!(
                    conflict_json["candidates"][2]["candidates"][1]["value"],
                    json!({"type":"kind", "value":false})
                );
            }
        }
    }
    let hidden = snapshot_conflict_view_size(&snapshot, false, usize::MAX).unwrap();
    assert_eq!(snapshot_conflict_view_size(&snapshot, true, hidden), None);
}
