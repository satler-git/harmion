//! Pure, in-memory update validation and projection. No persistence or authorization.
//!
//! `AttributeValue` provides exact JSON-shaped values. Wire decoding, size limits,
//! and duplicate-key rejection belong to the future protocol codec.

mod attribute;
mod content;

pub use attribute::{AttributeValue, Number, NumberError};
pub use content::{AttributeKey, Candidate, Content, ContentSnapshot, FieldState};

use std::collections::{BTreeMap, BTreeSet};

macro_rules! id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub [u8; 32]);
    };
}
id!(ClusterId);
id!(ContentId);
id!(UpdateId);
id!(NodeId);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Field {
    Body,
    Attribute { namespace: String, key: String },
    Kind(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value<V = AttributeValue> {
    Body(String),
    Attribute(V),
    Absent,
    Kind(bool),
}

impl<V> Value<V> {
    fn matches(&self, field: &Field) -> bool {
        matches!(
            (field, self),
            (Field::Body, Self::Body(_))
                | (Field::Attribute { .. }, Self::Attribute(_) | Self::Absent)
                | (Field::Kind(_), Self::Kind(_))
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change<V = AttributeValue> {
    pub value: Value<V>,
    pub replaces: BTreeSet<UpdateId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Create,
    Edit,
    Delete,
    Keep,
    Restore,
}

impl Action {
    fn live(self) -> bool {
        self != Self::Delete
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Update<V = AttributeValue> {
    pub format_version: u32,
    pub cluster_id: ClusterId,
    pub content_id: ContentId,
    pub root_id: UpdateId,
    pub update_id: UpdateId,
    pub author_node_id: NodeId,
    pub edited_at: i64,
    pub action: Action,
    pub basis: BTreeSet<UpdateId>,
    pub changes: BTreeMap<Field, Change<V>>,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{self:?}")]
pub enum Error {
    UnsupportedVersion,
    WrongCluster,
    IdMismatch,
    RootCollision,
    MissingDependency(UpdateId),
    InvalidBasis,
    InvalidAction,
    InvalidValue(Field),
    InvalidReplacement(Field),
    IncompleteRestore,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Acceptance {
    Inserted,
    Duplicate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Life {
    Live,
    Deleted,
    Conflict,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Heads {
    pub life: BTreeSet<UpdateId>,
    pub fields: BTreeMap<Field, BTreeSet<UpdateId>>,
}

/// Owns accepted immutable records; callers cannot mutate them after acceptance.
#[derive(Clone, Debug)]
pub struct Graph<V = AttributeValue> {
    cluster: ClusterId,
    updates: BTreeMap<UpdateId, Update<V>>,
    content_updates: BTreeMap<ContentId, BTreeSet<UpdateId>>,
}

impl<V: Eq + Clone> Graph<V> {
    /// Project all fields from one accepted update set into an owned snapshot.
    /// Returns `None` only for an unknown ID; deleted contents remain inspectable.
    pub fn snapshot(&self, id: ContentId) -> Option<ContentSnapshot<V>> {
        let heads = self.heads(id)?;
        Some(self.snapshot_from_heads(id, heads))
    }

    /// Reconstruct the fixed internal snapshot behind a local edit token.
    /// Not a general public historical-revision API.
    pub fn snapshot_at(
        &self,
        id: ContentId,
        basis: &BTreeSet<UpdateId>,
    ) -> Result<ContentSnapshot<V>, Error> {
        if basis.is_empty() {
            return Err(Error::InvalidBasis);
        }
        let scope = self.closure(basis)?;
        if scope.iter().any(|key| self.updates[key].content_id != id) {
            return Err(Error::InvalidBasis);
        }
        let heads = self.project(&scope);
        if heads.life != *basis {
            return Err(Error::InvalidBasis);
        }
        Ok(self.snapshot_from_heads(id, heads))
    }

    fn snapshot_from_heads(&self, id: ContentId, heads: Heads) -> ContentSnapshot<V> {
        let life = self.life_from_heads(&heads);
        let mut content = Content {
            id,
            body: String::new(),
            attributes: BTreeMap::new(),
            kinds: BTreeSet::new(),
        };
        let mut fields = BTreeMap::new();
        // Every accepted Content has a body, so at least one field contributes.
        let mut last_edited_at = i64::MIN;
        for (field, versions) in heads.fields {
            let selected = *versions
                .iter()
                .max_by_key(|id| (self.updates[id].edited_at, **id))
                .expect("accepted fields have at least one head");
            let update = &self.updates[&selected];
            last_edited_at = last_edited_at.max(update.edited_at);
            let value = &update.changes[&field].value;
            match (&field, value) {
                (Field::Body, Value::Body(text)) => content.body = text.clone(),
                (Field::Attribute { namespace, key }, Value::Attribute(value)) => {
                    content.attributes.insert(
                        AttributeKey {
                            namespace: namespace.clone(),
                            key: key.clone(),
                        },
                        value.clone(),
                    );
                }
                (Field::Kind(name), Value::Kind(true)) => {
                    content.kinds.insert(name.clone());
                }
                (Field::Attribute { .. }, Value::Absent) | (Field::Kind(_), Value::Kind(false)) => {
                }
                _ => unreachable!("accepted field values match their keys"),
            }
            let mut candidates: Vec<Candidate<V>> = Vec::new();
            for version in versions {
                let value = &self.updates[&version].changes[&field].value;
                if let Some(candidate) = candidates.iter_mut().find(|c| &c.value == value) {
                    candidate.versions.insert(version);
                } else {
                    candidates.push(Candidate {
                        value: value.clone(),
                        versions: BTreeSet::from([version]),
                    });
                }
            }
            fields.insert(
                field,
                FieldState {
                    selected,
                    candidates,
                },
            );
        }
        ContentSnapshot {
            content,
            life,
            last_edited_at,
            basis: heads.life,
            fields,
        }
    }
}

impl<V: Eq> Graph<V> {
    pub fn new(cluster: ClusterId) -> Self {
        Self {
            cluster,
            updates: BTreeMap::new(),
            content_updates: BTreeMap::new(),
        }
    }

    pub fn update(&self, id: UpdateId) -> Option<&Update<V>> {
        self.updates.get(&id)
    }

    pub fn content_ids(&self) -> impl Iterator<Item = ContentId> + '_ {
        self.content_updates.keys().copied()
    }

    pub fn updates(&self) -> impl Iterator<Item = &Update<V>> + '_ {
        self.updates.values()
    }

    /// Body versions visible in the shared causal history of two updates.
    pub fn common_body_versions(
        &self,
        left: UpdateId,
        right: UpdateId,
    ) -> Result<BTreeSet<UpdateId>, Error> {
        let left_scope = self.closure(&BTreeSet::from([left]))?;
        let right_scope = self.closure(&BTreeSet::from([right]))?;
        if self.updates[&left].content_id != self.updates[&right].content_id {
            return Err(Error::InvalidBasis);
        }
        let shared = left_scope.intersection(&right_scope).copied().collect();
        Ok(self
            .project(&shared)
            .fields
            .remove(&Field::Body)
            .unwrap_or_default())
    }

    fn closure(&self, basis: &BTreeSet<UpdateId>) -> Result<BTreeSet<UpdateId>, Error> {
        let mut found = BTreeSet::new();
        let mut pending: Vec<_> = basis.iter().copied().collect();
        while let Some(id) = pending.pop() {
            if found.insert(id) {
                let op = self.updates.get(&id).ok_or(Error::MissingDependency(id))?;
                pending.extend(&op.basis);
            }
        }
        Ok(found)
    }

    // Set-based projection also works when record IDs are not in causal order.
    fn project(&self, scope: &BTreeSet<UpdateId>) -> Heads {
        let mut result = Heads {
            life: scope.clone(),
            fields: BTreeMap::new(),
        };
        let mut removed: BTreeMap<Field, BTreeSet<UpdateId>> = BTreeMap::new();
        for id in scope {
            let op = &self.updates[id];
            for parent in &op.basis {
                if op.action != Action::Edit || self.updates[parent].action.live() {
                    result.life.remove(parent);
                }
            }
            for (field, change) in &op.changes {
                result.fields.entry(field.clone()).or_default().insert(*id);
                removed
                    .entry(field.clone())
                    .or_default()
                    .extend(&change.replaces);
            }
        }
        for (field, ids) in removed {
            result
                .fields
                .get_mut(&field)
                .unwrap()
                .retain(|id| !ids.contains(id));
        }
        result
    }

    pub fn heads(&self, content: ContentId) -> Option<Heads> {
        self.content_updates
            .get(&content)
            .map(|ids| self.project(ids))
    }

    pub fn life(&self, content: ContentId) -> Option<Life> {
        let heads = self.heads(content)?;
        Some(self.life_from_heads(&heads))
    }

    fn life_from_heads(&self, heads: &Heads) -> Life {
        let live = heads.life.iter().any(|id| self.updates[id].action.live());
        let deleted = heads.life.iter().any(|id| !self.updates[id].action.live());
        match (live, deleted) {
            (true, true) => Life::Conflict,
            (true, false) => Life::Live,
            _ => Life::Deleted,
        }
    }

    /// All equal-valued heads remain available to serve as the next edit's parents.
    pub fn candidates(&self, content: ContentId, field: &Field) -> Vec<(UpdateId, &Value<V>)> {
        self.heads(content)
            .and_then(|h| h.fields.get(field).cloned())
            .unwrap_or_default()
            .into_iter()
            .map(|id| (id, &self.updates[&id].changes[field].value))
            .collect()
    }

    pub fn normal(&self, content: ContentId, field: &Field) -> Option<(UpdateId, &Value<V>)> {
        self.candidates(content, field)
            .into_iter()
            .max_by_key(|(id, _)| (self.updates[id].edited_at, *id))
    }

    pub fn is_conflicted(&self, content: ContentId, field: &Field) -> bool {
        let candidates = self.candidates(content, field);
        candidates
            .first()
            .is_some_and(|(_, first)| candidates.iter().any(|(_, v)| v != first))
    }

    pub fn accept(&mut self, op: Update<V>) -> Result<Acceptance, Error> {
        if let Some(existing) = self.updates.get(&op.update_id) {
            return if existing == &op {
                Ok(Acceptance::Duplicate)
            } else {
                Err(Error::IdMismatch)
            };
        }
        if op.format_version != 1 {
            return Err(Error::UnsupportedVersion);
        }
        if op.cluster_id != self.cluster {
            return Err(Error::WrongCluster);
        }
        if op.basis.contains(&op.update_id) {
            return Err(Error::InvalidBasis);
        }
        if op.action == Action::Create {
            if self.content_updates.contains_key(&op.content_id) {
                return Err(Error::RootCollision);
            }
            if op.root_id != op.update_id
                || !op.basis.is_empty()
                || !op.changes.contains_key(&Field::Body)
            {
                return Err(Error::InvalidAction);
            }
        } else {
            let root = self
                .updates
                .get(&op.root_id)
                .ok_or(Error::MissingDependency(op.root_id))?;
            if root.action != Action::Create || root.content_id != op.content_id {
                return Err(Error::RootCollision);
            }
            if op.basis.is_empty() {
                return Err(Error::InvalidBasis);
            }
        }
        let scope = self.closure(&op.basis)?;
        if scope.iter().any(|id| {
            let parent = &self.updates[id];
            parent.content_id != op.content_id || parent.root_id != op.root_id
        }) {
            return Err(Error::InvalidBasis);
        }
        let heads = self.project(&scope);
        if heads.life != op.basis {
            return Err(Error::InvalidBasis);
        }
        let live = op.basis.iter().any(|id| self.updates[id].action.live());
        let deleted = op.basis.iter().any(|id| !self.updates[id].action.live());
        if match op.action {
            Action::Edit => !live,
            Action::Keep => !(live && deleted) || !op.changes.is_empty(),
            Action::Restore => live || !deleted,
            Action::Delete => !op.changes.is_empty(),
            Action::Create => false,
        } {
            return Err(Error::InvalidAction);
        }
        for (field, change) in &op.changes {
            if !change.value.matches(field) {
                return Err(Error::InvalidValue(field.clone()));
            }
            if change
                .replaces
                .iter()
                .any(|id| !scope.contains(id) || !self.updates[id].changes.contains_key(field))
            {
                return Err(Error::InvalidReplacement(field.clone()));
            }
        }
        if op.action == Action::Restore {
            if !op.changes.contains_key(&Field::Body)
                || heads.fields.keys().any(|f| !op.changes.contains_key(f))
            {
                return Err(Error::IncompleteRestore);
            }
            for (field, change) in &op.changes {
                if heads.fields.get(field).cloned().unwrap_or_default() != change.replaces {
                    return Err(Error::IncompleteRestore);
                }
            }
        }
        // Mutation only after every check succeeds.
        self.content_updates
            .entry(op.content_id)
            .or_default()
            .insert(op.update_id);
        self.updates.insert(op.update_id, op);
        Ok(Acceptance::Inserted)
    }

    /// Expand a complete restoration against its fixed basis, never current heads.
    pub fn restore_changes(
        &self,
        basis: &BTreeSet<UpdateId>,
        supplied: BTreeMap<Field, Value<V>>,
    ) -> Result<BTreeMap<Field, Change<V>>, Error> {
        if !supplied.contains_key(&Field::Body) {
            return Err(Error::IncompleteRestore);
        }
        let scope = self.closure(basis)?;
        let heads = self.project(&scope);
        if basis.is_empty() || heads.life != *basis {
            return Err(Error::InvalidBasis);
        }
        let first = &self.updates[basis.first().unwrap()];
        if basis.iter().any(|id| {
            self.updates[id].content_id != first.content_id || self.updates[id].action.live()
        }) {
            return Err(Error::InvalidAction);
        }
        let mut result = BTreeMap::new();
        for (field, value) in supplied {
            if !value.matches(&field) {
                return Err(Error::InvalidValue(field));
            }
            let replaces = heads.fields.get(&field).cloned().unwrap_or_default();
            result.insert(field, Change { value, replaces });
        }
        for (field, replaces) in heads.fields {
            result.entry(field.clone()).or_insert_with(|| Change {
                value: match field {
                    Field::Kind(_) => Value::Kind(false),
                    _ => Value::Absent,
                },
                replaces,
            });
        }
        Ok(result)
    }
}
