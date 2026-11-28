//! Pure Mote capture model and request builders.
//!
//! This crate knows the `capture` Kind, its attributes, and how to project an
//! App API read view. It does not open a node, hold credentials, or perform I/O.

use harmion_protocol::{
    domain::{AttributeKey, AttributeValue, Field, Number, NumberError, Value},
    query::{AttributePredicate, Condition},
};
use serde::Serialize;
use serde_json::Value as Json;
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, HashMap},
    str::FromStr,
};

pub const CAPTURE_KIND: &str = "capture";
pub const CAPTURE_NAMESPACE: &str = "capture";
pub const CREATED_AT: &str = "created_at_micros";
pub const ARCHIVED: &str = "archived";
pub const ARCHIVED_AT: &str = "archived_at_micros";
pub const PINNED: &str = "pinned";
pub const SESSION_OPEN: &str = "session_open";
pub const REPLY_TO: &str = "reply_to";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid capture view: {0}")]
    InvalidView(&'static str),
    #[error("content is not a capture")]
    NotCapture,
    #[error("invalid capture attribute `{0}`")]
    InvalidAttribute(&'static str),
    #[error("invalid capture timestamp")]
    InvalidTimestamp,
    #[error(transparent)]
    Number(#[from] NumberError),
}

/// Read projection of one capture Content. The edit token is deliberately not
/// serialized or included in Debug output: callers that need a mutation must
/// keep it in memory only.
#[derive(Clone, Serialize)]
pub struct Capture {
    pub id: String,
    pub body: String,
    pub created_at_micros: Option<i64>,
    pub archived: bool,
    pub archived_at_micros: Option<i64>,
    pub pinned: bool,
    pub session_open: bool,
    pub long: bool,
    pub reply_to: Option<String>,
    pub kinds: Vec<String>,
    pub conflicted: bool,
    pub deletion_conflict: bool,
    #[serde(skip)]
    edit_token: Option<String>,
}

impl std::fmt::Debug for Capture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Capture")
            .field("id", &self.id)
            .field("body_len", &self.body.len())
            .field("created_at_micros", &self.created_at_micros)
            .field("archived", &self.archived)
            .field("archived_at_micros", &self.archived_at_micros)
            .field("pinned", &self.pinned)
            .field("session_open", &self.session_open)
            .field("long", &self.long)
            .field("reply_to", &self.reply_to)
            .field("kinds", &self.kinds)
            .field("conflicted", &self.conflicted)
            .field("deletion_conflict", &self.deletion_conflict)
            .field("edit_token", &"[redacted]")
            .finish()
    }
}

impl Capture {
    /// Parse an App API `get`/`query` item. Multi-Kind content is accepted as
    /// long as it contains `capture`; only the capture attributes are used.
    pub fn from_view(view: &Json) -> Result<Self, Error> {
        let content = view
            .get("content")
            .and_then(Json::as_object)
            .ok_or(Error::InvalidView("content"))?;
        let id = content
            .get("id")
            .and_then(Json::as_str)
            .filter(|id| is_hex_64(id))
            .ok_or(Error::InvalidView("content.id"))?
            .to_owned();
        let body = content
            .get("body")
            .and_then(Json::as_str)
            .ok_or(Error::InvalidView("content.body"))?
            .to_owned();
        let kind_values = content
            .get("kinds")
            .and_then(Json::as_array)
            .ok_or(Error::InvalidView("content.kinds"))?;
        let mut kinds = BTreeSet::new();
        for kind in kind_values {
            kinds.insert(
                kind.as_str()
                    .ok_or(Error::InvalidView("content.kinds"))?
                    .to_owned(),
            );
        }
        if !kinds.contains(CAPTURE_KIND) {
            return Err(Error::NotCapture);
        }
        let attributes = capture_attributes(content.get("attributes"))?;
        let created_at_micros = attribute_timestamp(&attributes, CREATED_AT)?;
        let archived_at_micros = attribute_timestamp(&attributes, ARCHIVED_AT)?;
        let reply_to = attribute_string(&attributes, REPLY_TO)?;
        let edit_token = view
            .get("edit_token")
            .and_then(Json::as_str)
            .filter(|token| !token.is_empty())
            .map(str::to_owned);
        let conflicted = !view
            .get("conflicts")
            .and_then(Json::as_array)
            .ok_or(Error::InvalidView("conflicts"))?
            .is_empty();
        let deletion_conflict = view
            .get("deletion_conflict")
            .and_then(Json::as_bool)
            .ok_or(Error::InvalidView("deletion_conflict"))?;
        Ok(Self {
            id,
            body,
            created_at_micros,
            archived: attribute_bool(&attributes, ARCHIVED)?,
            archived_at_micros,
            pinned: attribute_bool(&attributes, PINNED)?,
            session_open: attribute_bool(&attributes, SESSION_OPEN)?,
            long: attributes.contains_key(SESSION_OPEN),
            reply_to,
            kinds: kinds.into_iter().collect(),
            conflicted,
            deletion_conflict,
            edit_token,
        })
    }

