//! JSON specifications for the binary App API bodies.
//!
//! Kotlin describes a save or a query as JSON; this module converts it into the
//! exact protocol types so the canonical encoders stay the single source of
//! truth. Unknown fields are rejected rather than ignored.
use harmion_domain::Change;
use harmion_domain::{AttributeValue, ContentId, Field, Number, UpdateId, Value};
use harmion_protocol::FieldSelection;
use harmion_protocol::query::{AttributePredicate, Condition, OrderOperator, OrderedValue};
use harmion_protocol::unhex;
use harmion_protocol::{Command, EditToken, RequestId, SaveRequest};
use serde::Deserialize;
use serde_json::Value as Json;

fn attribute(value: &Json) -> Result<AttributeValue, String> {
    Ok(match value {
        Json::Null => AttributeValue::Null,
        Json::Bool(flag) => AttributeValue::Bool(*flag),
        Json::Number(number) => AttributeValue::Number(
            number
                .to_string()
                .parse::<Number>()
                .map_err(|_| "invalid decimal number".to_owned())?,
        ),
        Json::String(text) => AttributeValue::String(text.clone()),
        Json::Array(items) => AttributeValue::Array(
            items
                .iter()
                .map(attribute)
                .collect::<Result<Vec<_>, String>>()?,
        ),
        Json::Object(entries) => AttributeValue::Object(
            entries
                .iter()
                .map(|(key, value)| Ok((key.clone(), attribute(value)?)))
                .collect::<Result<std::collections::BTreeMap<String, AttributeValue>, String>>()?,
        ),
    })
}

fn string_field(object: &serde_json::Map<String, Json>, key: &str) -> Result<String, String> {
    object
        .get(key)
        .and_then(Json::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("missing string field `{key}`"))
}

fn parse_field(spec: &Json) -> Result<(Field, Value), String> {
    let object = spec
        .as_object()
        .ok_or_else(|| "field must be an object".to_owned())?;
    match object.get("field").and_then(Json::as_str) {
        Some("body") => Ok((Field::Body, Value::Body(string_field(object, "text")?))),
        Some("attribute") => {
            let namespace = string_field(object, "namespace")?;
            let key = string_field(object, "key")?;
            let absent = object
                .get("absent")
                .and_then(Json::as_bool)
                .unwrap_or(false);
            let value = if absent {
                Value::Absent
            } else {
                let value = object
                    .get("value")
                    .ok_or_else(|| "attribute requires `value` or `absent`".to_owned())?;
                Value::Attribute(attribute(value)?)
            };
            Ok((Field::Attribute { namespace, key }, value))
        }
        Some("kind") => {
            let member = object
                .get("member")
                .and_then(Json::as_bool)
                .ok_or_else(|| "kind requires `member`".to_owned())?;
            Ok((
                Field::Kind(string_field(object, "name")?),
                Value::Kind(member),
            ))
        }
        _ => Err("unknown field spec".to_owned()),
    }
}

fn parse_field_key(spec: &Json) -> Result<Field, String> {
    let object = spec
        .as_object()
        .ok_or_else(|| "field must be an object".to_owned())?;
    match object.get("field").and_then(Json::as_str) {
        Some("body") => Ok(Field::Body),
        Some("attribute") => Ok(Field::Attribute {
            namespace: string_field(object, "namespace")?,
            key: string_field(object, "key")?,
        }),
        Some("kind") => Ok(Field::Kind(string_field(object, "name")?)),
        _ => Err("unknown field spec".to_owned()),
    }
}

fn parse_id(object: &serde_json::Map<String, Json>, key: &str) -> Result<UpdateId, String> {
    unhex::<32>(&string_field(object, key)?)
        .map(UpdateId)
        .map_err(|_| format!("`{key}` must be 32 hex bytes"))
}

