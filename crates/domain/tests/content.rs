use harmion_domain::*;
use std::collections::{BTreeMap, BTreeSet};

fn id(n: u8) -> UpdateId {
    UpdateId([n; 32])
}
fn ids(ns: &[u8]) -> BTreeSet<UpdateId> {
    ns.iter().map(|n| id(*n)).collect()
}
fn content_id() -> ContentId {
    ContentId([1; 32])
}
fn attr(namespace: &str, key: &str) -> Field {
    Field::Attribute {
        namespace: namespace.into(),
        key: key.into(),
    }
}
fn key(namespace: &str, key: &str) -> AttributeKey {
    AttributeKey {
        namespace: namespace.into(),
        key: key.into(),
    }
}
fn op(n: u8, action: Action, basis: &[u8], changes: Vec<(Field, Value, Vec<u8>)>) -> Update {
    Update {
        format_version: 1,
        cluster_id: ClusterId([1; 32]),
        content_id: content_id(),
        root_id: id(1),
        update_id: id(n),
        author_node_id: NodeId([1; 32]),
        edited_at: i64::from(n),
        action,
        basis: ids(basis),
        changes: changes
            .into_iter()
            .map(|(field, value, parents)| {
                (
                    field,
                    Change {
                        value,
                        replaces: ids(&parents),
                    },
                )
            })
            .collect(),
    }
}
fn root() -> Update {
    op(
        1,
        Action::Create,
        &[],
        vec![(Field::Body, Value::Body("base".into()), vec![])],
    )
}
fn graph() -> Graph {
    Graph::new(ClusterId([1; 32]))
}

#[test]
fn materializes_typed_content_and_preserves_absence_distinctions() {
    let mut g = graph();
    assert!(g.snapshot(content_id()).is_none());
    let object = AttributeValue::Object(BTreeMap::from([
        (
            "list".into(),
            AttributeValue::Array(vec![
                AttributeValue::Null,
                AttributeValue::Bool(true),
                AttributeValue::Null,
            ]),
        ),
        (
            "large".into(),
            AttributeValue::Number("9007199254740993".parse().unwrap()),
        ),
    ]));
    let r = op(
        1,
        Action::Create,
        &[],
        vec![
            (Field::Body, Value::Body("本文\n".into()), vec![]),
            (
                attr("a/b", "c"),
                Value::Attribute(AttributeValue::Null),
                vec![],
            ),
            (
                attr("a", "b/c"),
                Value::Attribute(AttributeValue::String(String::new())),
                vec![],
            ),
            (
                attr("app", "object"),
                Value::Attribute(object.clone()),
                vec![],
            ),
            (attr("app", "absent"), Value::Absent, vec![]),
            (Field::Kind("wiki".into()), Value::Kind(true), vec![]),
            (Field::Kind("ledger".into()), Value::Kind(false), vec![]),
        ],
    );
    g.accept(r).unwrap();
    let s = g.snapshot(content_id()).unwrap();
    assert_eq!(s.content.body, "本文\n");
    assert_eq!(s.content.id, content_id());
    assert_eq!(s.content.attributes.len(), 3);
    assert_eq!(s.content.attributes[&key("a/b", "c")], AttributeValue::Null);
    assert_eq!(
        s.content.attributes[&key("a", "b/c")],
        AttributeValue::String(String::new())
    );
    assert_eq!(s.content.attributes[&key("app", "object")], object);
    assert_eq!(s.content.kinds, BTreeSet::from(["wiki".into()]));
    assert_eq!(s.visible_content(), Some(&s.content));
    assert_eq!(s.edit_parents()[&attr("app", "absent")], ids(&[1]));
    assert_eq!(s.edit_parents()[&Field::Kind("ledger".into())], ids(&[1]));
}

#[test]
fn grouped_heads_and_edit_basis_are_frozen_while_other_updates_arrive() {
    let mut g = graph();
    g.accept(root()).unwrap();
    let change = |n, value: &str| {
        op(
            n,
            Action::Edit,
            &[1],
            vec![(Field::Body, Value::Body(value.into()), vec![1])],
        )
    };
    g.accept(change(2, "same")).unwrap();
    g.accept(change(3, "same")).unwrap();
    g.accept(change(4, "other")).unwrap();
    // Select the equal-valued group, despite its lower IDs.
    let mut same = change(5, "same");
    same.edited_at = 100;
    g.accept(same).unwrap();
    let old = g.snapshot(content_id()).unwrap();
    assert_eq!(old.content.body, "same");
    assert!(old.fields[&Field::Body].is_conflicted());
    assert_eq!(old.fields[&Field::Body].candidates.len(), 2);
    assert_eq!(old.edit_parents()[&Field::Body], ids(&[2, 3, 5]));
    g.accept(change(6, "late")).unwrap();
    let mut edit = op(7, Action::Edit, &[], vec![]);
    edit.basis = old.basis.clone();
    edit.changes.insert(
        Field::Body,
        Change {
            value: Value::Body("edited".into()),
            replaces: old.edit_parents()[&Field::Body].clone(),
        },
    );
    g.accept(edit).unwrap();
    assert_eq!(old.content.body, "same");
    assert_eq!(old.basis, ids(&[2, 3, 4, 5]));
    assert_eq!(
        g.heads(content_id()).unwrap().fields[&Field::Body],
        ids(&[4, 6, 7])
    );
}

