use harmion_domain::*;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_RECORD_BYTES: usize = 1024 * 1024;
pub const MAX_DEPTH: usize = 64;
pub const MAX_ITEMS: usize = 32_768;
const MAGIC: &[u8; 4] = b"HUP\0";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodecError {
    InvalidJson,
    InvalidHex,
    TooLarge,
    TooDeep,
    TooManyItems,
    Truncated,
    BadMagic,
    UnsupportedVersion(u32),
    InvalidTag,
    InvalidUtf8,
    InvalidNumber,
    InvalidFieldValue,
    Duplicate,
    NonCanonical,
    TrailingBytes,
}
impl std::fmt::Display for CodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for CodecError {}

/// Preserves canonical bytes for storage and relay. Digest is not a signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    update: Update,
    bytes: Vec<u8>,
    digest: [u8; 32],
}
impl Record {
    pub fn from_update(update: Update) -> Result<Self, CodecError> {
        let bytes = encode(&update)?;
        Ok(Self {
            digest: Sha256::digest(&bytes).into(),
            bytes,
            update,
        })
    }
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CodecError> {
        let update = decode(bytes)?;
        Ok(Self {
            digest: Sha256::digest(bytes).into(),
            bytes: bytes.to_vec(),
            update,
        })
    }
    pub fn update(&self) -> &Update {
        &self.update
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn digest(&self) -> &[u8; 32] {
        &self.digest
    }
}

pub(crate) struct Encoder {
    bytes: Vec<u8>,
    items: usize,
    limit: usize,
}
impl Encoder {
    pub(crate) fn new() -> Self {
        Self {
            bytes: Vec::new(),
            items: 0,
            limit: MAX_RECORD_BYTES,
        }
    }
    pub(crate) fn finish(self) -> Vec<u8> {
        self.bytes
    }
    pub(crate) fn with_limit(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            items: 0,
            limit,
        }
    }

    pub(crate) fn raw(&mut self, bytes: &[u8]) -> Result<(), CodecError> {
        if bytes.len() > self.limit - self.bytes.len() {
            return Err(CodecError::TooLarge);
        }
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }
    pub(crate) fn tag(&mut self, tag: u8) -> Result<(), CodecError> {
        self.raw(&[tag])
    }
    pub(crate) fn count(&mut self, count: usize) -> Result<(), CodecError> {
        if count > MAX_ITEMS - self.items {
            return Err(CodecError::TooManyItems);
        }
        self.items += count;
        self.raw(&(count as u32).to_be_bytes())
    }
    fn string(&mut self, text: &str) -> Result<(), CodecError> {
        if text.len() > MAX_RECORD_BYTES {
            return Err(CodecError::TooLarge);
        }
        self.raw(&(text.len() as u32).to_be_bytes())?;
        self.raw(text.as_bytes())
    }
    pub(crate) fn ids(&mut self, ids: &BTreeSet<UpdateId>) -> Result<(), CodecError> {
        self.count(ids.len())?;
        for id in ids {
            self.raw(&id.0)?;
        }
        Ok(())
    }
    pub(crate) fn field(&mut self, field: &Field) -> Result<(), CodecError> {
        match field {
            Field::Body => self.tag(0),
            Field::Attribute { namespace, key } => {
                self.tag(1)?;
                self.string(namespace)?;
                self.string(key)
            }
            Field::Kind(name) => {
                self.tag(2)?;
                self.string(name)
            }
        }
    }
    fn attribute(&mut self, value: &AttributeValue, depth: usize) -> Result<(), CodecError> {
        if depth > MAX_DEPTH {
            return Err(CodecError::TooDeep);
        }
        match value {
            AttributeValue::Null => self.tag(0),
            AttributeValue::Bool(false) => self.tag(1),
            AttributeValue::Bool(true) => self.tag(2),
            AttributeValue::Number(n) => {
                self.tag(3)?;
                self.string(&n.to_string())
            }
            AttributeValue::String(s) => {
                self.tag(4)?;
                self.string(s)
            }
            AttributeValue::Array(values) => {
                self.tag(5)?;
                self.count(values.len())?;
                for value in values {
                    self.attribute(value, depth + 1)?;
                }
                Ok(())
            }
            AttributeValue::Object(values) => {
                self.tag(6)?;
                self.count(values.len())?;
                for (key, value) in values {
                    self.string(key)?;
                    self.attribute(value, depth + 1)?;
                }
                Ok(())
            }
        }
    }
    pub(crate) fn value(&mut self, field: &Field, value: &Value) -> Result<(), CodecError> {
        match (field, value) {
            (Field::Body, Value::Body(text)) => {
                self.tag(0)?;
                self.string(text)
            }
            (Field::Attribute { .. }, Value::Attribute(value)) => {
                self.tag(1)?;
                self.attribute(value, 0)
            }
            (Field::Attribute { .. }, Value::Absent) => self.tag(2),
            (Field::Kind(_), Value::Kind(false)) => self.tag(3),
            (Field::Kind(_), Value::Kind(true)) => self.tag(4),
            _ => Err(CodecError::InvalidFieldValue),
        }
    }
}

