//! Logical App save contract and canonical request identity. These are separate
//! from immutable update records: callers never supply a causal basis directly.
use crate::{
    CodecError,
    record::{Decoder, Encoder},
};
use harmion_domain::{Change, ContentId, Field, UpdateId, Value};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct AppId(pub [u8; 32]);
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RequestId(pub [u8; 32]);
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct EditToken(pub [u8; 32]);
impl std::fmt::Debug for EditToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EditToken([redacted])")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Create {
        fields: BTreeMap<Field, Value>,
    },
    Edit {
        token: EditToken,
        fields: BTreeMap<Field, Value>,
    },
    Resolve {
        token: EditToken,
        changes: BTreeMap<Field, Change>,
    },
    Delete {
        token: EditToken,
    },
    Keep {
        token: EditToken,
    },
    Restore {
        token: EditToken,
        fields: BTreeMap<Field, Value>,
    },
    Select {
        token: EditToken,
        selections: BTreeMap<Field, FieldSelection>,
    },
    RestoreFrom {
        token: EditToken,
        body: String,
        fields: BTreeMap<Field, UpdateId>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldSelection {
    pub version: UpdateId,
    pub replaces: BTreeSet<UpdateId>,
}

impl Command {
    pub fn token(&self) -> Option<EditToken> {
        match self {
            Self::Create { .. } => None,
            Self::Edit { token, .. }
            | Self::Resolve { token, .. }
            | Self::Delete { token }
            | Self::Keep { token }
            | Self::Select { token, .. }
            | Self::RestoreFrom { token, .. }
            | Self::Restore { token, .. } => Some(*token),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveRequest {
    pub request_id: RequestId,
    pub content_id: ContentId,
    pub command: Command,
}

/// HRQ-v1 identity includes the fixed token and the exact explicit change set.
/// Equal-valued numbers normalize as in record v1; generated IDs/time are absent.
pub fn encode_save_request(request: &SaveRequest) -> Result<Vec<u8>, CodecError> {
    let mut e = Encoder::new();
    e.raw(b"HRQ\0")?;
    e.raw(&1u32.to_be_bytes())?;
    e.raw(&request.request_id.0)?;
    e.raw(&request.content_id.0)?;
    e.tag(match request.command {
        Command::Create { .. } => 0,
        Command::Edit { .. } => 1,
        Command::Resolve { .. } => 2,
        Command::Delete { .. } => 3,
        Command::Keep { .. } => 4,
        Command::Restore { .. } => 5,
        Command::Select { .. } => 6,
        Command::RestoreFrom { .. } => 7,
    })?;
    if let Some(token) = request.command.token() {
        e.raw(&token.0)?;
    }
    match &request.command {
        Command::Create { fields }
        | Command::Edit { fields, .. }
        | Command::Restore { fields, .. } => {
            e.count(fields.len())?;
            for (field, value) in fields {
                e.field(field)?;
                e.value(field, value)?;
            }
        }
        Command::Resolve { changes, .. } => {
            e.count(changes.len())?;
            for (field, change) in changes {
                e.field(field)?;
                e.value(field, &change.value)?;
                e.ids(&change.replaces)?;
            }
        }
        Command::Select { selections, .. } => {
            e.count(selections.len())?;
            for (field, selection) in selections {
                e.field(field)?;
                e.raw(&selection.version.0)?;
                e.ids(&selection.replaces)?;
            }
        }
        Command::RestoreFrom { body, fields, .. } => {
            e.string(body)?;
            e.count(fields.len())?;
            for (field, version) in fields {
                if *field == Field::Body {
                    return Err(CodecError::InvalidFieldValue);
                }
                e.field(field)?;
                e.raw(&version.0)?;
            }
        }
        Command::Delete { .. } | Command::Keep { .. } => (),
    }
    Ok(e.finish())
}

pub fn decode_save_request(bytes: &[u8]) -> Result<SaveRequest, CodecError> {
    let mut d = Decoder::new(bytes)?;
    if d.raw(4)? != b"HRQ\0" {
        return Err(CodecError::BadMagic);
    }
    let version = d.u32()?;
    if version != 1 {
        return Err(CodecError::UnsupportedVersion(version));
    }
    let request_id = RequestId(d.fixed()?);
    let content_id = ContentId(d.fixed()?);
    let tag = d.tag()?;
    if tag > 7 {
        return Err(CodecError::InvalidTag);
    }
    let token = if tag != 0 {
        Some(EditToken(d.fixed()?))
    } else {
        None
    };
    let command = match tag {
        0 | 1 | 5 => {
            let mut fields = BTreeMap::new();
            for _ in 0..d.count()? {
                if fields.insert(d.field()?, d.value()?).is_some() {
                    return Err(CodecError::Duplicate);
                }
            }
            match tag {
                0 => Command::Create { fields },
                1 => Command::Edit {
                    token: token.unwrap(),
                    fields,
                },
                _ => Command::Restore {
                    token: token.unwrap(),
                    fields,
                },
            }
        }
        2 => {
            let mut changes = BTreeMap::new();
            for _ in 0..d.count()? {
                let field = d.field()?;
                let change = Change {
                    value: d.value()?,
                    replaces: d.ids()?,
                };
                if changes.insert(field, change).is_some() {
                    return Err(CodecError::Duplicate);
                }
            }
            Command::Resolve {
                token: token.unwrap(),
                changes,
            }
        }
        3 => Command::Delete {
            token: token.unwrap(),
        },
        4 => Command::Keep {
            token: token.unwrap(),
        },
        6 => {
            let mut selections = BTreeMap::new();
            for _ in 0..d.count()? {
                let field = d.field()?;
                let selection = FieldSelection {
                    version: UpdateId(d.fixed()?),
                    replaces: d.ids()?,
                };
                if selections.insert(field, selection).is_some() {
                    return Err(CodecError::Duplicate);
                }
            }
            Command::Select {
                token: token.unwrap(),
                selections,
            }
        }
        _ => {
            let body = d.string()?;
            let mut fields = BTreeMap::new();
            for _ in 0..d.count()? {
                if fields.insert(d.field()?, UpdateId(d.fixed()?)).is_some() {
                    return Err(CodecError::Duplicate);
                }
            }
            Command::RestoreFrom {
                token: token.unwrap(),
                body,
                fields,
            }
        }
    };
    d.finish()?;
    let request = SaveRequest {
        request_id,
        content_id,
        command,
    };
    if encode_save_request(&request)? != bytes {
        return Err(CodecError::NonCanonical);
    }
    Ok(request)
}
