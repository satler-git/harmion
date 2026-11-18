use harmion_domain::*;
use std::collections::{BTreeMap, BTreeSet};

fn id(n: u8) -> UpdateId {
    UpdateId([n; 32])
}
fn ids(ns: &[u8]) -> BTreeSet<UpdateId> {
    ns.iter().map(|n| id(*n)).collect()
}
fn content() -> ContentId {
    ContentId([1; 32])
}
fn graph() -> Graph<String> {
    Graph::new(ClusterId([1; 32]))
}
fn attr(name: &str) -> Field {
    Field::Attribute {
        namespace: "test".into(),
        key: name.into(),
    }
}
fn body(s: &str) -> Value<String> {
    Value::Body(s.into())
}
fn op(
    n: u8,
    action: Action,
    basis: &[u8],
    changes: Vec<(Field, Value<String>, Vec<u8>)>,
) -> Update<String> {
    Update {
        format_version: 1,
        cluster_id: ClusterId([1; 32]),
        content_id: content(),
        root_id: id(1),
        update_id: id(n),
        author_node_id: NodeId([1; 32]),
        edited_at: i64::from(n),
        action,
        basis: ids(basis),
        changes: changes
            .into_iter()
            .map(|(f, value, parents)| {
                (
                    f,
                    Change {
                        value,
                        replaces: ids(&parents),
                    },
                )
            })
            .collect(),
    }
}
fn root() -> Update<String> {
    op(
        1,
        Action::Create,
        &[],
        vec![(Field::Body, body("base"), vec![])],
    )
}
fn edit(n: u8, basis: &[u8], parents: &[u8], text: &str) -> Update<String> {
    op(
        n,
        Action::Edit,
        basis,
        vec![(Field::Body, body(text), parents.to_vec())],
    )
}

// Simulate arbitrary arrival orders; missing dependencies are queued outside Graph.
// Retry every accepted record to represent retransmission. Compare full projections.
fn permutations(ops: Vec<Update<String>>, check: impl Fn(&Graph<String>)) {
    fn walk(ops: &mut [Update<String>], at: usize, check: &impl Fn(&Graph<String>)) {
        if at != ops.len() {
            for i in at..ops.len() {
                ops.swap(at, i);
                walk(ops, at + 1, check);
                ops.swap(at, i);
            }
            return;
        }
        let mut g = graph();
        let mut pending = Vec::new();
        for update in ops.iter() {
            pending.push(update.clone());
            loop {
                let mut progress = false;
                for i in (0..pending.len()).rev() {
                    match g.accept(pending[i].clone()) {
                        Ok(Acceptance::Inserted) => {
                            assert_eq!(g.accept(pending.remove(i)), Ok(Acceptance::Duplicate));
                            progress = true;
                        }
                        Err(Error::MissingDependency(_)) => (),
                        result => panic!("unexpected acceptance: {result:?}"),
                    }
                }
                if !progress {
                    break;
                }
            }
        }
        assert!(pending.is_empty());
        check(&g);
    }
    let mut reference = graph();
    for op in &ops {
        reference.accept(op.clone()).unwrap();
    }
    let expected = reference.snapshot(content());
    walk(&mut ops.clone(), 0, &|g| {
        assert_eq!(g.snapshot(content()), expected);
        check(g);
    });
}
fn field_heads(g: &Graph<String>, field: Field, expected: &[u8]) {
    assert_eq!(g.heads(content()).unwrap().fields[&field], ids(expected));
}

#[test]
fn resolution_and_late_edit_converge() {
    permutations(
        vec![
            root(),
            edit(2, &[1], &[1], "A"),
            edit(3, &[1], &[1], "B"),
            edit(4, &[3], &[3], "B2"),
            edit(5, &[2, 3], &[2, 3], "resolved"),
        ],
        |g| {
            field_heads(g, Field::Body, &[4, 5]);
            assert_eq!(g.heads(content()).unwrap().life, ids(&[4, 5]));
            assert!(g.is_conflicted(content(), &Field::Body));
            assert_eq!(
                g.normal(content(), &Field::Body),
                Some((id(5), &body("resolved")))
            );
        },
    );
}

