//! SQLite projection and execution for collection queries.

use crate::{db, fixed};
use harmion_domain::{AttributeValue, ContentId, ContentSnapshot, FieldState, Graph, Life, Number};
use harmion_node::{Error, access::Filter};
use harmion_protocol::AppId;
use harmion_protocol::query::{
    AttributePredicate, Condition, OrderOperator, OrderedValue, attribute_key_bytes,
};
use rusqlite::{Connection, params, params_from_iter, types::Value};
use std::collections::BTreeSet;

pub(super) fn initialize(sql: &Connection) -> Result<(), Error> {
    sql.execute_batch(
        "CREATE TABLE search_contents (
            content BLOB PRIMARY KEY NOT NULL CHECK(length(content)=32),
            last_edited_at INTEGER NOT NULL,
            conflicted INTEGER NOT NULL DEFAULT 0);
        CREATE INDEX search_edited ON search_contents(last_edited_at,content);
        CREATE INDEX search_conflicted ON search_contents(content) WHERE conflicted=1;
        CREATE TABLE search_kinds (
            content BLOB NOT NULL REFERENCES search_contents(content) ON DELETE CASCADE,
            kind TEXT NOT NULL, PRIMARY KEY(content,kind));
        CREATE INDEX search_kind ON search_kinds(kind,content);
        CREATE TABLE search_attributes (
            content BLOB NOT NULL REFERENCES search_contents(content) ON DELETE CASCADE,
            namespace TEXT NOT NULL, name TEXT NOT NULL, value BLOB NOT NULL,
            number_key BLOB, text_key BLOB, is_array INTEGER NOT NULL,
            PRIMARY KEY(content,namespace,name));
        CREATE INDEX search_attribute_value ON search_attributes(namespace,name,value,content);
        CREATE INDEX search_attribute_number ON search_attributes(namespace,name,number_key,content)
            WHERE number_key IS NOT NULL;
        CREATE INDEX search_attribute_text ON search_attributes(namespace,name,text_key,content)
            WHERE text_key IS NOT NULL;
        CREATE TABLE search_elements (
            content BLOB NOT NULL REFERENCES search_contents(content) ON DELETE CASCADE,
            namespace TEXT NOT NULL, name TEXT NOT NULL, value BLOB NOT NULL,
            PRIMARY KEY(content,namespace,name,value));
        CREATE INDEX search_element ON search_elements(namespace,name,value,content);",
    )
    .map_err(db)
}

pub(super) fn initialize_version_refs(sql: &Connection, graph: &Graph) -> Result<(), Error> {
    sql.execute_batch(
        "CREATE TABLE content_records (
            content BLOB NOT NULL CHECK(length(content)=32),
            seq INTEGER NOT NULL UNIQUE REFERENCES accept_log(seq),
            PRIMARY KEY(content,seq));
        CREATE TABLE search_basis (
            content BLOB NOT NULL REFERENCES search_contents(content) ON DELETE CASCADE,
            update_id BLOB NOT NULL REFERENCES accept_log(update_id),
            PRIMARY KEY(content,update_id));",
    )
    .map_err(db)?;
    let mut insert = sql
        .prepare("INSERT INTO content_records SELECT ?1,seq FROM accept_log WHERE update_id=?2")
        .map_err(db)?;
    for update in graph.updates() {
        if insert
            .execute(params![update.content_id.0, update.update_id.0])
            .map_err(db)?
            != 1
        {
            return Err(Error::CorruptLog);
        }
    }
    for id in graph.content_ids() {
        put_projection(sql, &graph.snapshot(id).ok_or(Error::CorruptLog)?)?;
    }
    Ok(())
}

pub(super) fn put_projection(sql: &Connection, snapshot: &ContentSnapshot) -> Result<(), Error> {
    let id = snapshot.content.id.0;
    sql.execute("DELETE FROM search_contents WHERE content=?1", [id])
        .map_err(db)?;
    let Some(content) = snapshot.visible_content() else {
        return Ok(());
    };
    sql.execute(
        "INSERT INTO search_contents VALUES (?1,?2,?3)",
        params![id, snapshot.last_edited_at, conflict_state(snapshot)],
    )
    .map_err(db)?;
    let mut basis = sql
        .prepare_cached("INSERT INTO search_basis VALUES (?1,?2)")
        .map_err(db)?;
    for update in &snapshot.basis {
        basis.execute(params![id, update.0]).map_err(db)?;
    }
    let mut kind = sql
        .prepare_cached("INSERT INTO search_kinds VALUES (?1,?2)")
        .map_err(db)?;
    for name in &content.kinds {
        kind.execute(params![id, name]).map_err(db)?;
    }
    let mut attribute = sql
        .prepare_cached("INSERT INTO search_attributes VALUES (?1,?2,?3,?4,?5,?6,?7)")
        .map_err(db)?;
    let mut element = sql
        .prepare_cached(
            "INSERT INTO search_elements VALUES (?1,?2,?3,?4)
         ON CONFLICT(content,namespace,name,value) DO NOTHING",
        )
        .map_err(db)?;
    for (key, value) in &content.attributes {
        let (number_key, text_key) = match value {
            AttributeValue::Number(number) => (Some(decimal_key(number)), None),
            AttributeValue::String(text) => (None, Some(text.as_bytes())),
            _ => (None, None),
        };
        attribute
            .execute(params![
                id,
                key.namespace,
                key.key,
                attribute_key_bytes(value)?,
                number_key,
                text_key,
                matches!(value, AttributeValue::Array(_)),
            ])
            .map_err(db)?;
        if let AttributeValue::Array(values) = value {
            for value in values {
                element
                    .execute(params![
                        id,
                        key.namespace,
                        key.key,
                        attribute_key_bytes(value)?
                    ])
                    .map_err(db)?;
            }
        }
    }
    Ok(())
}