pub fn encode(update: &Update) -> Result<Vec<u8>, CodecError> {
    if update.format_version != 1 {
        return Err(CodecError::UnsupportedVersion(update.format_version));
    }
    let mut e = Encoder {
        bytes: Vec::new(),
        items: 0,
        limit: MAX_RECORD_BYTES,
    };
    e.raw(MAGIC)?;
    e.raw(&update.format_version.to_be_bytes())?;
    for id in [
        &update.cluster_id.0,
        &update.content_id.0,
        &update.root_id.0,
        &update.update_id.0,
        &update.author_node_id.0,
    ] {
        e.raw(id)?;
    }
    e.raw(&update.edited_at.to_be_bytes())?;
    e.tag(match update.action {
        Action::Create => 0,
        Action::Edit => 1,
        Action::Delete => 2,
        Action::Keep => 3,
        Action::Restore => 4,
    })?;
    e.ids(&update.basis)?;
    e.count(update.changes.len())?;
    for (field, change) in &update.changes {
        e.field(field)?;
        e.value(field, &change.value)?;
        e.ids(&change.replaces)?;
    }
    Ok(e.bytes)
}

pub(crate) struct Decoder<'a> {
    remaining: &'a [u8],
    items: usize,
}
impl<'a> Decoder<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Result<Self, CodecError> {
        Self::with_limit(bytes, MAX_RECORD_BYTES)
    }
    pub(crate) fn with_limit(bytes: &'a [u8], limit: usize) -> Result<Self, CodecError> {
        if bytes.len() > limit {
            return Err(CodecError::TooLarge);
        }
        Ok(Self {
            remaining: bytes,
            items: 0,
        })
    }
    pub(crate) fn finish(self) -> Result<(), CodecError> {
        if self.remaining.is_empty() {
            Ok(())
        } else {
            Err(CodecError::TrailingBytes)
        }
    }

    pub(crate) fn raw(&mut self, len: usize) -> Result<&'a [u8], CodecError> {
        if len > self.remaining.len() {
            return Err(CodecError::Truncated);
        }
        let (bytes, rest) = self.remaining.split_at(len);
        self.remaining = rest;
        Ok(bytes)
    }
    pub(crate) fn fixed<const N: usize>(&mut self) -> Result<[u8; N], CodecError> {
        Ok(self.raw(N)?.try_into().expect("fixed length slice"))
    }
    pub(crate) fn tag(&mut self) -> Result<u8, CodecError> {
        Ok(self.fixed::<1>()?[0])
    }
    pub(crate) fn u32(&mut self) -> Result<u32, CodecError> {
        Ok(u32::from_be_bytes(self.fixed()?))
    }
    pub(crate) fn count(&mut self) -> Result<usize, CodecError> {
        let count = self.u32()? as usize;
        if count > MAX_ITEMS - self.items {
            return Err(CodecError::TooManyItems);
        }
        self.items += count;
        Ok(count)
    }
    fn string(&mut self) -> Result<String, CodecError> {
        let len = self.u32()? as usize;
        Ok(std::str::from_utf8(self.raw(len)?)
            .map_err(|_| CodecError::InvalidUtf8)?
            .into())
    }
    pub(crate) fn ids(&mut self) -> Result<BTreeSet<UpdateId>, CodecError> {
        let mut ids = BTreeSet::new();
        for _ in 0..self.count()? {
            if !ids.insert(UpdateId(self.fixed()?)) {
                return Err(CodecError::Duplicate);
            }
        }
        Ok(ids)
    }
    pub(crate) fn field(&mut self) -> Result<Field, CodecError> {
        match self.tag()? {
            0 => Ok(Field::Body),
            1 => Ok(Field::Attribute {
                namespace: self.string()?,
                key: self.string()?,
            }),
            2 => Ok(Field::Kind(self.string()?)),
            _ => Err(CodecError::InvalidTag),
        }
    }
    fn attribute(&mut self, depth: usize) -> Result<AttributeValue, CodecError> {
        if depth > MAX_DEPTH {
            return Err(CodecError::TooDeep);
        }
        Ok(match self.tag()? {
            0 => AttributeValue::Null,
            1 => AttributeValue::Bool(false),
            2 => AttributeValue::Bool(true),
            3 => AttributeValue::Number(
                self.string()?
                    .parse()
                    .map_err(|_| CodecError::InvalidNumber)?,
            ),
            4 => AttributeValue::String(self.string()?),
            5 => {
                let mut values = Vec::new();
                for _ in 0..self.count()? {
                    values.push(self.attribute(depth + 1)?);
                }
                AttributeValue::Array(values)
            }
            6 => {
                let mut values = BTreeMap::new();
                for _ in 0..self.count()? {
                    let key = self.string()?;
                    let value = self.attribute(depth + 1)?;
                    if values.insert(key, value).is_some() {
                        return Err(CodecError::Duplicate);
                    }
                }
                AttributeValue::Object(values)
            }
            _ => return Err(CodecError::InvalidTag),
        })
    }
    pub(crate) fn value(&mut self) -> Result<Value, CodecError> {
        match self.tag()? {
            0 => Ok(Value::Body(self.string()?)),
            1 => Ok(Value::Attribute(self.attribute(0)?)),
            2 => Ok(Value::Absent),
            3 => Ok(Value::Kind(false)),
            4 => Ok(Value::Kind(true)),
            _ => Err(CodecError::InvalidTag),
        }
    }
}

