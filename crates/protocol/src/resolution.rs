use crate::{
    CodecError, EditToken, FieldSelection,
    record::{Decoder, Encoder},
    unhex,
};
use harmion_domain::{ContentId, Field, UpdateId};
use std::collections::BTreeMap;

#[derive(serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SelectionField {
    Body,
    Attribute { namespace: String, key: String },
    Kind { name: String },
}

impl From<SelectionField> for Field {
    fn from(field: SelectionField) -> Self {
        match field {
            SelectionField::Body => Field::Body,
            SelectionField::Attribute { namespace, key } => Field::Attribute { namespace, key },
            SelectionField::Kind { name } => Field::Kind(name),
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectionSpec {
    pub field: SelectionField,
    pub version: String,
    pub replaces: Vec<String>,
}

pub fn parse_selections(
    specs: Vec<SelectionSpec>,
) -> Result<BTreeMap<Field, FieldSelection>, CodecError> {
    let mut selections = BTreeMap::new();
    for spec in specs {
        let field = spec.field.into();
        let selection = FieldSelection {
            version: UpdateId(unhex(&spec.version)?),
            replaces: spec
                .replaces
                .iter()
                .map(|id| unhex(id).map(UpdateId))
                .collect::<Result<_, _>>()?,
        };
        if selections.insert(field, selection).is_some() {
            return Err(CodecError::Duplicate);
        }
    }
    Ok(selections)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergePart {
    Base(Option<UpdateId>),
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MergeInputRequest {
    pub content: ContentId,
    pub token: EditToken,
    pub left: UpdateId,
    pub right: UpdateId,
    pub part: MergePart,
}

impl MergeInputRequest {
    pub fn encode(self) -> Result<Vec<u8>, CodecError> {
        let mut encoder = Encoder::new();
        encoder.raw(&self.content.0)?;
        encoder.raw(&self.token.0)?;
        encoder.raw(&self.left.0)?;
        encoder.raw(&self.right.0)?;
        match self.part {
            MergePart::Base(None) => encoder.tag(0)?,
            MergePart::Base(Some(version)) => {
                encoder.tag(1)?;
                encoder.raw(&version.0)?;
            }
            MergePart::Left => encoder.tag(2)?,
            MergePart::Right => encoder.tag(3)?,
        }
        Ok(encoder.finish())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        let mut decoder = Decoder::new(bytes)?;
        let request = Self {
            content: ContentId(decoder.fixed()?),
            token: EditToken(decoder.fixed()?),
            left: UpdateId(decoder.fixed()?),
            right: UpdateId(decoder.fixed()?),
            part: match decoder.tag()? {
                0 => MergePart::Base(None),
                1 => MergePart::Base(Some(UpdateId(decoder.fixed()?))),
                2 => MergePart::Left,
                3 => MergePart::Right,
                _ => return Err(CodecError::InvalidTag),
            },
        };
        decoder.finish()?;
        Ok(request)
    }
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MergeInput {
    Body { version: String, text: String },
    ChooseBase { versions: Vec<String> },
    NoBase,
}

#[derive(Debug, serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MergePlan {
    Proposed {
        base_version: String,
        text: String,
        has_conflicts: bool,
    },
    ChooseBase {
        versions: Vec<String>,
    },
    NoBase,
}
