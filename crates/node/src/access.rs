use crate::{Error, storage::Transaction};
use harmion_domain::{Content, ContentSnapshot};
use harmion_protocol::{AppId, FilterSpec, GrantSpec};
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
    pub call: bool,
    pub provide: bool,
    /// Names explicitly pre-approved for Provide (OAuth `provide:NAME`).
    /// Empty means the grant is not name-restricted.
    pub provide_names: BTreeSet<String>,
    pub manage_apps: bool,
    pub manage_nodes: bool,
}
impl Grants {
    pub fn administrator() -> Self {
        Self {
            read: Some(Filter::All),
            write: Some(Filter::All),
            call: true,
            provide: true,
            provide_names: BTreeSet::new(),
            manage_apps: true,
            manage_nodes: true,
        }
    }
    pub fn is_administrator(&self) -> bool {
        // Interpreter grants are independent of the owner-management baseline.
        // Existing administrator rows migrated from earlier schemas must keep
        // their kind-conflict and restore visibility.
        self.read == Some(Filter::All)
            && self.write == Some(Filter::All)
            && self.manage_apps
            && self.manage_nodes
    }
    pub fn can_read(&self, content: &Content) -> bool {
        self.read.as_ref().is_some_and(|f| f.matches(content))
    }
    pub(crate) fn can_read_snapshot(&self, snapshot: &ContentSnapshot) -> bool {
        snapshot
            .visible_content()
            .is_some_and(|content| self.can_read(content))
    }
    pub fn can_write(&self, content: &Content) -> bool {
        self.write.as_ref().is_some_and(|f| f.matches(content))
    }
    pub fn can_call(&self) -> bool {
        self.call
    }
    pub fn can_provide(&self) -> bool {
        self.provide
    }
    pub fn is_provider_name_preapproved(&self, name: &str) -> bool {
        self.provide && self.provide_names.contains(name)
    }
    pub fn allows_provider_name(&self, name: &str) -> bool {
        self.provide && (self.provide_names.is_empty() || self.provide_names.contains(name))
    }
}

impl From<GrantSpec> for Grants {
    fn from(spec: GrantSpec) -> Self {
        let filter = |value| match value {
            FilterSpec::All => Filter::All,
            FilterSpec::Kinds { kinds } => Filter::Kinds(kinds.into_iter().collect()),
        };
        Self {
            read: spec.read.map(filter),
            write: spec.write.map(filter),
            call: spec.call,
            provide: spec.provide,
            provide_names: spec.provide_names.into_iter().collect(),
            manage_apps: spec.manage_apps,
            manage_nodes: spec.manage_nodes,
        }
    }
}

impl From<Grants> for GrantSpec {
    fn from(grants: Grants) -> Self {
        let filter = |value| match value {
            Filter::All => FilterSpec::All,
            Filter::Kinds(kinds) => FilterSpec::Kinds {
                kinds: kinds.into_iter().collect(),
            },
        };
        Self {
            read: grants.read.map(filter),
            write: grants.write.map(filter),
            call: grants.call,
            provide: grants.provide,
            provide_names: grants.provide_names.into_iter().collect(),
            manage_apps: grants.manage_apps,
            manage_nodes: grants.manage_nodes,
        }
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
    let verifier = credential.verifier();
    let app = match tx.app_by_verifier(verifier)? {
        Some(app) => app,
        None => {
            let access = tx
                .oauth_access(verifier)?
                .filter(|access| access.expires_at > now)
                .ok_or(Error::Unauthenticated)?;
            let app = tx.app_by_id(access.app)?.ok_or(Error::Unauthenticated)?;
            if app.grants.manage_apps || app.grants.manage_nodes {
                return Err(Error::Unauthenticated);
            }
            app
        }
    };
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

/// Validate grant values constructed outside a typed OAuth scope.
pub fn validate_grants(grants: &Grants) -> Result<(), Error> {
    if !grants.provide && !grants.provide_names.is_empty() {
        return Err(Error::InvalidRequest);
    }
    for name in &grants.provide_names {
        harmion_protocol::interpreter::validate_provider_name(name)
            .map_err(|_| Error::InvalidRequest)?;
    }
    Ok(())
}
