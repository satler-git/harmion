//! Canonical signed membership records. Decoding is not authorization.
use crate::{
    CodecError,
    record::{Decoder, Encoder},
};
use harmion_domain::{ClusterId, NodeId};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub const MAX_CERTIFICATE_BYTES: usize = 128 * 1024;
pub type CertificateId = [u8; 32];

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Binding {
    Https(Vec<u8>),
    Iroh([u8; 32]),
    /// HMC v2 only. Carries the exact node signing public key, which must equal
    /// `Member.signing_key`. Binding uniqueness compares this key, never just the
    /// transport discriminant.
    SignedHttps([u8; 32]),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub node: NodeId,
    pub signing_key: [u8; 32],
    pub binding: Binding,
    pub endpoint: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Admit(Member),
    /// Cut is a fixed set of records issued by target that remain authorized.
    /// It includes retained admissions and previously confirmed retirements.
    Retire {
        target: NodeId,
        retained: BTreeSet<CertificateId>,
    },
    Dismiss {
        retirements: BTreeSet<CertificateId>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Statement {
    pub cluster: ClusterId,
    pub issuer: NodeId,
    pub signing_key: [u8; 32],
    /// A fresh nonce permits explicit reapproval of an identical member.
    pub nonce: [u8; 32],
    pub action: Action,
}

impl Statement {
    /// HMC v2 is used only for an Admit carrying the explicit `SignedHttps`
    /// binding. Retire and Dismiss keep byte-identical v1 encodings; the version
    /// is not a timestamp and carries no ordering meaning.
    pub fn version(&self) -> u32 {
        match &self.action {
            Action::Admit(member) if matches!(member.binding, Binding::SignedHttps(_)) => 2,
            _ => 1,
        }
    }

    pub fn signing_bytes(&self) -> Result<Vec<u8>, CodecError> {
        let mut e = Encoder::with_limit(MAX_CERTIFICATE_BYTES - 64);
        e.raw(b"HMC\0")?;
        e.raw(&self.version().to_be_bytes())?;
        e.raw(&self.cluster.0)?;
        e.raw(&self.issuer.0)?;
        e.raw(&self.signing_key)?;
        e.raw(&self.nonce)?;
        match &self.action {
            Action::Dismiss { retirements } => {
                if retirements.is_empty() {
                    return Err(CodecError::NonCanonical);
                }
                e.tag(2)?;
                e.count(retirements.len())?;
                for id in retirements {
                    e.raw(id)?;
                }
            }
            Action::Admit(member) => {
                e.tag(0)?;
                e.raw(&member.node.0)?;
                e.raw(&member.signing_key)?;
                match &member.binding {
                    Binding::Https(bytes) => {
                        if bytes.is_empty() || bytes.len() > 65536 {
                            return Err(CodecError::TooLarge);
                        }
                        e.tag(0)?;
                        e.raw(&(bytes.len() as u32).to_be_bytes())?;
                        e.raw(bytes)?;
                    }
                    Binding::Iroh(key) => {
                        e.tag(1)?;
                        e.raw(key)?;
                    }
                    Binding::SignedHttps(key) => {
                        // The typed binding repeats the membership signing key
                        // explicitly; it is not a second requester-chosen credential.
                        if *key != member.signing_key {
                            return Err(CodecError::InvalidFieldValue);
                        }
                        e.tag(2)?;
                        e.raw(key)?;
                    }
                }
                if member.endpoint.len() > 2048 {
                    return Err(CodecError::TooLarge);
                }
                e.raw(&(member.endpoint.len() as u32).to_be_bytes())?;
                e.raw(member.endpoint.as_bytes())?;
            }
            Action::Retire { target, retained } => {
                e.tag(1)?;
                e.raw(&target.0)?;
                e.count(retained.len())?;
                for id in retained {
                    e.raw(id)?;
                }
            }
        }
        Ok(e.finish())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Certificate {
    statement: Statement,
    signature: [u8; 64],
    bytes: Vec<u8>,
    id: CertificateId,
}
impl Certificate {
    pub fn new(statement: Statement, signature: [u8; 64]) -> Result<Self, CodecError> {
        let mut bytes = statement.signing_bytes()?;
        bytes.extend_from_slice(&signature);
        Ok(Self {
            id: Sha256::digest(&bytes).into(),
            statement,
            signature,
            bytes,
        })
    }
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CodecError> {
        let mut d = Decoder::with_limit(bytes, MAX_CERTIFICATE_BYTES)?;
        if d.raw(4)? != b"HMC\0" {
            return Err(CodecError::BadMagic);
        }
        let version = d.u32()?;
        if version != 1 && version != 2 {
            return Err(CodecError::UnsupportedVersion(version));
        }
        let cluster = ClusterId(d.fixed()?);
        let issuer = NodeId(d.fixed()?);
        let signing_key = d.fixed()?;
        let nonce = d.fixed()?;
        let action = match d.tag()? {
            2 => {
                let count = d.count()?;
                if count == 0 {
                    return Err(CodecError::NonCanonical);
                }
                let mut retirements = BTreeSet::new();
                for _ in 0..count {
                    let id = d.fixed()?;
                    if retirements.last().is_some_and(|previous| previous >= &id) {
                        return Err(CodecError::NonCanonical);
                    }
                    retirements.insert(id);
                }
                Action::Dismiss { retirements }
            }
            0 => {
                let node = NodeId(d.fixed()?);
                let signing_key = d.fixed()?;
                let binding = match (version, d.tag()?) {
                    (1, 0) => {
                        let len = d.u32()? as usize;
                        if len == 0 || len > 65536 {
                            return Err(CodecError::TooLarge);
                        }
                        Binding::Https(d.raw(len)?.to_vec())
                    }
                    (1, 1) => Binding::Iroh(d.fixed()?),
                    (2, 2) => {
                        let key = d.fixed()?;
                        if key != signing_key {
                            return Err(CodecError::InvalidFieldValue);
                        }
                        Binding::SignedHttps(key)
                    }
                    _ => return Err(CodecError::InvalidTag),
                };
                let len = d.u32()? as usize;
                if len > 2048 {
                    return Err(CodecError::TooLarge);
                }
                let endpoint = std::str::from_utf8(d.raw(len)?)
                    .map_err(|_| CodecError::InvalidUtf8)?
                    .to_owned();
                Action::Admit(Member {
                    node,
                    signing_key,
                    binding,
                    endpoint,
                })
            }
            1 => {
                let target = NodeId(d.fixed()?);
                let count = d.count()?;
                let mut retained = BTreeSet::new();
                for _ in 0..count {
                    let id = d.fixed()?;
                    if retained.last().is_some_and(|previous| previous >= &id) {
                        return Err(CodecError::NonCanonical);
                    }
                    retained.insert(id);
                }
                Action::Retire { target, retained }
            }
            _ => return Err(CodecError::InvalidTag),
        };
        let signature = d.fixed()?;
        d.finish()?;
        let result = Self::new(
            Statement {
                cluster,
                issuer,
                signing_key,
                nonce,
                action,
            },
            signature,
        )?;
        if result.bytes != bytes {
            return Err(CodecError::NonCanonical);
        }
        Ok(result)
    }
    pub fn statement(&self) -> &Statement {
        &self.statement
    }
    pub fn signature(&self) -> &[u8; 64] {
        &self.signature
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn id(&self) -> CertificateId {
        self.id
    }
    pub fn signed_bytes(&self) -> &[u8] {
        &self.bytes[..self.bytes.len() - 64]
    }
}