fn parse_fields(fields: &Json) -> Result<std::collections::BTreeMap<Field, Value>, String> {
    let array = fields
        .as_array()
        .ok_or_else(|| "fields must be an array".to_owned())?;
    let mut parsed = std::collections::BTreeMap::new();
    for spec in array {
        let (field, value) = parse_field(spec)?;
        if parsed.insert(field, value).is_some() {
            return Err("duplicate field in save request".to_owned());
        }
    }
    Ok(parsed)
}

fn parse_token(object: &serde_json::Map<String, Json>) -> Result<EditToken, String> {
    let text = string_field(object, "token")?;
    let bytes = unhex::<32>(&text).map_err(|_| "token must be 32 hex bytes".to_owned())?;
    Ok(EditToken(bytes))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveSpec {
    request_id: String,
    content_id: String,
    command: Json,
}

pub fn save_request(json: &str) -> Result<SaveRequest, String> {
    let spec: SaveSpec = serde_json::from_str(json).map_err(|error| error.to_string())?;
    let request_id = unhex::<32>(&spec.request_id)
        .map(RequestId)
        .map_err(|_| "request_id must be 32 hex bytes".to_owned())?;
    let content_id = unhex::<32>(&spec.content_id)
        .map(ContentId)
        .map_err(|_| "content_id must be 32 hex bytes".to_owned())?;
    let command = spec
        .command
        .as_object()
        .ok_or_else(|| "command must be an object".to_owned())?;
    let kind = string_field(command, "type")?;
    let fields = || -> Result<std::collections::BTreeMap<Field, Value>, String> {
        parse_fields(
            command
                .get("fields")
                .ok_or_else(|| "command requires `fields`".to_owned())?,
        )
    };
    let command = match kind.as_str() {
        "create" => Command::Create { fields: fields()? },
        "edit" => Command::Edit {
            token: parse_token(command)?,
            fields: fields()?,
        },
        "delete" => Command::Delete {
            token: parse_token(command)?,
        },
        "keep" => Command::Keep {
            token: parse_token(command)?,
        },
        "restore" => Command::Restore {
            token: parse_token(command)?,
            fields: fields()?,
        },
        "select" => {
            let token = parse_token(command)?;
            let specs = command
                .get("selections")
                .and_then(Json::as_array)
                .ok_or_else(|| "select requires `selections`".to_owned())?;
            let mut selections = std::collections::BTreeMap::new();
            for spec in specs {
                let object = spec
                    .as_object()
                    .ok_or_else(|| "selection must be an object".to_owned())?;
                let field = parse_field_key(spec)?;
                let version = parse_id(object, "version")?;
                let replaces = object
                    .get("replaces")
                    .and_then(Json::as_array)
                    .ok_or_else(|| "selection requires `replaces`".to_owned())?;
                let replaces = replaces
                    .iter()
                    .map(|value| {
                        value
                            .as_str()
                            .ok_or_else(|| "replaces must be strings".to_owned())
                            .and_then(|text| {
                                unhex::<32>(text)
                                    .map(UpdateId)
                                    .map_err(|_| "replaces must be 32 hex bytes".to_owned())
                            })
                    })
                    .collect::<Result<_, _>>()?;
                if selections
                    .insert(field, FieldSelection { version, replaces })
                    .is_some()
                {
                    return Err("duplicate field in selection".to_owned());
                }
            }
            Command::Select { token, selections }
        }
        "resolve" => {
            let token = parse_token(command)?;
            let specs = command
                .get("changes")
                .and_then(Json::as_array)
                .ok_or_else(|| "resolve requires `changes`".to_owned())?;
            let mut changes = std::collections::BTreeMap::new();
            for spec in specs {
                let object = spec
                    .as_object()
                    .ok_or_else(|| "change must be an object".to_owned())?;
                let (field, value) = parse_field(spec)?;
                let replaces = object
                    .get("replaces")
                    .and_then(Json::as_array)
                    .ok_or_else(|| "change requires `replaces`".to_owned())?;
                let replaces = replaces
                    .iter()
                    .map(|value| {
                        value
                            .as_str()
                            .ok_or_else(|| "replaces must be strings".to_owned())
                            .and_then(|text| {
                                unhex::<32>(text)
                                    .map(UpdateId)
                                    .map_err(|_| "replaces must be 32 hex bytes".to_owned())
                            })
                    })
                    .collect::<Result<_, _>>()?;
                if changes.insert(field, Change { value, replaces }).is_some() {
                    return Err("duplicate field in resolve".to_owned());
                }
            }
            Command::Resolve { token, changes }
        }
        _ => return Err("unsupported command type".to_owned()),
    };
    Ok(SaveRequest {
        request_id,
        content_id,
        command,
    })
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum ConditionSpec {
    And {
        conditions: Vec<ConditionSpec>,
    },
    Or {
        conditions: Vec<ConditionSpec>,
    },
    Not {
        condition: Box<ConditionSpec>,
    },
    Kind {
        kind: String,
    },
    LastEditedAt {
        operator: OperatorSpec,
        utc_micros: i64,
    },
    Attribute {
        namespace: String,
        key: String,
        predicate: PredicateSpec,
    },
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum OperatorSpec {
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
}

impl From<OperatorSpec> for OrderOperator {
    fn from(value: OperatorSpec) -> Self {
        match value {
            OperatorSpec::Less => Self::Less,
            OperatorSpec::LessOrEqual => Self::LessOrEqual,
            OperatorSpec::Greater => Self::Greater,
            OperatorSpec::GreaterOrEqual => Self::GreaterOrEqual,
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum PredicateSpec {
    Exists,
    Eq { value: Json },
    Compare { operator: OperatorSpec, value: Json },
}

fn ordered(value: &Json) -> Result<OrderedValue, String> {
    match value {
        Json::String(text) => Ok(OrderedValue::String(text.clone())),
        Json::Number(number) => Ok(OrderedValue::Number(
            number
                .to_string()
                .parse::<Number>()
                .map_err(|_| "invalid decimal number".to_owned())?,
        )),
        _ => Err("compare value must be a string or a number".to_owned()),
    }
}

fn condition(spec: &ConditionSpec) -> Result<Condition, String> {
    Ok(match spec {
        ConditionSpec::And { conditions } => Condition::And(
            conditions
                .iter()
                .map(condition)
                .collect::<Result<Vec<_>, String>>()?,
        ),
        ConditionSpec::Or { conditions } => Condition::Or(
            conditions
                .iter()
                .map(condition)
                .collect::<Result<Vec<_>, String>>()?,
        ),
        ConditionSpec::Not { condition: inner } => Condition::Not(Box::new(condition(inner)?)),
        ConditionSpec::Kind { kind } => Condition::Kind(kind.clone()),
        ConditionSpec::LastEditedAt {
            operator,
            utc_micros,
        } => Condition::LastEditedAt {
            operator: (*operator).into(),
            utc_micros: *utc_micros,
        },
        ConditionSpec::Attribute {
            namespace,
            key,
            predicate,
        } => Condition::Attribute {
            key: harmion_domain::AttributeKey {
                namespace: namespace.clone(),
                key: key.clone(),
            },
            predicate: match predicate {
                PredicateSpec::Exists => AttributePredicate::Exists,
                PredicateSpec::Eq { value } => AttributePredicate::Eq(attribute(value)?),
                PredicateSpec::Compare { operator, value } => AttributePredicate::Compare {
                    operator: (*operator).into(),
                    value: ordered(value)?,
                },
            },
        },
    })
}

pub fn condition_from_str(json: &str) -> Result<Condition, String> {
    let spec: ConditionSpec = serde_json::from_str(json).map_err(|error| error.to_string())?;
    condition(&spec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use harmion_protocol::encode_save_request;
    use std::collections::BTreeSet;

    #[test]
    fn create_request_matches_the_canonical_encoder() {
        let request = save_request(
            r#"{
                "request_id": "0101010101010101010101010101010101010101010101010101010101010101",
                "content_id": "0202020202020202020202020202020202020202020202020202020202020202",
                "command": {
                    "type": "create",
                    "fields": [
                        {"field": "body", "text": "hello"},
                        {"field": "attribute", "namespace": "app", "key": "done", "value": false},
                        {"field": "attribute", "namespace": "app", "key": "gone", "absent": true},
                        {"field": "attribute", "namespace": "app", "key": "none", "value": null},
                        {"field": "kind", "name": "note", "member": true}
                    ]
                }
            }"#,
        )
        .unwrap();
        let bytes = encode_save_request(&request).unwrap();
        assert_eq!(&bytes[..4], b"HRQ\0");
        // Edit without a token is rejected before any network call.
        assert!(
            save_request(
                r#"{"request_id":"0101010101010101010101010101010101010101010101010101010101010101",
                    "content_id":"0202020202020202020202020202020202020202020202020202020202020202",
                    "command":{"type":"edit","fields":[]}}"#
            )
            .is_err()
        );
    }

    #[test]
    fn conditions_parse_and_reject_unknown_shapes() {
        let condition = condition_from_str(
            r#"{"type":"and","conditions":[
                {"type":"kind","kind":"note"},
                {"type":"attribute","namespace":"app","key":"done",
                 "predicate":{"type":"eq","value":true}},
                {"type":"last_edited_at","operator":"greater","utc_micros":10}
            ]}"#,
        )
        .unwrap();
        let Condition::And(terms) = condition else {
            panic!("expected and");
        };
        assert_eq!(terms.len(), 3);
        assert!(condition_from_str(r#"{"type":"kind","kind":"x","extra":1}"#).is_err());
        assert!(
            condition_from_str(
                r#"{"type":"attribute","namespace":"a","key":"b",
                    "predicate":{"type":"compare","operator":"greater","value":true}}"#
            )
            .is_err()
        );
    }

    #[test]
    fn duplicate_fields_are_rejected() {
        assert!(
            save_request(
                r#"{"request_id":"0101010101010101010101010101010101010101010101010101010101010101",
                    "content_id":"0202020202020202020202020202020202020202020202020202020202020202",
                    "command":{"type":"create","fields":[
                        {"field":"body","text":"a"},{"field":"body","text":"b"}]}}"#
            )
            .is_err()
        );
    }

    #[test]
    fn select_and_resolve_commands_parse() {
        let request_id = "01".repeat(32);
        let content_id = "02".repeat(32);
        let token = "03".repeat(32);
        let version = "04".repeat(32);
        let select = save_request(&format!(
            r#"{{"request_id":"{request_id}","content_id":"{content_id}",
                "command":{{"type":"select","token":"{token}","selections":[
                    {{"field":"body","version":"{version}","replaces":["{version}"]}},
                    {{"field":"kind","name":"note","version":"{version}","replaces":["{version}"]}}
                ]}}}}"#
        ))
        .unwrap();
        let Command::Select {
            token: parsed,
            selections,
        } = select.command
        else {
            panic!("expected select");
        };
        assert_eq!(parsed.0, [3u8; 32]);
        assert_eq!(selections.len(), 2);
        assert_eq!(selections[&Field::Body].version, UpdateId([4u8; 32]));
        assert_eq!(
            selections[&Field::Kind("note".into())].replaces,
            BTreeSet::from([UpdateId([4u8; 32])])
        );

        let resolve = save_request(&format!(
            r#"{{"request_id":"{request_id}","content_id":"{content_id}",
                "command":{{"type":"resolve","token":"{token}","changes":[
                    {{"field":"body","text":"merged","replaces":["{version}"]}},
                    {{"field":"attribute","namespace":"app","key":"n","value":3,"replaces":["{version}"]}}
                ]}}}}"#
        ))
        .unwrap();
        let Command::Resolve { changes, .. } = resolve.command else {
            panic!("expected resolve");
        };
        assert_eq!(changes[&Field::Body].value, Value::Body("merged".into()));
        assert_eq!(
            changes[&Field::Attribute {
                namespace: "app".into(),
                key: "n".into()
            }]
                .value,
            Value::Attribute(AttributeValue::Number("3".parse().unwrap()))
        );
        assert_eq!(changes.len(), 2);
    }
}
