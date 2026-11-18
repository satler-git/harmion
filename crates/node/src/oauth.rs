//! Node-local authorization code and token lifecycle. HTTP and consent UI live in the host.

use crate::{
    Error,
    access::{App, Credential, Filter, Grants, authenticate},
    application::Host,
    storage::{Store, Transaction},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use harmion_protocol::AppId;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use url::{Host as UrlHost, Url};

const SECOND: i64 = 1_000_000;
const PENDING_TTL: i64 = 5 * 60 * SECOND;
const CODE_TTL: i64 = 60 * SECOND;
pub const ACCESS_TTL_SECONDS: i64 = 10 * 60;
const FAMILY_TTL: i64 = 30 * 24 * 60 * 60 * SECOND;
pub const MAX_PENDING: usize = 128;

#[derive(Clone, PartialEq, Eq)]
pub struct Client {
    pub id: AppId,
    pub name: String,
    pub redirect: String,
}

#[derive(Clone)]
pub struct Authorization {
    pub request: [u8; 32],
    pub client: AppId,
    pub redirect: String,
    pub challenge: [u8; 32],
    pub state: String,
    pub grants: Grants,
    pub expires_at: i64,
}

pub struct Code {
    pub client: AppId,
    pub redirect: String,
    pub challenge: [u8; 32],
    pub generation: u64,
    pub expires_at: i64,
    pub family: Option<[u8; 32]>,
}

pub struct Access {
    pub app: AppId,
    pub expires_at: i64,
}

pub struct Refresh {
    pub family: [u8; 32],
    pub app: AppId,
    pub expires_at: i64,
    pub used: bool,
}

pub struct Decision {
    pub redirect: String,
    pub state: String,
    pub code: Option<[u8; 32]>,
}

pub struct Tokens {
    pub access: Credential,
    pub refresh: [u8; 32],
    pub expires_in: i64,
    pub grants: Grants,
}

/// Persisted state belongs to the issuing node and is excluded from replication.
pub trait Storage {
    fn oauth_client(&mut self, id: AppId) -> Result<Option<Client>, Error>;
    fn put_oauth_client(&mut self, client: &Client) -> Result<(), Error>;
    fn oauth_pending(&mut self, request: [u8; 32]) -> Result<Option<Authorization>, Error>;
    fn oauth_pending_count(&mut self, now: i64) -> Result<usize, Error>;
    fn put_oauth_pending(&mut self, authorization: &Authorization) -> Result<(), Error>;
    fn delete_oauth_pending(&mut self, request: [u8; 32]) -> Result<(), Error>;
    fn oauth_code(&mut self, digest: [u8; 32]) -> Result<Option<Code>, Error>;
    fn put_oauth_code(&mut self, digest: [u8; 32], code: &Code) -> Result<(), Error>;
    fn consume_oauth_code(&mut self, digest: [u8; 32], family: [u8; 32]) -> Result<(), Error>;
    fn oauth_access(&mut self, verifier: [u8; 32]) -> Result<Option<Access>, Error>;
    fn put_oauth_access(
        &mut self,
        verifier: [u8; 32],
        family: [u8; 32],
        expires_at: i64,
    ) -> Result<(), Error>;
    fn oauth_refresh(&mut self, digest: [u8; 32]) -> Result<Option<Refresh>, Error>;
    fn put_oauth_refresh(&mut self, digest: [u8; 32], family: [u8; 32]) -> Result<(), Error>;
    fn consume_oauth_refresh(&mut self, digest: [u8; 32]) -> Result<(), Error>;
    fn put_oauth_family(
        &mut self,
        family: [u8; 32],
        app: AppId,
        expires_at: i64,
    ) -> Result<(), Error>;
    fn revoke_oauth_family(&mut self, family: [u8; 32]) -> Result<(), Error>;
    fn revoke_oauth_app(&mut self, app: AppId) -> Result<(), Error>;
    fn prune_oauth(&mut self, now: i64) -> Result<(), Error>;
}

pub fn pkce_challenge(verifier: &str) -> Result<String, Error> {
    Ok(URL_SAFE_NO_PAD.encode(pkce_digest(verifier)?))
}

fn pkce_digest(verifier: &str) -> Result<[u8; 32], Error> {
    if !(43..=128).contains(&verifier.len())
        || !verifier
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~'))
    {
        return Err(Error::OAuthInvalidGrant);
    }
    Ok(Sha256::digest(verifier.as_bytes()).into())
}

/// Native public clients register a literal loopback address and exact path/query.
fn redirect_parts(redirect: &str) -> Result<(&str, &str), Error> {
    if redirect.len() > 2048
        || !redirect.bytes().all(|b| b.is_ascii_graphic())
        || redirect.contains(['\\', '#'])
    {
        return Err(Error::InvalidRequest);
    }
    let parsed = Url::parse(redirect).map_err(|_| Error::InvalidRequest)?;
    let loopback = match parsed.host() {
        Some(UrlHost::Ipv4(ip)) => ip.is_loopback(),
        Some(UrlHost::Ipv6(ip)) => ip.is_loopback(),
        _ => false,
    };
    if !loopback
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port() == Some(0)
        || parsed.query_pairs().any(|(key, _)| {
            matches!(
                key.as_ref(),
                "code" | "state" | "error" | "error_description"
            )
        })
    {
        return Err(Error::InvalidRequest);
    }
    let raw = redirect
        .strip_prefix("http://")
        .ok_or(Error::InvalidRequest)?;
    let boundary = raw.find(['/', '?']).unwrap_or(raw.len());
    let (authority, remainder) = raw.split_at(boundary);
    let host_end = if authority.starts_with('[') {
        authority.find(']').ok_or(Error::InvalidRequest)? + 1
    } else {
        authority.find(':').unwrap_or(authority.len())
    };
    let (host, port) = authority.split_at(host_end);
    if host != parsed.host_str().ok_or(Error::InvalidRequest)?
        || (!port.is_empty()
            && port
                .strip_prefix(':')
                .and_then(|p| p.parse::<u16>().ok())
                .filter(|p| *p != 0)
                .is_none())
    {
        return Err(Error::InvalidRequest);
    }
    Ok((host, remainder))
}

pub fn register(
    store: &mut impl Store,
    host: &mut impl Host,
    owner: &Credential,
    name: String,
    redirect: String,
) -> Result<Client, Error> {
    redirect_parts(&redirect)?;
    if name.trim().is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
        return Err(Error::InvalidRequest);
    }
    store.transaction(|tx| {
        require_owner(tx, owner, host.now_micros())?;
        let client = Client {
            id: AppId(host.random_bytes()?),
            name,
            redirect,
        };
        // A registered public client has no direct bearer credential.
        tx.insert_app(&App {
            id: client.id,
            verifier: host.random_bytes()?,
            expires_at: i64::MAX,
            revoked: false,
            grants: Grants::default(),
        })?;
        tx.put_oauth_client(&client)?;
        Ok(client)
    })
}

