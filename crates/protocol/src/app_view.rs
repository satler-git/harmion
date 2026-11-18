use crate::{EditToken, hex};
use harmion_domain::*;
use serde_json::{Value as Json, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadView {
    pub content: Content,
    pub edit_token: EditToken,
    pub conflicts: BTreeSet<Field>,
    pub deletion_conflict: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictView {
    pub view: ReadView,
    pub candidates: BTreeMap<Field, Vec<Candidate>>,
    pub deleted: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveResult {
    pub update_id: UpdateId,
    pub view: Option<ReadView>,
}

pub fn field_json(field: &Field) -> Json {
    match field {
        Field::Body => json!({"type":"body"}),
        Field::Attribute { namespace, key } => {
            json!({"type":"attribute","namespace":namespace,"key":key})
        }
        Field::Kind(name) => json!({"type":"kind","name":name}),
    }
}
fn attribute_json(value: &AttributeValue) -> Json {
    match value {
        AttributeValue::Null => Json::Null,
        AttributeValue::Bool(b) => Json::Bool(*b),
        AttributeValue::Number(n) => Json::Number(
            n.to_string()
                .parse()
                .expect("validated decimal with arbitrary precision"),
        ),
        AttributeValue::String(s) => Json::String(s.clone()),
        AttributeValue::Array(values) => Json::Array(values.iter().map(attribute_json).collect()),
        AttributeValue::Object(values) => Json::Object(
            values
                .iter()
                .map(|(k, v)| (k.clone(), attribute_json(v)))
                .collect(),
        ),
    }
}
fn value_json(value: &Value) -> Json {
    match value {
        Value::Body(s) => json!({"type":"body","value":s}),
        Value::Attribute(v) => json!({"type":"attribute","value":attribute_json(v)}),
        Value::Absent => json!({"type":"absent"}),
        Value::Kind(b) => json!({"type":"kind","value":b}),
    }
}
pub fn read_view_json(view: &ReadView) -> Json {
    let attributes: Vec<_> = view
        .content
        .attributes
        .iter()
        .map(|(key, value)| {
            json!({
                "namespace":key.namespace,"key":key.key,"value":attribute_json(value)
            })
        })
        .collect();
    json!({"content":{"id":hex(&view.content.id.0),"body":view.content.body,"attributes":attributes,"kinds":view.content.kinds},
        "edit_token":hex(&view.edit_token.0),"conflicts":view.conflicts.iter().map(field_json).collect::<Vec<_>>(),
        "deletion_conflict":view.deletion_conflict})
}
pub fn conflict_view_json(view: &ConflictView) -> Json {
    let candidates: Vec<_> = view.candidates.iter().map(|(field, groups)| json!({"field":field_json(field),
        "candidates":groups.iter().map(|group| json!({"value":value_json(&group.value),
            "versions":group.versions.iter().map(|v| hex(&v.0)).collect::<Vec<_>>()})).collect::<Vec<_>>() })).collect();
    json!({"view":read_view_json(&view.view),"candidates":candidates,"deleted":view.deleted})
}
pub fn save_result_json(result: &SaveResult) -> Json {
    json!({"update_id":hex(&result.update_id.0),"saved":true,"view":result.view.as_ref().map(read_view_json)})
}
