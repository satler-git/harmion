use crate::{Error, storage::Transaction};
use harmion_domain::Content;
use harmion_protocol::AppId;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Filter {
    All,
    Kinds(BTreeSet<String>),
}
impl Filter {
    pub fn matches(&self, content: &Content) -> bool {
        match self {
            Self::All => true,
            Self::Kinds(kinds) => !kinds.is_disjoint(&content.kinds),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Grants {
    pub read: Option<Filter>,
    pub write: Option<Filter>,
    pub manage_apps: bool,
    pub manage_nodes: bool,
}
impl Grants {
    pub fn administrator() -> Self {
        Self {
            read: Some(Filter::All),
            write: Some(Filter::All),
            manage_apps: true,
            manage_nodes: true,
        }
    }
    pub fn is_administrator(&self) -> bool {
        *self == Self::administrator()
    }
    pub fn can_read(&self, content: &Content) -> bool {
        self.read.as_ref().is_some_and(|f| f.matches(content))
    }
    pub fn can_write(&self, content: &Content) -> bool {
        self.write.as_ref().is_some_and(|f| f.matches(content))
    }
}

/// High-entropy bearer secret supplied only by an authenticated transport.
/// Redacted Debug prevents accidental inclusion in diagnostic messages.
#[derive(Clone, PartialEq, Eq)]
pub struct Credential([u8; 32]);
impl Credential {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
    pub fn expose_bytes(&self) -> &[u8; 32] {
        &self.0
    }
    pub fn verifier(&self) -> [u8; 32] {
        Sha256::new()
            .chain_update(b"harmion:app-credential:v1\0")
            .chain_update(self.0)
            .finalize()
            .into()
    }
}
impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Credential([redacted])")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct App {
    pub id: AppId,
    pub verifier: [u8; 32],
    pub expires_at: i64,
    pub revoked: bool,
    pub grants: Grants,
}

/// Constructed only by authentication inside the same transaction as the use case.
pub(crate) struct Principal {
    pub app: App,
}
pub(crate) fn authenticate(
    tx: &mut dyn Transaction,
    credential: &Credential,
    now: i64,
) -> Result<Principal, Error> {
    let app = tx
        .app_by_verifier(credential.verifier())?
        .ok_or(Error::Unauthenticated)?;
    if app.revoked || now >= app.expires_at {
        return Err(Error::Unauthenticated);
    }
    Ok(Principal { app })
}

pub fn permissions(
    store: &mut impl crate::storage::Store,
    credential: &Credential,
    now: i64,
) -> Result<Grants, Error> {
    store.transaction(|tx| Ok(authenticate(tx, credential, now)?.app.grants))
}