pub struct Start {
    pub client: AppId,
    pub redirect: String,
    pub challenge: String,
    pub state: String,
    pub grants: Grants,
}

pub fn start(
    store: &mut impl Store,
    host: &mut impl Host,
    request: Start,
) -> Result<Authorization, Error> {
    if request.challenge.len() != 43 {
        return Err(Error::InvalidRequest);
    }
    let challenge = URL_SAFE_NO_PAD
        .decode(&request.challenge)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or(Error::InvalidRequest)?;
    if request.state.is_empty()
        || request.state.len() > 512
        || request.state.chars().any(char::is_control)
        || request.grants.manage_apps
        || request.grants.manage_nodes
        || (request.grants.read.is_none() && request.grants.write.is_none())
    {
        return Err(Error::InvalidRequest);
    }
    store.transaction(|tx| {
        let now = host.now_micros();
        let client = tx
            .oauth_client(request.client)?
            .ok_or(Error::InvalidRequest)?;
        active_app(tx, client.id, now)?;
        if redirect_parts(&client.redirect)? != redirect_parts(&request.redirect)? {
            return Err(Error::InvalidRequest);
        }
        tx.prune_oauth(now)?;
        if tx.oauth_pending_count(now)? >= MAX_PENDING {
            return Err(Error::OAuthCapacity);
        }
        let authorization = Authorization {
            request: host.random_bytes()?,
            client: client.id,
            redirect: request.redirect,
            challenge,
            state: request.state,
            grants: request.grants,
            expires_at: now.saturating_add(PENDING_TTL),
        };
        tx.put_oauth_pending(&authorization)?;
        Ok(authorization)
    })
}

pub fn pending(
    store: &mut impl Store,
    owner: &Credential,
    now: i64,
    request: [u8; 32],
) -> Result<(Client, Authorization), Error> {
    store.transaction(|tx| {
        require_owner(tx, owner, now)?;
        let authorization = load_pending(tx, request, now)?;
        let client = tx
            .oauth_client(authorization.client)?
            .ok_or(Error::CorruptLog)?;
        Ok((client, authorization))
    })
}

pub fn decide(
    store: &mut impl Store,
    host: &mut impl Host,
    owner: &Credential,
    request: [u8; 32],
    allow: bool,
) -> Result<Decision, Error> {
    store.transaction(|tx| {
        let now = host.now_micros();
        require_owner(tx, owner, now)?;
        let pending = load_pending(tx, request, now)?;
        let mut app = active_app(tx, pending.client, now)?;
        let code = if allow {
            if app.grants != pending.grants {
                tx.revoke_oauth_app(app.id)?;
                app.grants = pending.grants;
                tx.update_app(&app)?;
            }
            let code = host.random_bytes()?;
            let generation = tx.grant_generation(app.id)?;
            tx.put_oauth_code(
                digest(b"code", code),
                &Code {
                    client: app.id,
                    redirect: pending.redirect.clone(),
                    challenge: pending.challenge,
                    generation,
                    expires_at: now.saturating_add(CODE_TTL),
                    family: None,
                },
            )?;
            Some(code)
        } else {
            None
        };
        tx.delete_oauth_pending(request)?;
        Ok(Decision {
            redirect: pending.redirect,
            state: pending.state,
            code,
        })
    })
}

