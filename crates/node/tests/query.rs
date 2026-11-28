use harmion_domain::*;
use harmion_node::{
    access::{Filter, Grants},
    query::matches,
};
use harmion_protocol::query::{AttributePredicate, Condition, OrderOperator, OrderedValue};

fn key(name: &str) -> AttributeKey {
    AttributeKey {
        namespace: "test".into(),
        key: name.into(),
    }
}

fn attribute(name: &str, predicate: AttributePredicate) -> Condition {
    Condition::Attribute {
        key: key(name),
        predicate,
    }
}

fn number(text: &str) -> AttributeValue {
    AttributeValue::Number(text.parse().unwrap())
}

#[test]
fn attribute_predicates_use_exact_values_and_distinguish_missing_keys() {
    let tags = vec![number("1"), AttributeValue::Null, number("1.0")];
    let snapshot = ContentSnapshot {
        content: Content {
            id: ContentId([1; 32]),
            body: String::new(),
            attributes: [
                (key("null"), AttributeValue::Null),
                (key("text"), AttributeValue::String("é".into())),
                (key("count"), number("9007199254740993")),
                (key("tags"), AttributeValue::Array(tags.clone())),
            ]
            .into(),
            kinds: Default::default(),
        },
        life: Life::Live,
        last_edited_at: 0,
        basis: Default::default(),
        fields: Default::default(),
    };
    let admin = Grants::administrator();
    let greater_than = |value| AttributePredicate::Compare {
        operator: OrderOperator::Greater,
        value,
    };
    let cases = [
        (attribute("null", AttributePredicate::Exists), true),
        (
            attribute("null", AttributePredicate::Eq(AttributeValue::Null)),
            true,
        ),
        (attribute("missing", AttributePredicate::Exists), false),
        (
            attribute("missing", AttributePredicate::Eq(AttributeValue::Null)),
            false,
        ),
        (
            Condition::Not(Box::new(attribute("missing", AttributePredicate::Exists))),
            true,
        ),
        (
            attribute(
                "count",
                greater_than(OrderedValue::Number("9007199254740992".parse().unwrap())),
            ),
            true,
        ),
        (
            attribute(
                "count",
                greater_than(OrderedValue::String("9007199254740992".into())),
            ),
            false,
        ),
        (
            attribute("text", greater_than(OrderedValue::String("z".into()))),
            true,
        ),
        (
            attribute("tags", AttributePredicate::Contains(number("1e0"))),
            true,
        ),
        (
            attribute(
                "tags",
                AttributePredicate::ContainsAny(vec![number("2"), AttributeValue::Null]),
            ),
            true,
        ),
        (
            attribute("tags", AttributePredicate::ContainsAny(vec![])),
            false,
        ),
        (
            attribute("tags", AttributePredicate::ContainsAll(vec![])),
            true,
        ),
        (
            attribute(
                "tags",
                AttributePredicate::ContainsAll(vec![number("1"), AttributeValue::Null]),
            ),
            true,
        ),
        (
            attribute(
                "tags",
                AttributePredicate::ContainsAll(vec![number("1"), number("2")]),
            ),
            false,
        ),
        (
            attribute("null", AttributePredicate::ContainsAll(vec![])),
            false,
        ),
        (
            attribute(
                "tags",
                AttributePredicate::Eq(AttributeValue::Array(tags.clone())),
            ),
            true,
        ),
        (
            attribute(
                "tags",
                AttributePredicate::Eq(AttributeValue::Array(tags[..2].to_vec())),
            ),
            false,
        ),
        (
            attribute(
                "tags",
                AttributePredicate::Eq(AttributeValue::Array(vec![
                    tags[1].clone(),
                    tags[0].clone(),
                    tags[2].clone(),
                ])),
            ),
            false,
        ),
        (Condition::And(vec![]), true),
        (Condition::Or(vec![]), false),
    ];
    for (condition, expected) in cases {
        assert_eq!(
            matches(&snapshot, &admin, &condition),
            expected,
            "{condition:?}"
        );
    }
}

