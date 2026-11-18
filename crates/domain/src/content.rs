use std::collections::{BTreeMap, BTreeSet};

use crate::{AttributeValue, ContentId, Field, Life, UpdateId, Value};

/// Namespace and key remain separate, even when either contains a slash.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct AttributeKey {
    pub namespace: String,
    pub key: String,
}

/// Materialized normal values. Does not contain a token or grant access.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Content<V = AttributeValue> {
    pub id: ContentId,
    pub body: String,
    pub attributes: BTreeMap<AttributeKey, V>,
    pub kinds: BTreeSet<String>,
}

/// Equal-valued heads are grouped without discarding any causal IDs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate<V = AttributeValue> {
    pub value: Value<V>,
    pub versions: BTreeSet<UpdateId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldState<V = AttributeValue> {
    pub selected: UpdateId,
    pub candidates: Vec<Candidate<V>>,
}

impl<V> FieldState<V> {
    pub fn is_conflicted(&self) -> bool {
        self.candidates.len() > 1
    }

    /// Parents for editing the displayed value, including equal-valued heads.
    pub fn selected_candidate(&self) -> &Candidate<V> {
        self.candidates
            .iter()
            .find(|c| c.versions.contains(&self.selected))
            .expect("selected version belongs to a candidate")
    }
}

/// Internal owned snapshot, including deleted data and non-public kind conflicts.
/// Node/application must authorize and redact this before constructing an App DTO.
/// `basis` and field parents belong in an opaque, node-local edit token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentSnapshot<V = AttributeValue> {
    pub content: Content<V>,
    pub life: Life,
    pub basis: BTreeSet<UpdateId>,
    pub fields: BTreeMap<Field, FieldState<V>>,
}

impl<V> ContentSnapshot<V> {
    /// Deleted data is retained internally; deletion conflicts stay visible.
    pub fn visible_content(&self) -> Option<&Content<V>> {
        (self.life != Life::Deleted).then_some(&self.content)
    }

    /// Includes explicit absent/false heads, though omitted from normal values.
    /// Only fields actually changed by a later edit should use these parents.
    pub fn edit_parents(&self) -> BTreeMap<Field, BTreeSet<UpdateId>> {
        self.fields
            .iter()
            .map(|(field, state)| (field.clone(), state.selected_candidate().versions.clone()))
            .collect()
    }
}
