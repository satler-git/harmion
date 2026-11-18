use crate::{EditToken, MAX_APP_RESPONSE_BYTES, hex};
use harmion_domain::*;
use serde::{
    Serialize, Serializer,
    ser::{SerializeMap, SerializeSeq},
};
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
struct AttributeRef<'a>(&'a AttributeValue);

impl Serialize for AttributeRef<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            AttributeValue::Null => serializer.serialize_unit(),
            AttributeValue::Bool(b) => serializer.serialize_bool(*b),
            AttributeValue::Number(n) => n
                .to_string()
                .parse::<serde_json::Number>()
                .expect("validated decimal with arbitrary precision")
                .serialize(serializer),
            AttributeValue::String(s) => serializer.serialize_str(s),
            AttributeValue::Array(values) => {
                let mut sequence = serializer.serialize_seq(Some(values.len()))?;
                for value in values {
                    sequence.serialize_element(&AttributeRef(value))?;
                }
                sequence.end()
            }
            AttributeValue::Object(values) => {
                let mut map = serializer.serialize_map(Some(values.len()))?;
                for (key, value) in values {
                    map.serialize_entry(key, &AttributeRef(value))?;
                }
                map.end()
            }
        }
    }
}

struct Attributes<'a>(&'a BTreeMap<AttributeKey, AttributeValue>);

impl Serialize for Attributes<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Entry<'a> {
            namespace: &'a str,
            key: &'a str,
            value: AttributeRef<'a>,
        }
        let mut sequence = serializer.serialize_seq(Some(self.0.len()))?;
        for (key, value) in self.0 {
            sequence.serialize_element(&Entry {
                namespace: &key.namespace,
                key: &key.key,
                value: AttributeRef(value),
            })?;
        }
        sequence.end()
    }
}

struct Conflicts<'a>(&'a BTreeSet<Field>);

impl Serialize for Conflicts<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(self.0.len()))?;
        for field in self.0 {
            sequence.serialize_element(&field_json(field))?;
        }
        sequence.end()
    }
}

#[derive(Serialize)]
struct ContentRef<'a> {
    id: String,
    body: &'a str,
    attributes: Attributes<'a>,
    kinds: &'a BTreeSet<String>,
}

#[derive(Serialize)]
struct ReadViewRef<'a> {
    content: ContentRef<'a>,
    edit_token: String,
    conflicts: Conflicts<'a>,
    deletion_conflict: bool,
}

impl<'a> From<&'a ReadView> for ReadViewRef<'a> {
    fn from(view: &'a ReadView) -> Self {
        Self {
            content: ContentRef {
                id: hex(&view.content.id.0),
                body: &view.content.body,
                attributes: Attributes(&view.content.attributes),
                kinds: &view.content.kinds,
            },
            edit_token: hex(&view.edit_token.0),
            conflicts: Conflicts(&view.conflicts),
            deletion_conflict: view.deletion_conflict,
        }
    }
}

pub(crate) fn value_json(value: &Value) -> Json {
    match value {
        Value::Body(s) => json!({"type":"body","value":s}),
        Value::Attribute(v) => json!({"type":"attribute","value":AttributeRef(v)}),
        Value::Absent => json!({"type":"absent"}),
        Value::Kind(b) => json!({"type":"kind","value":b}),
    }
}
pub fn read_view_json(view: &ReadView) -> Json {
    serde_json::to_value(ReadViewRef::from(view)).expect("read view is serializable")
}

/// Counts the encoded view without constructing a JSON tree or serialized body.
pub fn read_view_size(view: &ReadView, limit: usize) -> Option<usize> {
    struct Budget(usize);
    impl std::io::Write for Budget {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self
                .0
                .checked_sub(bytes.len())
                .ok_or(std::io::ErrorKind::FileTooLarge)?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut budget = Budget(limit);
    serde_json::to_writer(&mut budget, &ReadViewRef::from(view)).ok()?;
    Some(limit - budget.0)
}
pub fn conflict_view_json(view: &ConflictView) -> Json {
    let candidates: Vec<_> = view.candidates.iter().map(|(field, groups)| json!({"field":field_json(field),
        "candidates":groups.iter().map(|group| json!({"value":value_json(&group.value),
            "versions":group.versions.iter().map(|v| hex(&v.0)).collect::<Vec<_>>()})).collect::<Vec<_>>() })).collect();
    json!({"view":read_view_json(&view.view),"candidates":candidates,"deleted":view.deleted})
}
pub fn save_result_json(result: &SaveResult) -> Json {
    // The required receipt must fit regardless of the accumulated Content size.
    const ENVELOPE_BYTES: usize = br#"{"update_id":"","saved":true,"view":}"#.len() + 64;
    let mut receipt = json!({"update_id":hex(&result.update_id.0),"saved":true,"view":null});
    if let Some(view) = &result.view {
        if read_view_size(view, MAX_APP_RESPONSE_BYTES - ENVELOPE_BYTES).is_some() {
            receipt["view"] = read_view_json(view);
        } else {
            receipt["view_error"] = json!("response_too_large");
        }
    }
    receipt
}