#[test]
fn queries_apply_read_grants_to_the_normal_value_and_exclude_deleted_content() {
    let cluster = ClusterId([1; 32]);
    let id = ContentId([1; 32]);
    let root = UpdateId([1; 32]);
    let wiki = Field::Kind("wiki".into());
    let mut graph = Graph::new(cluster);
    let create = Update {
        format_version: 1,
        cluster_id: cluster,
        content_id: id,
        root_id: root,
        update_id: root,
        author_node_id: NodeId([1; 32]),
        edited_at: 1,
        action: Action::Create,
        basis: Default::default(),
        changes: [
            (
                Field::Body,
                Change {
                    value: Value::Body("text".into()),
                    replaces: Default::default(),
                },
            ),
            (
                wiki.clone(),
                Change {
                    value: Value::Kind(true),
                    replaces: Default::default(),
                },
            ),
        ]
        .into(),
    };
    graph.accept(create.clone()).unwrap();
    let edit = |id: u8, present| Update {
        update_id: UpdateId([id; 32]),
        edited_at: i64::from(id),
        action: Action::Edit,
        basis: [root].into(),
        changes: [(
            wiki.clone(),
            Change {
                value: Value::Kind(present),
                replaces: [root].into(),
            },
        )]
        .into(),
        ..create.clone()
    };
    graph.accept(edit(2, true)).unwrap();
    graph.accept(edit(3, false)).unwrap();
    let visible = graph.snapshot(id).unwrap();
    assert!(visible.fields[&wiki].is_conflicted());
    let conflicted = Condition::Conflicted;
    let wiki_reader = Grants {
        read: Some(Filter::Kinds(["wiki".into()].into())),
        ..Grants::default()
    };
    let admin = Grants::administrator();
    assert!(matches(&visible, &admin, &conflicted));
    // Conflict state is internal even when the conflicted kind is not public.
    let non_admin = Grants {
        read: Some(Filter::All),
        ..Grants::default()
    };
    assert!(matches(&visible, &non_admin, &conflicted));
    let is_wiki = Condition::Kind("wiki".into());
    let all = Condition::And(vec![]);
    let not_wiki = Condition::Not(Box::new(is_wiki.clone()));
    let edited_at_or_after = |utc_micros| Condition::LastEditedAt {
        operator: OrderOperator::GreaterOrEqual,
        utc_micros,
    };

    assert!(matches(&visible, &admin, &edited_at_or_after(3)));
    assert!(!matches(&visible, &admin, &edited_at_or_after(4)));
    assert!(!matches(&visible, &wiki_reader, &edited_at_or_after(3)));
    assert!(!matches(&visible, &admin, &is_wiki));
    assert!(matches(&visible, &admin, &not_wiki));
    assert!(!matches(&visible, &wiki_reader, &not_wiki));
    assert!(!matches(&visible, &Grants::default(), &all));

    let delete = Update {
        update_id: UpdateId([4; 32]),
        edited_at: 4,
        action: Action::Delete,
        basis: visible.basis,
        changes: Default::default(),
        ..create.clone()
    };
    graph.accept(delete).unwrap();
    let deleted = graph.snapshot(id).unwrap();
    assert_eq!(deleted.life, Life::Deleted);
    assert!(!matches(&deleted, &admin, &all));
    assert!(!matches(&deleted, &admin, &edited_at_or_after(3)));
    assert!(!matches(&deleted, &admin, &conflicted));

    graph.accept(edit(5, true)).unwrap();
    let deletion_conflict = graph.snapshot(id).unwrap();
    assert_eq!(deletion_conflict.life, Life::Conflict);
    assert!(matches(&deletion_conflict, &admin, &conflicted));
    assert!(matches(&deletion_conflict, &wiki_reader, &is_wiki));
    assert!(matches(
        &deletion_conflict,
        &wiki_reader,
        &Condition::And(vec![is_wiki, edited_at_or_after(5)])
    ));
}