/// Conflict state is internal, so a hidden kind conflict still counts.
fn conflict_state(snapshot: &ContentSnapshot) -> bool {
    snapshot.life == Life::Conflict || snapshot.fields.values().any(FieldState::is_conflicted)
}

/// Fill the flag for rows projected before the column existed. The projection is
/// derived from the graph, so replaying it is not a repair of authoritative data.
pub(super) fn backfill_conflicts(sql: &Connection, graph: &Graph) -> Result<(), Error> {
    let mut update = sql
        .prepare_cached("UPDATE search_contents SET conflicted=?1 WHERE content=?2")
        .map_err(db)?;
    for id in graph.content_ids() {
        let snapshot = graph.snapshot(id).ok_or(Error::CorruptLog)?;
        update
            .execute(params![conflict_state(&snapshot), id.0])
            .map_err(db)?;
    }
    Ok(())
}

// Lexicographically ordered BLOBs retain the full range and precision of Number.
fn decimal_key(number: &Number) -> Vec<u8> {
    let (negative, digits, exponent) = number.decimal_parts();
    if digits == "0" {
        return vec![1];
    }
    let scale = i128::from(exponent) + digits.len() as i128;
    let mut magnitude = ((scale as u128) ^ (1 << 127)).to_be_bytes().to_vec();
    magnitude.extend(digits.bytes());
    magnitude.push(0); // Explicit terminator reverses the order of prefixes for negatives.
    let mut key = vec![if negative { 0 } else { 2 }];
    if negative {
        key.extend(magnitude.into_iter().map(|byte| !byte));
    } else {
        key.extend(magnitude);
    }
    key
}

pub(super) fn search(
    sql: &Connection,
    condition: &Condition,
    read: &Filter,
    after: Option<ContentId>,
    limit: u32,
) -> Result<Vec<ContentId>, Error> {
    let (constraints, mut bindings) = search_constraints(condition, read, None)?;
    let resume = if let Some(after) = after {
        bindings.push(Value::Blob(after.0.to_vec()));
        "AND content > ?"
    } else {
        ""
    };
    bindings.push(Value::Integer(i64::from(limit)));
    let mut statement = sql
        .prepare(&format!(
            "SELECT content FROM search_contents WHERE {constraints}
         {resume} ORDER BY content LIMIT ?"
        ))
        .map_err(db)?;
    let mut rows = statement.query(params_from_iter(bindings)).map_err(db)?;
    let mut matches = Vec::new();
    while let Some(row) = rows.next().map_err(db)? {
        matches.push(ContentId(fixed(row.get(0).map_err(db)?)?));
    }
    Ok(matches)
}

fn placeholders(count: usize) -> String {
    vec!["?"; count].join(",")
}

pub(super) fn search_constraints(
    condition: &Condition,
    read: &Filter,
    app: Option<AppId>,
) -> Result<(String, Vec<Value>), Error> {
    let mut bindings = Vec::new();
    let condition = predicate(condition, &mut bindings, 0)?;
    let permission = match (read, app) {
        (Filter::All, _) => "1".into(),
        (Filter::Kinds(_), Some(app)) => {
            bindings.push(Value::Blob(app.0.to_vec()));
            "content IN (
                SELECT s.content FROM app_kinds AS k JOIN search_kinds AS s ON s.kind=k.kind
                WHERE k.app_id=? AND k.permission=0
            )"
            .into()
        }
        (Filter::Kinds(kinds), None) => kind_condition(kinds.iter(), &mut bindings),
    };
    Ok((format!("({condition}) AND ({permission})"), bindings))
}

fn kind_condition<'a>(
    kinds: impl ExactSizeIterator<Item = &'a String>,
    bindings: &mut Vec<Value>,
) -> String {
    if kinds.len() == 0 {
        return "0".into();
    }
    let placeholders = placeholders(kinds.len());
    bindings.extend(kinds.cloned().map(Value::Text));
    format!("content IN (SELECT content FROM search_kinds WHERE kind IN ({placeholders}))")
}