#[test]
fn unrelated_edit_preserves_body_conflict() {
    permutations(
        vec![
            root(),
            edit(2, &[1], &[1], "A"),
            edit(3, &[1], &[1], "B"),
            op(
                4,
                Action::Edit,
                &[2, 3],
                vec![(attr("title"), Value::Attribute("title".into()), vec![])],
            ),
        ],
        |g| {
            field_heads(g, Field::Body, &[2, 3]);
            assert_eq!(g.heads(content()).unwrap().life, ids(&[4]));
        },
    );
}

#[test]
fn edit_does_not_keep_and_keep_can_conflict_with_delete() {
    let a = edit(2, &[1], &[1], "A");
    let d = op(3, Action::Delete, &[1], vec![]);
    permutations(
        vec![root(), a.clone(), d.clone(), edit(4, &[2, 3], &[2], "E")],
        |g| {
            assert_eq!(g.life(content()), Some(Life::Conflict));
            assert_eq!(g.heads(content()).unwrap().life, ids(&[3, 4]));
            field_heads(g, Field::Body, &[4]);
        },
    );
    permutations(
        vec![
            root(),
            a,
            d,
            op(4, Action::Keep, &[2, 3], vec![]),
            op(5, Action::Delete, &[2, 3], vec![]),
        ],
        |g| {
            assert_eq!(g.life(content()), Some(Life::Conflict));
            assert_eq!(g.heads(content()).unwrap().life, ids(&[4, 5]));
        },
    );
}

#[test]
fn clocks_do_not_restore_ancestors_and_ties_use_id() {
    let mut later = edit(3, &[2], &[2], "later");
    later.edited_at = i64::MIN;
    permutations(vec![root(), edit(2, &[1], &[1], "A"), later], |g| {
        assert_eq!(
            g.normal(content(), &Field::Body),
            Some((id(3), &body("later")))
        );
    });
    let mut a = edit(2, &[1], &[1], "A");
    let mut b = edit(3, &[1], &[1], "B");
    a.edited_at = i64::MAX;
    b.edited_at = i64::MAX;
    permutations(vec![root(), a, b], |g| {
        assert_eq!(g.normal(content(), &Field::Body).unwrap().0, id(3))
    });
}

#[test]
fn kind_value_groups_retain_all_parents() {
    let kind = Field::Kind("wiki".into());
    let change = |n, basis: &[u8], parents: Vec<u8>, value| {
        op(
            n,
            Action::Edit,
            basis,
            vec![(kind.clone(), Value::Kind(value), parents)],
        )
    };
    let adds = vec![
        root(),
        change(2, &[1], vec![], true),
        change(3, &[1], vec![], true),
    ];
    permutations(adds.clone(), |g| {
        field_heads(g, kind.clone(), &[2, 3]);
        assert!(!g.is_conflicted(content(), &kind));
    });
    let mut remove = adds;
    remove.push(change(4, &[2, 3], vec![2, 3], false));
    permutations(remove.clone(), |g| field_heads(g, kind.clone(), &[4]));
    remove.push(change(5, &[2, 3], vec![2, 3], true));
    permutations(remove, |g| {
        field_heads(g, kind.clone(), &[4, 5]);
        assert!(g.is_conflicted(content(), &kind));
    });
}

#[test]
fn restore_resets_known_fields_but_retains_unseen_updates() {
    let mut r = root();
    r.changes.insert(
        attr("secret"),
        Change {
            value: Value::Attribute("old".into()),
            replaces: ids(&[]),
        },
    );
    r.changes.insert(
        Field::Kind("wiki".into()),
        Change {
            value: Value::Kind(true),
            replaces: ids(&[]),
        },
    );
    let d = op(2, Action::Delete, &[1], vec![]);
    let mut g = graph();
    g.accept(r.clone()).unwrap();
    g.accept(d.clone()).unwrap();
    let mut restore = op(3, Action::Restore, &[2], vec![]);
    restore.changes = g
        .restore_changes(
            &ids(&[2]),
            BTreeMap::from([(Field::Body, body("restored"))]),
        )
        .unwrap();
    assert_eq!(restore.changes[&attr("secret")].value, Value::Absent);
    assert_eq!(
        restore.changes[&Field::Kind("wiki".into())].value,
        Value::Kind(false)
    );
    let unseen = op(
        4,
        Action::Edit,
        &[1],
        vec![
            (attr("secret"), Value::Attribute("new".into()), vec![1]),
            (attr("unseen"), Value::Attribute("added".into()), vec![]),
        ],
    );
    permutations(vec![r.clone(), d.clone(), restore.clone(), unseen], |g| {
        field_heads(g, attr("secret"), &[3, 4]);
        field_heads(g, attr("unseen"), &[4]);
        field_heads(g, Field::Body, &[3]);
    });
    let mut other = restore.clone();
    other.update_id = id(4);
    other.changes.get_mut(&Field::Body).unwrap().value = body("other");
    permutations(vec![r, d, restore.clone(), other], |g| {
        field_heads(g, Field::Body, &[3, 4])
    });
    restore.changes.remove(&attr("secret"));
    assert_eq!(g.accept(restore), Err(Error::IncompleteRestore));
}