    pub fn token(&self) -> Option<&str> {
        self.edit_token.as_deref()
    }

    pub fn short_id(&self) -> &str {
        self.id.get(..8).unwrap_or(&self.id)
    }

    pub fn matches(&self, query: &str) -> bool {
        if query.is_empty() {
            return true;
        }
        self.body.to_lowercase().contains(&query.to_lowercase())
    }

    pub fn created_at(&self) -> i64 {
        self.created_at_micros.unwrap_or(i64::MIN)
    }

    pub fn archived_at(&self) -> i64 {
        self.archived_at_micros.unwrap_or(i64::MIN)
    }

    pub fn preview(&self, max_chars: usize) -> String {
        let first = self
            .body
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("");
        let mut preview: String = first
            .trim()
            .chars()
            .take(max_chars)
            .map(|character| {
                if character.is_control() {
                    ' '
                } else {
                    character
                }
            })
            .collect();
        if first.trim().chars().count() > max_chars {
            preview.push('…');
        }
        preview
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArchiveFilter {
    Inbox,
    Archived,
    All,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Filter {
    pub archive: ArchiveFilter,
    pub pinned_only: bool,
}

impl Filter {
    pub fn matches(&self, capture: &Capture) -> bool {
        let archive_matches = match self.archive {
            ArchiveFilter::Inbox => !capture.archived,
            ArchiveFilter::Archived => capture.archived,
            ArchiveFilter::All => true,
        };
        archive_matches && (!self.pinned_only || capture.pinned)
    }

    pub fn apply<'a>(&self, captures: impl IntoIterator<Item = &'a Capture>) -> Vec<&'a Capture> {
        captures
            .into_iter()
            .filter(|capture| self.matches(capture))
            .collect()
    }
}

/// Push the archive and pin filters into the node query so an inbox listing
/// does not need to fetch every archived Content.
pub fn query_condition(filter: &Filter) -> Condition {
    let mut terms = vec![Condition::Kind(CAPTURE_KIND.into())];
    let archived = Condition::Attribute {
        key: AttributeKey {
            namespace: CAPTURE_NAMESPACE.into(),
            key: ARCHIVED.into(),
        },
        predicate: AttributePredicate::Eq(AttributeValue::Bool(true)),
    };
    match filter.archive {
        ArchiveFilter::Inbox => terms.push(Condition::Not(Box::new(archived))),
        ArchiveFilter::Archived => terms.push(archived),
        ArchiveFilter::All => {}
    }
    if filter.pinned_only {
        terms.push(Condition::Attribute {
            key: AttributeKey {
                namespace: CAPTURE_NAMESPACE.into(),
                key: PINNED.into(),
            },
            predicate: AttributePredicate::Eq(AttributeValue::Bool(true)),
        });
    }
    Condition::And(terms)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sort {
    Inbox,
    Archive,
    Created,
}

pub fn sort_captures(captures: &mut [Capture], order: Sort) {
    match order {
        Sort::Inbox => captures.sort_by(inbox_cmp),
        Sort::Archive => captures.sort_by(archive_cmp),
        Sort::Created => captures.sort_by(created_cmp),
    }
}

pub fn inbox_cmp(left: &Capture, right: &Capture) -> Ordering {
    left.created_at()
        .cmp(&right.created_at())
        .then_with(|| left.id.cmp(&right.id))
}

pub fn archive_cmp(left: &Capture, right: &Capture) -> Ordering {
    left.archived_at()
        .cmp(&right.archived_at())
        .then_with(|| left.created_at().cmp(&right.created_at()))
        .then_with(|| left.id.cmp(&right.id))
}

fn created_cmp(left: &Capture, right: &Capture) -> Ordering {
    left.created_at()
        .cmp(&right.created_at())
        .then_with(|| left.id.cmp(&right.id))
}

/// Fields for a short capture create request.
pub fn create_fields(
    body: &str,
    created_at_micros: i64,
    reply_to: Option<&str>,
) -> Result<BTreeMap<Field, Value>, Error> {
    let mut fields = BTreeMap::new();
    fields.insert(Field::Body, Value::Body(body.to_owned()));
    fields.insert(Field::Kind(CAPTURE_KIND.to_owned()), Value::Kind(true));
    fields.insert(
        attribute(CREATED_AT),
        Value::Attribute(AttributeValue::Number(Number::from_str(
            &created_at_micros.to_string(),
        )?)),
    );
    if let Some(reply_to) = reply_to {
        fields.insert(
            attribute(REPLY_TO),
            Value::Attribute(AttributeValue::String(reply_to.to_owned())),
        );
    }
    Ok(fields)
}

/// Fields for an archive/unarchive edit. Restoring removes the archive time.
pub fn archive_fields(
    archived: bool,
    archived_at_micros: Option<i64>,
) -> Result<BTreeMap<Field, Value>, Error> {
    let mut fields = BTreeMap::new();
    fields.insert(
        attribute(ARCHIVED),
        Value::Attribute(AttributeValue::Bool(archived)),
    );
    fields.insert(
        attribute(ARCHIVED_AT),
        match (archived, archived_at_micros) {
            (true, Some(micros)) => Value::Attribute(AttributeValue::Number(Number::from_str(
                &micros.to_string(),
            )?)),
            (true, None) => Value::Absent,
            (false, _) => Value::Absent,
        },
    );
    Ok(fields)
}

pub fn pin_fields(pinned: bool) -> BTreeMap<Field, Value> {
    let mut fields = BTreeMap::new();
    fields.insert(
        attribute(PINNED),
        Value::Attribute(AttributeValue::Bool(pinned)),
    );
    fields
}

fn attribute(key: &'static str) -> Field {
    Field::Attribute {
        namespace: CAPTURE_NAMESPACE.to_owned(),
        key: key.to_owned(),
    }
}

fn capture_attributes(value: Option<&Json>) -> Result<HashMap<&str, &Json>, Error> {
    let entries = value
        .and_then(Json::as_array)
        .ok_or(Error::InvalidView("content.attributes"))?;
    let mut attributes = HashMap::new();
    for entry in entries {
        let namespace = entry
            .get("namespace")
            .and_then(Json::as_str)
            .ok_or(Error::InvalidView("attribute.namespace"))?;
        if namespace != CAPTURE_NAMESPACE {
            continue;
        }
        let key = entry
            .get("key")
            .and_then(Json::as_str)
            .ok_or(Error::InvalidView("attribute.key"))?;
        let value = entry.get("value").unwrap_or(&Json::Null);
        attributes.insert(key, value);
    }
    Ok(attributes)
}

fn attribute_bool(attributes: &HashMap<&str, &Json>, key: &'static str) -> Result<bool, Error> {
    match attributes.get(key) {
        None | Some(Json::Null) => Ok(false),
        Some(Json::Bool(value)) => Ok(*value),
        Some(_) => Err(Error::InvalidAttribute(key)),
    }
}

fn attribute_timestamp(
    attributes: &HashMap<&str, &Json>,
    key: &'static str,
) -> Result<Option<i64>, Error> {
    match attributes.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::Number(number)) => number
            .as_i64()
            .or_else(|| number.as_u64().and_then(|value| i64::try_from(value).ok()))
            .or_else(|| {
                number.as_f64().and_then(|value| {
                    (value.is_finite()
                        && value.fract() == 0.0
                        && value >= i64::MIN as f64
                        && value < i64::MAX as f64)
                        .then_some(value as i64)
                })
            })
            .map(Some)
            .ok_or(Error::InvalidTimestamp),
        Some(Json::String(value)) => value
            .parse::<i64>()
            .map(Some)
            .map_err(|_| Error::InvalidTimestamp),
        Some(_) => Err(Error::InvalidTimestamp),
    }
}