fn comparison(operator: OrderOperator) -> &'static str {
    match operator {
        OrderOperator::Less => "<",
        OrderOperator::LessOrEqual => "<=",
        OrderOperator::Greater => ">",
        OrderOperator::GreaterOrEqual => ">=",
    }
}

fn predicate(
    condition: &Condition,
    bindings: &mut Vec<Value>,
    depth: usize,
) -> Result<String, Error> {
    if depth > harmion_protocol::MAX_DEPTH {
        return Err(Error::InvalidRequest);
    }
    let expression = match condition {
        Condition::And(items) if items.is_empty() => "1".into(),
        Condition::Or(items) if items.is_empty() => "0".into(),
        Condition::And(items) | Condition::Or(items) => {
            let join = if matches!(condition, Condition::And(_)) {
                " AND "
            } else {
                " OR "
            };
            let parts = items
                .iter()
                .map(|item| predicate(item, bindings, depth + 1))
                .collect::<Result<Vec<_>, _>>()?;
            format!("({})", parts.join(join))
        }
        Condition::Not(item) => format!("NOT ({})", predicate(item, bindings, depth + 1)?),
        Condition::Kind(kind) => kind_condition(std::iter::once(kind), bindings),
        Condition::LastEditedAt {
            operator,
            utc_micros,
        } => {
            bindings.push(Value::Integer(*utc_micros));
            format!("last_edited_at {} ?", comparison(*operator))
        }
        Condition::Attribute { key, predicate } => attribute_predicate(key, predicate, bindings)?,
        Condition::Conflicted => "conflicted=1".into(),
    };
    Ok(expression)
}

fn attribute_predicate(
    key: &harmion_domain::AttributeKey,
    predicate: &AttributePredicate,
    bindings: &mut Vec<Value>,
) -> Result<String, Error> {
    if matches!(predicate, AttributePredicate::ContainsAny(values) if values.is_empty()) {
        return Ok("0".into());
    }
    bindings.extend([
        Value::Text(key.namespace.clone()),
        Value::Text(key.key.clone()),
    ]);
    let (table, suffix) = match predicate {
        AttributePredicate::Exists => ("search_attributes", String::new()),
        AttributePredicate::Eq(value) => {
            bindings.push(Value::Blob(attribute_key_bytes(value)?));
            ("search_attributes", " AND value = ?".into())
        }
        AttributePredicate::Compare { operator, value } => {
            let (column, key_value) = match value {
                OrderedValue::Number(number) => ("number_key", decimal_key(number)),
                OrderedValue::String(text) => ("text_key", text.as_bytes().to_vec()),
            };
            bindings.push(Value::Blob(key_value));
            (
                "search_attributes",
                format!(" AND {column} {} ?", comparison(*operator)),
            )
        }
        AttributePredicate::Contains(value) => {
            bindings.push(Value::Blob(attribute_key_bytes(value)?));
            ("search_elements", " AND value = ?".into())
        }
        AttributePredicate::ContainsAny(values) => {
            for value in values {
                bindings.push(Value::Blob(attribute_key_bytes(value)?));
            }
            (
                "search_elements",
                format!(" AND value IN ({})", placeholders(values.len())),
            )
        }
        AttributePredicate::ContainsAll(values) if values.is_empty() => {
            ("search_attributes", " AND is_array=1".into())
        }
        AttributePredicate::ContainsAll(values) => {
            let keys = values
                .iter()
                .map(attribute_key_bytes)
                .collect::<Result<BTreeSet<_>, _>>()?;
            let count = keys.len();
            bindings.extend(keys.into_iter().map(Value::Blob));
            bindings.push(Value::Integer(
                i64::try_from(count).map_err(|_| Error::InvalidRequest)?,
            ));
            // Each equality key occurs at most once per Content and attribute.
            (
                "search_elements",
                format!(
                    " AND value IN ({}) GROUP BY content HAVING COUNT(*) = ?",
                    placeholders(count)
                ),
            )
        }
    };
    Ok(attribute_subquery(table, &suffix))
}

// ID membership is two-valued even when the attribute is absent or has the wrong type.
fn attribute_subquery(table: &str, suffix: &str) -> String {
    format!("content IN (SELECT content FROM {table} WHERE namespace=? AND name=?{suffix})")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_index_key_preserves_exact_order() {
        let values = [
            "-9e9223372036854775807",
            "-1.201",
            "-1.2",
            "-1e-9223372036854775808",
            "0",
            "1e-9223372036854775808",
            "1.2",
            "1.201",
            "9007199254740992",
            "9007199254740993",
            "9e9223372036854775807",
        ]
        .map(|text| text.parse::<Number>().unwrap());
        for left in &values {
            for right in &values {
                assert_eq!(decimal_key(left).cmp(&decimal_key(right)), left.cmp(right));
            }
        }
    }
}