#[test]
fn parallel_deletes_are_not_a_life_conflict() {
    permutations(
        vec![
            root(),
            op(2, Action::Delete, &[1], vec![]),
            op(3, Action::Delete, &[1], vec![]),
        ],
        |g| {
            assert_eq!(g.life(content()), Some(Life::Deleted));
            assert_eq!(g.heads(content()).unwrap().life, ids(&[2, 3]));
            field_heads(g, Field::Body, &[1]);
        },
    );
}

#[test]
fn rejected_records_leave_state_unchanged() {
    let mut g = graph();
    g.accept(root()).unwrap();
    let before = g.heads(content());
    let mut bad = Vec::new();
    let mut mismatch = root();
    mismatch.edited_at = 99;
    bad.push(mismatch);
    let mut version = edit(2, &[1], &[1], "x");
    version.format_version = 2;
    bad.push(version);
    let mut cluster = edit(2, &[1], &[1], "x");
    cluster.cluster_id = ClusterId([2; 32]);
    bad.push(cluster);
    let mut collision = root();
    collision.update_id = id(2);
    collision.root_id = id(2);
    bad.push(collision);
    bad.push(edit(2, &[9], &[1], "x"));
    bad.push(edit(2, &[2], &[1], "x"));
    bad.push(edit(2, &[], &[], "x"));
    bad.push(edit(2, &[1], &[9], "x"));
    bad.push(op(2, Action::Keep, &[1], vec![]));
    bad.push(op(2, Action::Restore, &[1], vec![]));
    bad.push(op(
        2,
        Action::Delete,
        &[1],
        vec![(Field::Body, body("x"), vec![])],
    ));
    bad.push(op(
        2,
        Action::Edit,
        &[1],
        vec![(Field::Body, Value::Absent, vec![1])],
    ));
    bad.push(op(
        2,
        Action::Edit,
        &[1],
        vec![(attr("new"), Value::Absent, vec![1])],
    ));
    for op in bad {
        assert!(g.accept(op).is_err());
        assert_eq!(g.heads(content()), before);
    }
    g.accept(edit(2, &[1], &[1], "x")).unwrap();
    assert_eq!(
        g.accept(edit(3, &[1, 2], &[2], "x")),
        Err(Error::InvalidBasis)
    );
    g.accept(op(3, Action::Delete, &[2], vec![])).unwrap();
    assert_eq!(
        g.accept(edit(4, &[3], &[2], "x")),
        Err(Error::InvalidAction)
    );
    // An old, valid edit remains admissible after deletion.
    g.accept(edit(4, &[1], &[1], "late")).unwrap();
    assert_eq!(g.life(content()), Some(Life::Conflict));
}

#[test]
fn content_roots_and_references_are_isolated() {
    let mut g = graph();
    g.accept(root()).unwrap();
    let mut second = root();
    second.content_id = ContentId([2; 32]);
    second.root_id = id(2);
    second.update_id = id(2);
    g.accept(second).unwrap();
    assert_eq!(g.accept(edit(3, &[2], &[2], "x")), Err(Error::InvalidBasis));
    let mut wrong_root = edit(3, &[1], &[1], "x");
    wrong_root.root_id = id(2);
    assert_eq!(g.accept(wrong_root), Err(Error::RootCollision));
}