#[test]
fn deleted_snapshots_remain_internal_and_late_edits_show_deletion_conflict() {
    let mut g = graph();
    g.accept(root()).unwrap();
    g.accept(op(2, Action::Delete, &[1], vec![])).unwrap();
    let deleted = g.snapshot(content_id()).unwrap();
    assert_eq!(deleted.life, Life::Deleted);
    assert!(deleted.visible_content().is_none());
    assert_eq!(deleted.content.body, "base");
    g.accept(op(
        3,
        Action::Edit,
        &[1],
        vec![(
            attr("app", "title"),
            Value::Attribute(AttributeValue::String("late".into())),
            vec![],
        )],
    ))
    .unwrap();
    let conflict = g.snapshot(content_id()).unwrap();
    assert_eq!(conflict.life, Life::Conflict);
    assert_eq!(conflict.visible_content().unwrap().attributes.len(), 1);
    assert_eq!(conflict.content.body, "base");
    assert_eq!(conflict.basis, ids(&[2, 3]));
    assert!(deleted.visible_content().is_none());
}

#[test]
fn unset_and_false_omit_normal_values_but_keep_conflict_candidates() {
    let mut g = graph();
    g.accept(root()).unwrap();
    g.accept(op(
        2,
        Action::Edit,
        &[1],
        vec![
            (
                attr("app", "value"),
                Value::Attribute(AttributeValue::Null),
                vec![],
            ),
            (Field::Kind("wiki".into()), Value::Kind(true), vec![]),
        ],
    ))
    .unwrap();
    g.accept(op(
        3,
        Action::Edit,
        &[1],
        vec![
            (attr("app", "value"), Value::Absent, vec![]),
            (Field::Kind("wiki".into()), Value::Kind(false), vec![]),
        ],
    ))
    .unwrap();
    let s = g.snapshot(content_id()).unwrap();
    assert!(s.content.attributes.is_empty());
    assert!(s.content.kinds.is_empty());
    assert!(s.fields[&attr("app", "value")].is_conflicted());
    assert!(s.fields[&Field::Kind("wiki".into())].is_conflicted());
    assert_eq!(s.edit_parents()[&attr("app", "value")], ids(&[3]));
    assert_eq!(s.edit_parents()[&Field::Kind("wiki".into())], ids(&[3]));
}

#[test]
fn structural_equality_controls_attribute_conflicts() {
    let mut g = graph();
    g.accept(root()).unwrap();
    let number = |text: &str| AttributeValue::Number(text.parse().unwrap());
    for (n, text) in [(2, "1"), (3, "1.0"), (4, "10e-1")] {
        g.accept(op(
            n,
            Action::Edit,
            &[1],
            vec![(attr("app", "n"), Value::Attribute(number(text)), vec![])],
        ))
        .unwrap();
    }
    let s = g.snapshot(content_id()).unwrap();
    assert!(!s.fields[&attr("app", "n")].is_conflicted());
    assert_eq!(s.edit_parents()[&attr("app", "n")], ids(&[2, 3, 4]));
    assert_ne!(number("9007199254740992"), number("9007199254740993"));
    let array = |xs: &[&str]| AttributeValue::Array(xs.iter().map(|x| number(x)).collect());
    assert_ne!(array(&["1", "2"]), array(&["2", "1"]));
    assert_ne!(array(&["1"]), array(&["1", "1"]));
    assert_eq!(
        AttributeValue::Object(BTreeMap::from([
            ("a".into(), number("1")),
            ("b".into(), number("2"))
        ])),
        AttributeValue::Object(BTreeMap::from([
            ("b".into(), number("2.0")),
            ("a".into(), number("1.0"))
        ])),
    );
}

#[test]
fn exact_numbers_validate_syntax_and_normalize_without_rounding() {
    for (input, expected) in [
        ("-0.00e+5", "0"),
        ("123.4500", "12345e-2"),
        ("0.001", "1e-3"),
        ("100", "1e2"),
        ("-10e-1", "-1"),
        ("1E+2", "1e2"),
        ("1.0e-9223372036854775808", "1e-9223372036854775808"),
    ] {
        let n: Number = input.parse().unwrap();
        assert_eq!(n.to_string(), expected);
        assert_eq!(n, expected.parse().unwrap());
    }
    for invalid in [
        "", "-", "+1", "01", "-01", ".1", "1.", "1e", "1e+", "1e--1", "1e1e1", "1.2.3", "NaN",
        "Infinity", " 1", "1 ", "１",
    ] {
        assert_eq!(
            invalid.parse::<Number>(),
            Err(NumberError::InvalidSyntax),
            "{invalid}"
        );
    }
    for overflow in [
        "1e9223372036854775808",
        "10e9223372036854775807",
        "0.1e-9223372036854775808",
    ] {
        assert_eq!(
            overflow.parse::<Number>(),
            Err(NumberError::ExponentOutOfRange)
        );
    }
    assert_eq!(Number::from(i64::MIN).to_string(), i64::MIN.to_string());
    assert_eq!(Number::from(u64::MAX).to_string(), u64::MAX.to_string());
}