pub fn exchange(
    store: &mut impl Store,
    host: &mut impl Host,
    client: AppId,
    code: [u8; 32],
    redirect: &str,
    verifier: &str,
) -> Result<Tokens, Error> {
    let challenge = pkce_digest(verifier)?;
    let result = store.transaction(|tx| {
        let now = host.now_micros();
        let digest = digest(b"code", code);
        let code = tx.oauth_code(digest)?.ok_or(Error::OAuthInvalidGrant)?;
        if code.client != client
            || code.redirect != redirect
            || code.expires_at <= now
            || !bool::from(code.challenge.ct_eq(&challenge))
            || code.generation != tx.grant_generation(client)?
        {
            return Err(Error::OAuthInvalidGrant);
        }
        let app = active_app(tx, client, now)?;
        if let Some(family) = code.family {
            tx.revoke_oauth_family(family)?;
            return Ok(None);
        }
        let family = host.random_bytes()?;
        let expires = now.saturating_add(FAMILY_TTL).min(app.expires_at);
        tx.prune_oauth(now)?;
        tx.put_oauth_family(family, client, expires)?;
        tx.consume_oauth_code(digest, family)?;
        Ok(Some(issue_tokens(
            tx, host, family, now, expires, app.grants,
        )?))
    })?;
    result.ok_or(Error::OAuthInvalidGrant)
}

pub fn refresh(
    store: &mut impl Store,
    host: &mut impl Host,
    client: AppId,
    refresh: [u8; 32],
) -> Result<Tokens, Error> {
    let result = store.transaction(|tx| {
        let now = host.now_micros();
        let digest = digest(b"refresh", refresh);
        let token = tx.oauth_refresh(digest)?.ok_or(Error::OAuthInvalidGrant)?;
        if token.app != client || token.expires_at <= now {
            return Err(Error::OAuthInvalidGrant);
        }
        let app = active_app(tx, client, now)?;
        if token.used {
            tx.revoke_oauth_family(token.family)?;
            return Ok(None);
        }
        tx.prune_oauth(now)?;
        tx.consume_oauth_refresh(digest)?;
        Ok(Some(issue_tokens(
            tx,
            host,
            token.family,
            now,
            token.expires_at.min(app.expires_at),
            app.grants,
        )?))
    })?;
    result.ok_or(Error::OAuthInvalidGrant)
}

fn issue_tokens(
    tx: &mut dyn Transaction,
    host: &mut impl Host,
    family: [u8; 32],
    now: i64,
    expires: i64,
    grants: Grants,
) -> Result<Tokens, Error> {
    let access = Credential::from_bytes(host.random_bytes()?);
    let refresh = host.random_bytes()?;
    let expires_at = now.saturating_add(ACCESS_TTL_SECONDS * SECOND).min(expires);
    tx.put_oauth_access(access.verifier(), family, expires_at)?;
    tx.put_oauth_refresh(digest(b"refresh", refresh), family)?;
    Ok(Tokens {
        access,
        refresh,
        expires_in: expires_at.saturating_sub(now) / SECOND,
        grants,
    })
}

fn digest(domain: &[u8], secret: [u8; 32]) -> [u8; 32] {
    Sha256::new()
        .chain_update(b"harmion:oauth:v1\0")
        .chain_update(domain)
        .chain_update(secret)
        .finalize()
        .into()
}

fn active_app(tx: &mut dyn Transaction, id: AppId, now: i64) -> Result<App, Error> {
    tx.app_by_id(id)?
        .filter(|app| !app.revoked && app.expires_at > now)
        .ok_or(Error::OAuthInvalidGrant)
}

fn load_pending(
    tx: &mut dyn Transaction,
    request: [u8; 32],
    now: i64,
) -> Result<Authorization, Error> {
    tx.oauth_pending(request)?
        .filter(|authorization| authorization.expires_at > now)
        .ok_or(Error::OAuthInvalidGrant)
}

fn require_owner(tx: &mut dyn Transaction, owner: &Credential, now: i64) -> Result<(), Error> {
    if !authenticate(tx, owner, now)?.app.grants.manage_apps {
        return Err(Error::Forbidden);
    }
    Ok(())
}

pub(crate) fn restrict_grant_update(previous: &Grants, new: &Grants) -> Result<(), Error> {
    let subset = |old: &Option<Filter>, new: &Option<Filter>| match (old, new) {
        (_, None) | (Some(Filter::All), _) => true,
        (Some(Filter::Kinds(old)), Some(Filter::Kinds(new))) => new.is_subset(old),
        _ => false,
    };
    if new.manage_apps
        || new.manage_nodes
        || !subset(&previous.read, &new.read)
        || !subset(&previous.write, &new.write)
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}
