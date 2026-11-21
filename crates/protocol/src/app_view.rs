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

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum FieldRef<'a> {
    Body,
    Attribute { namespace: &'a str, key: &'a str },
    Kind { name: &'a str },
}
impl<'a> From<&'a Field> for FieldRef<'a> {
    fn from(field: &'a Field) -> Self {
        match field {
            Field::Body => Self::Body,
            Field::Attribute { namespace, key } => Self::Attribute { namespace, key },
            Field::Kind(name) => Self::Kind { name },
        }
    }
}
pub fn field_json(field: &Field) -> Json {
    serde_json::to_value(FieldRef::from(field)).expect("field is serializable")
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

/// Serialize borrowed elements one at a time without collecting fields,
/// candidates, or hexadecimal version IDs into another vector.
struct Sequence<I>(I);
impl<I> Serialize for Sequence<I>
where
    I: Iterator + Clone,
    I::Item: Serialize,
{
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(None)?;
        for value in self.0.clone() {
            sequence.serialize_element(&value)?;
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
impl<'a> From<&'a Content> for ContentRef<'a> {
    fn from(content: &'a Content) -> Self {
        Self {
            id: hex(&content.id.0),
            body: &content.body,
            attributes: Attributes(&content.attributes),
            kinds: &content.kinds,
        }
    }
}
#[derive(Serialize)]
struct ReadViewRef<'a, C> {
    content: ContentRef<'a>,
    edit_token: String,
    conflicts: C,
    deletion_conflict: bool,
}
fn read_view_ref(view: &ReadView) -> impl Serialize + '_ {
    ReadViewRef {
        content: ContentRef::from(&view.content),
        edit_token: hex(&view.edit_token.0),
        conflicts: Sequence(view.conflicts.iter().map(FieldRef::from)),
        deletion_conflict: view.deletion_conflict,
    }
}
fn visible_field(field: &Field, include_kinds: bool) -> bool {
    include_kinds || !matches!(field, Field::Kind(_))
}
fn snapshot_read_ref(snapshot: &ContentSnapshot, include_kinds: bool) -> impl Serialize + '_ {
    ReadViewRef {
        content: ContentRef::from(&snapshot.content),
        // The placeholder has exactly the same wire length as any minted token.
        edit_token: hex(&[0; 32]),
        conflicts: Sequence(
            snapshot
                .fields
                .iter()
                .filter(move |(field, state)| {
                    state.is_conflicted() && visible_field(field, include_kinds)
                })
                .map(|(field, _)| FieldRef::from(field)),
        ),
        deletion_conflict: snapshot.life == Life::Conflict,
    }
}
#[derive(Serialize)]
#[serde(tag = "type", content = "value", rename_all = "lowercase")]
enum ValueRef<'a> {
    Body(&'a str),
    Attribute(AttributeRef<'a>),
    Absent,
    Kind(bool),
}
impl<'a> From<&'a Value> for ValueRef<'a> {
    fn from(value: &'a Value) -> Self {
        match value {
            Value::Body(s) => Self::Body(s),
            Value::Attribute(v) => Self::Attribute(AttributeRef(v)),
            Value::Absent => Self::Absent,
            Value::Kind(b) => Self::Kind(*b),
        }
    }
}
pub(crate) fn value_json(value: &Value) -> Json {
    serde_json::to_value(ValueRef::from(value)).expect("value is serializable")
}
#[derive(Serialize)]
struct CandidateRef<'a, V> {
    value: ValueRef<'a>,
    versions: V,
}
fn candidate_ref(candidate: &Candidate) -> impl Serialize + '_ {
    CandidateRef {
        value: ValueRef::from(&candidate.value),
        versions: Sequence(candidate.versions.iter().map(|version| hex(&version.0))),
    }
}
#[derive(Serialize)]
struct FieldCandidatesRef<'a, C> {
    field: FieldRef<'a>,
    candidates: C,
}
fn field_candidates_ref<'a>(field: &'a Field, candidates: &'a [Candidate]) -> impl Serialize + 'a {
    FieldCandidatesRef {
        field: FieldRef::from(field),
        candidates: Sequence(candidates.iter().map(candidate_ref)),
    }
}
#[derive(Serialize)]
struct ConflictViewRef<V, C> {
    view: V,
    candidates: C,
    deleted: bool,
}
fn conflict_view_ref(view: &ConflictView) -> impl Serialize + '_ {
    ConflictViewRef {
        view: read_view_ref(&view.view),
        candidates: Sequence(
            view.candidates
                .iter()
                .map(|(field, groups)| field_candidates_ref(field, groups)),
        ),
        deleted: view.deleted,
    }
}
fn snapshot_conflict_ref(snapshot: &ContentSnapshot, include_kinds: bool) -> impl Serialize + '_ {
    ConflictViewRef {
        view: snapshot_read_ref(snapshot, include_kinds),
        candidates: Sequence(
            snapshot
                .fields
                .iter()
                .filter(move |(field, _)| visible_field(field, include_kinds))
                .map(|(field, state)| field_candidates_ref(field, &state.candidates)),
        ),
        deleted: snapshot.life == Life::Deleted,
    }
}

/// Stop at the first write over budget, without building JSON or a response body.
fn encoded_size(value: &impl Serialize, limit: usize) -> Option<usize> {
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
    serde_json::to_writer(&mut budget, value).ok()?;
    Some(limit - budget.0)
}
pub fn read_view_json(view: &ReadView) -> Json {
    serde_json::to_value(read_view_ref(view)).expect("read view is serializable")
}
/// Counts the complete encoded read view without constructing JSON or a body.
pub fn read_view_size(view: &ReadView, limit: usize) -> Option<usize> {
    encoded_size(&read_view_ref(view), limit)
}
pub fn conflict_view_json(view: &ConflictView) -> Json {
    serde_json::to_value(conflict_view_ref(view)).expect("conflict view is serializable")
}
/// Counts the complete conflict envelope, including its normal read view.
pub fn conflict_view_size(view: &ConflictView, limit: usize) -> Option<usize> {
    encoded_size(&conflict_view_ref(view), limit)
}
/// Preflight before cloning output or minting a fixed-length token.
/// `include_kinds` controls kind conflict visibility, not ordinary Content kinds.
pub fn snapshot_read_view_size(
    snapshot: &ContentSnapshot,
    include_kinds: bool,
    limit: usize,
) -> Option<usize> {
    encoded_size(&snapshot_read_ref(snapshot, include_kinds), limit)
}
/// As above, including all visible candidate groups and the complete envelope.
pub fn snapshot_conflict_view_size(
    snapshot: &ContentSnapshot,
    include_kinds: bool,
    limit: usize,
) -> Option<usize> {
    encoded_size(&snapshot_conflict_ref(snapshot, include_kinds), limit)
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

#[cfg(test)]
mod tests {
    use super::{Sequence, encoded_size};
    use std::cell::Cell;

    #[test]
    fn budget_overflow_stops_before_visiting_remaining_elements() {
        let visited = Cell::new(0);
        let large = "\u{1}".repeat(4096);
        let sequence = Sequence((0..100).map(|_| {
            visited.set(visited.get() + 1);
            &large
        }));
        assert_eq!(encoded_size(&sequence, 10), None);
        assert_eq!(visited.get(), 1);
        visited.set(0);
        assert_eq!(encoded_size(&sequence, 0), None);
        assert_eq!(visited.get(), 0);
    }
}
