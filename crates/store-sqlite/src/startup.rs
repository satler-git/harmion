//! Verify persisted Content histories without retaining the entire node graph.
//!
//! The mapping is derived, not an authority: check coverage before using it to
//! partition the authoritative log. Never repair or omit invalid rows.
use crate::{db, fixed};
use harmion_domain::{Acceptance, ContentId, Graph};
use harmion_node::{Error, storage::Identity};
use harmion_protocol::Record;
use rusqlite::Connection;

const CONTENT_ROWS: &str = "SELECT records.content,records.seq,log.update_id,log.bytes,log.digest
    FROM content_records AS records LEFT JOIN accept_log AS log ON log.seq=records.seq
    ORDER BY records.content,records.seq";

pub(super) fn verify_content_history(sql: &Connection, identity: Identity) -> Result<(), Error> {
    verify_with(sql, identity, |_| {})
}

fn verify_with(
    sql: &Connection,
    identity: Identity,
    mut verified_content: impl FnMut(&Graph),
) -> Result<(), Error> {
    // An inner join or a traversal of mapped records alone would silently hide
    // authoritative rows with a missing mapping. Counts alone are insufficient.
    for check in [
        "SELECT EXISTS(SELECT 1 FROM accept_log AS log WHERE NOT EXISTS
            (SELECT 1 FROM content_records AS records WHERE records.seq=log.seq))",
        "SELECT EXISTS(SELECT 1 FROM content_records GROUP BY seq HAVING COUNT(*)<>1)",
        // Normally protected by UNIQUE/PRIMARY KEY. Check explicitly to retain
        // the old whole-Graph duplicate guarantees even for a weakened schema.
        "SELECT EXISTS(SELECT 1 FROM accept_log GROUP BY update_id HAVING COUNT(*)<>1)",
        "SELECT EXISTS(SELECT 1 FROM accept_log GROUP BY seq HAVING COUNT(*)<>1)",
    ] {
        if sql
            .query_row(check, [], |row| row.get::<_, bool>(0))
            .map_err(db)?
        {
            return Err(Error::CorruptLog);
        }
    }
    let mut statement = sql.prepare(CONTENT_ROWS).map_err(db)?;
    let mut rows = statement.query([]).map_err(db)?;
    let mut current = None;
    let mut previous = 0;
    let mut graph = Graph::new(identity.cluster);
    while let Some(row) = rows.next().map_err(db)? {
        let content = ContentId(fixed(row.get(0).map_err(db)?)?);
        let sequence: i64 = row.get(1).map_err(db)?;
        if current != Some(content) {
            if current.is_some() {
                verified_content(&graph);
            }
            // Drop the old Content before decoding the first row of the next.
            graph = Graph::new(identity.cluster);
            current = Some(content);
            previous = 0;
        }
        if sequence <= previous {
            return Err(Error::CorruptLog);
        }
        previous = sequence;
        // NULLs from the LEFT JOIN expose orphan mapping rows. Borrow encoded
        // bytes from SQLite rather than collecting raw records into a Vec.
        let id = fixed(row.get(2).map_err(db)?)?;
        let bytes = row.get_ref(3).map_err(db)?;
        let bytes = bytes.as_blob().map_err(|_| Error::CorruptLog)?;
        let digest = fixed(row.get(4).map_err(db)?)?;
        let record = Record::from_bytes(bytes)?;
        if record.update().update_id.0 != id
            || *record.digest() != digest
            || record.update().content_id != content
        {
            return Err(Error::CorruptLog);
        }
        // Dependencies may only refer to earlier updates of this same Content.
        // Cross-Content roots/bases/replacements must fail, never be imported
        // from a different partition to make an invalid record acceptable.
        if graph.accept(record.update().clone())? != Acceptance::Inserted {
            return Err(Error::CorruptLog);
        }
    }
    if current.is_some() {
        verified_content(&graph);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use harmion_domain::{Action, Change, ClusterId, Field, NodeId, Update, UpdateId, Value};
    use harmion_protocol::LogEpoch;
    use rusqlite::params;
    use std::collections::{BTreeMap, BTreeSet};

    #[test]
    fn verification_visits_one_content_graph_at_a_time_with_index_ordered_rows() {
        let sql = Connection::open_in_memory().unwrap();
        sql.execute_batch("CREATE TABLE accept_log(seq INTEGER PRIMARY KEY, update_id BLOB UNIQUE, bytes BLOB, digest BLOB);
            CREATE TABLE content_records(content BLOB NOT NULL, seq INTEGER NOT NULL UNIQUE REFERENCES accept_log(seq), PRIMARY KEY(content,seq));").unwrap();
        let identity = Identity {
            cluster: ClusterId([1; 32]),
            node: NodeId([1; 32]),
            epoch: LogEpoch([1; 32]),
        };
        for n in 1..=8 {
            let record = Record::from_update(Update {
                format_version: 1,
                cluster_id: identity.cluster,
                content_id: ContentId([n; 32]),
                root_id: UpdateId([n; 32]),
                update_id: UpdateId([n; 32]),
                author_node_id: identity.node,
                edited_at: 0,
                action: Action::Create,
                basis: BTreeSet::new(),
                changes: BTreeMap::from([(
                    Field::Body,
                    Change {
                        value: Value::Body("body".into()),
                        replaces: BTreeSet::new(),
                    },
                )]),
            })
            .unwrap();
            sql.execute(
                "INSERT INTO accept_log VALUES(?1,?2,?3,?4)",
                params![
                    i64::from(n),
                    record.update().update_id.0,
                    record.bytes(),
                    record.digest()
                ],
            )
            .unwrap();
            sql.execute(
                "INSERT INTO content_records VALUES(?1,?2)",
                params![record.update().content_id.0, i64::from(n)],
            )
            .unwrap();
        }
        let mut visited = 0;
        verify_with(&sql, identity, |graph| {
            visited += 1;
            assert_eq!(graph.content_ids().count(), 1);
            assert_eq!(graph.updates().count(), 1);
        })
        .unwrap();
        assert_eq!(visited, 8);
        let plan: Vec<String> = sql
            .prepare(&format!("EXPLAIN QUERY PLAN {CONTENT_ROWS}"))
            .unwrap()
            .query_map([], |row| row.get(3))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(
            !plan.iter().any(|step| step.contains("TEMP B-TREE")),
            "{plan:?}"
        );
        assert!(
            plan.iter()
                .any(|step| step.contains("COVERING INDEX") && step.contains("content_records")),
            "{plan:?}"
        );
        eprintln!("startup stream query plan: {plan:?}");
    }
}