fn attribute_string(
    attributes: &HashMap<&str, &Json>,
    key: &'static str,
) -> Result<Option<String>, Error> {
    match attributes.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::String(value)) if !value.is_empty() => Ok(Some(value.to_owned())),
        Some(Json::String(_)) => Ok(None),
        Some(_) => Err(Error::InvalidAttribute(key)),
    }
}

fn is_hex_64(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn id(byte: &str) -> String {
        byte.repeat(32)
    }

    fn view(body: &str, attributes: Json) -> Json {
        json!({
            "content": {
                "id": id("11"),
                "body": body,
                "kinds": ["capture"],
                "attributes": attributes,
            },
            "edit_token": "22".repeat(32),
            "conflicts": [],
            "deletion_conflict": false
        })
    }

    #[test]
    fn parses_capture_attributes_and_hides_token() {
        let capture = Capture::from_view(&view(
            "hello",
            json!([
                {"namespace":"capture","key":"created_at_micros","value":1_700_000_000_000_000i64},
                {"namespace":"capture","key":"archived","value":true},
                {"namespace":"capture","key":"archived_at_micros","value":1_700_000_000_100_000i64},
                {"namespace":"capture","key":"pinned","value":true},
                {"namespace":"capture","key":"session_open","value":false},
                {"namespace":"capture","key":"reply_to","value":"33".repeat(32)},
                {"namespace":"other","key":"ignored","value":"x"}
            ]),
        ))
        .unwrap();
        assert_eq!(capture.id, id("11"));
        assert_eq!(capture.body, "hello");
        assert_eq!(capture.created_at_micros, Some(1_700_000_000_000_000));
        assert!(capture.archived);
        assert_eq!(capture.archived_at_micros, Some(1_700_000_000_100_000));
        assert!(capture.pinned);
        assert!(!capture.session_open);
        assert_eq!(capture.reply_to, Some("33".repeat(32)));
        assert!(capture.token().is_some());
        let json = serde_json::to_value(&capture).unwrap();
        assert!(json.get("edit_token").is_none());
        assert!(json.get("body").is_some());
    }

    #[test]
    fn debug_redacts_token_and_json_has_nullable_keys() {
        let capture = Capture::from_view(&view("hello", json!([]))).unwrap();
        let debug = format!("{capture:?}");
        assert!(debug.contains("[redacted]"));
        assert!(!debug.contains(&"22".repeat(32)));
        let json = serde_json::to_value(&capture).unwrap();
        assert!(json.get("created_at_micros").unwrap().is_null());
        assert!(json.get("archived_at_micros").unwrap().is_null());
        assert!(json.get("reply_to").unwrap().is_null());
    }

    #[test]
    fn preview_replaces_control_characters() {
        let capture = Capture::from_view(&view("a\u{1b}[31m b\nc", json!([]))).unwrap();
        assert_eq!(capture.preview(72), "a [31m b");
    }

    #[test]
    fn conflict_fields_are_required() {
        let mut value = view("hello", json!([]));
        value.as_object_mut().unwrap().remove("conflicts");
        assert!(matches!(
            Capture::from_view(&value),
            Err(Error::InvalidView("conflicts"))
        ));
        let mut value = view("hello", json!([]));
        value.as_object_mut().unwrap().remove("deletion_conflict");
        assert!(matches!(
            Capture::from_view(&value),
            Err(Error::InvalidView("deletion_conflict"))
        ));
    }

    #[test]
    fn query_condition_pushes_inbox_and_pin_filters() {
        let condition = query_condition(&Filter {
            archive: ArchiveFilter::Inbox,
            pinned_only: true,
        });
        let Condition::And(terms) = condition else {
            panic!("expected an AND condition");
        };
        assert_eq!(terms.len(), 3);
        assert!(matches!(terms[0], Condition::Kind(_)));
        assert!(matches!(terms[1], Condition::Not(_)));
        assert!(matches!(terms[2], Condition::Attribute { .. }));
    }

    #[test]
    fn detects_finished_long_captures_by_attribute_presence() {
        let capture = Capture::from_view(&view(
            "finished long capture",
            json!([
                {"namespace":"capture","key":"session_open","value":false}
            ]),
        ))
        .unwrap();
        assert!(capture.long);
        assert!(!capture.session_open);
    }

    #[test]
    fn rejects_non_capture_content() {
        let mut value = view("hello", json!([]));
        value["content"]["kinds"] = json!(["wiki"]);
        assert!(matches!(Capture::from_view(&value), Err(Error::NotCapture)));
    }

    #[test]
    fn accepts_multi_kind_capture() {
        let mut value = view("hello", json!([]));
        value["content"]["kinds"] = json!(["capture", "wiki"]);
        assert!(Capture::from_view(&value).is_ok());
    }

    #[test]
    fn accepts_scientific_integer_timestamp() {
        let capture = Capture::from_view(&view(
            "hello",
            json!([
                {"namespace":"capture","key":"created_at_micros","value":1.7e15}
            ]),
        ))
        .unwrap();
        assert_eq!(capture.created_at_micros, Some(1_700_000_000_000_000));
    }

    #[test]
    fn archive_order_keeps_undated_archives_first() {
        let mut captures = vec![
            Capture {
                id: id("03"),
                body: String::new(),
                created_at_micros: Some(3),
                archived: true,
                archived_at_micros: Some(3),
                pinned: false,
                session_open: false,
                long: false,
                reply_to: None,
                kinds: vec![CAPTURE_KIND.into()],
                conflicted: false,
                deletion_conflict: false,
                edit_token: None,
            },
            Capture {
                id: id("01"),
                body: String::new(),
                created_at_micros: Some(1),
                archived: true,
                archived_at_micros: None,
                pinned: false,
                session_open: false,
                long: false,
                reply_to: None,
                kinds: vec![CAPTURE_KIND.into()],
                conflicted: false,
                deletion_conflict: false,
                edit_token: None,
            },
            Capture {
                id: id("02"),
                body: String::new(),
                created_at_micros: Some(2),
                archived: true,
                archived_at_micros: Some(2),
                pinned: false,
                session_open: false,
                long: false,
                reply_to: None,
                kinds: vec![CAPTURE_KIND.into()],
                conflicted: false,
                deletion_conflict: false,
                edit_token: None,
            },
        ];
        sort_captures(&mut captures, Sort::Archive);
        assert_eq!(
            captures
                .iter()
                .map(|capture| capture.id.as_str())
                .collect::<Vec<_>>(),
            vec![id("01"), id("02"), id("03")]
        );
    }

    #[test]
    fn filter_and_search() {
        let mut capture = Capture::from_view(&view("Hello world", json!([]))).unwrap();
        capture.created_at_micros = Some(1);
        let filter = Filter {
            archive: ArchiveFilter::Inbox,
            pinned_only: false,
        };
        assert!(filter.matches(&capture));
        assert!(capture.matches("hello"));
        assert!(!capture.matches("missing"));
    }

    #[test]
    fn builds_create_and_archive_fields() {
        let fields = create_fields("hello", 1_700_000_000_000_000, Some(&id("44"))).unwrap();
        assert!(matches!(fields.get(&Field::Body), Some(Value::Body(body)) if body == "hello"));
        assert!(matches!(
            fields.get(&Field::Kind(CAPTURE_KIND.into())),
            Some(Value::Kind(true))
        ));
        let archived = archive_fields(true, Some(1_700_000_000_000_001)).unwrap();
        assert!(matches!(
            archived.get(&attribute(ARCHIVED)),
            Some(Value::Attribute(AttributeValue::Bool(true)))
        ));
        assert!(matches!(
            archived.get(&attribute(ARCHIVED_AT)),
            Some(Value::Attribute(AttributeValue::Number(_)))
        ));
        let restored = archive_fields(false, None).unwrap();
        assert!(matches!(
            restored.get(&attribute(ARCHIVED_AT)),
            Some(Value::Absent)
        ));
    }
}