pub fn decode(bytes: &[u8]) -> Result<Update, CodecError> {
    if bytes.len() > MAX_RECORD_BYTES {
        return Err(CodecError::TooLarge);
    }
    let mut d = Decoder {
        remaining: bytes,
        items: 0,
    };
    if d.raw(4)? != MAGIC {
        return Err(CodecError::BadMagic);
    }
    let format_version = d.u32()?;
    if format_version != 1 {
        return Err(CodecError::UnsupportedVersion(format_version));
    }
    let cluster_id = ClusterId(d.fixed()?);
    let content_id = ContentId(d.fixed()?);
    let root_id = UpdateId(d.fixed()?);
    let update_id = UpdateId(d.fixed()?);
    let author_node_id = NodeId(d.fixed()?);
    let edited_at = i64::from_be_bytes(d.fixed()?);
    let action = match d.tag()? {
        0 => Action::Create,
        1 => Action::Edit,
        2 => Action::Delete,
        3 => Action::Keep,
        4 => Action::Restore,
        _ => return Err(CodecError::InvalidTag),
    };
    let basis = d.ids()?;
    let mut changes = BTreeMap::new();
    for _ in 0..d.count()? {
        let field = d.field()?;
        let value = d.value()?;
        let replaces = d.ids()?;
        if changes.insert(field, Change { value, replaces }).is_some() {
            return Err(CodecError::Duplicate);
        }
    }
    if !d.remaining.is_empty() {
        return Err(CodecError::TrailingBytes);
    }
    let update = Update {
        format_version,
        cluster_id,
        content_id,
        root_id,
        update_id,
        author_node_id,
        edited_at,
        action,
        basis,
        changes,
    };
    // One representation per value: also rejects unsorted collections and decimal aliases.
    if encode(&update)? != bytes {
        return Err(CodecError::NonCanonical);
    }
    Ok(update)
}
