//! Integrated OAuth connection requests. The owner approves app grants and the
//! optional Provider manifest together; the requesting app receives tokens and
//! a Provider ticket after proving its one-time request secret and PKCE verifier.
use crate::Error;
use crate::access::{Credential, Grants, authenticate, validate_grants};
use crate::application::Host;
use crate::interpreter::registry;
use crate::storage::{Store, Transaction};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use harmion_protocol::AppId;
use harmion_protocol::connect::{
    ConnectionId, ConnectionOffer, ConnectionStatus, ConnectionStatusView, MAX_PENDING_CONNECTIONS,
    validate_offer,
};
use harmion_protocol::interpreter::{ProviderId, ProviderRegistration, ProviderStatus};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

#[derive(Clone, Debug, PartialEq)]
pub struct ConnectionRequest {
    pub id: ConnectionId,
    pub secret_verifier: [u8; 32],
    pub name: String,
    pub redirect: String,
    pub web_origin: Option<String>,
    pub client: Option<AppId>,
    pub challenge: [u8; 32],
    pub state: String,
    pub grants: Grants,
    /// Canonical ProviderRegistration JSON, already validated by protocol.
    pub provider_manifest: Option<Vec<u8>>,
    pub provider_replace: bool,
    pub status: ConnectionStatus,
    pub created_at: i64,
    pub expires_at: i64,
    pub approved_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub app: Option<AppId>,
    pub provider_id: Option<ProviderId>,
}
impl ConnectionRequest {
    pub fn provider_name(&self) -> Result<Option<String>, Error> {
        self.provider_manifest
            .as_ref()
            .map(|bytes| {
                serde_json::from_slice::<ProviderRegistration>(bytes)
                    .map(|registration| registration.name)
                    .map_err(|_| Error::CorruptLog)
            })
            .transpose()
    }
    pub fn is_expired(&self, now: i64) -> bool {
        self.expires_at <= now
            && matches!(
                self.status,
                ConnectionStatus::Pending | ConnectionStatus::Approved
            )
    }
}

pub struct Started {
    pub id: ConnectionId,
    pub secret: [u8; 32],
    pub expires_at: i64,
}
pub struct Approval {
    pub app: Option<AppId>,
    pub provider: Option<ProviderId>,
    pub replaced: Option<ProviderId>,
    pub status: ConnectionStatus,
    pub grants: Option<Grants>,
}
pub struct Completion {
    pub app: AppId,
    pub provider: Option<ProviderId>,
    pub provider_name: Option<String>,
    pub tokens: crate::oauth::Tokens,
}

pub fn start(
    store: &mut impl Store,
    host: &mut impl Host,
    offer: ConnectionOffer,
) -> Result<Started, Error> {
    validate_offer(&offer)?;
    let grants: Grants = offer.grants.clone().into();
    validate_grants(&grants)?;
    let challenge: [u8; 32] = URL_SAFE_NO_PAD
        .decode(&offer.challenge)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or(Error::InvalidRequest)?;
    let client = offer
        .client_id
        .as_deref()
        .map(harmion_protocol::unhex::<32>)
        .transpose()
        .map_err(|_| Error::InvalidRequest)?
        .map(AppId);
    let provider_manifest = offer
        .provider
        .as_ref()
        .map(serde_json::to_vec)
        .transpose()
        .map_err(|_| Error::InvalidRequest)?;
    let secret = host.random_bytes()?;
    let id = ConnectionId(host.random_bytes()?);
    store.transaction(|tx| {
        expire_requests(tx, host.now_micros())?;
        if tx.connection_request_count(host.now_micros())? >= MAX_PENDING_CONNECTIONS {
            return Err(Error::ConnectionCapacity);
        }
        let now = host.now_micros();
        let request = ConnectionRequest {
            id,
            secret_verifier: secret_verifier(secret),
            name: offer.name,
            redirect: offer.redirect_uri,
            web_origin: offer.web_origin,
            client,
            challenge,
            state: offer.state,
            grants,
            provider_manifest,
            provider_replace: offer.provider_replace,
            status: ConnectionStatus::Pending,
            created_at: now,
            expires_at: now.saturating_add(5 * 60 * 1_000_000),
            approved_at: None,
            completed_at: None,
            app: None,
            provider_id: None,
        };
        tx.insert_connection_request(&request)?;
        Ok(Started {
            id,
            secret,
            expires_at: request.expires_at,
        })
    })
}

pub fn status(
    store: &mut impl Store,
    host: &mut impl Host,
    id: ConnectionId,
    secret: [u8; 32],
) -> Result<ConnectionStatusView, Error> {
    store.transaction(|tx| {
        let mut request = load(tx, id, secret)?;
        if request.is_expired(host.now_micros()) {
            request.status = ConnectionStatus::Expired;
            tx.update_connection_request(&request)?;
        }
        Ok(ConnectionStatusView {
            connection_id: harmion_protocol::connect::hex_connection_id(request.id),
            status: request.status.as_str(),
            name: request.name.clone(),
            web_origin: request.web_origin.clone(),
            provider_name: request.provider_name()?,
            expires_at: request.expires_at.to_string(),
        })
    })
}

pub fn list(
    store: &mut impl Store,
    owner: &Credential,
    now: i64,
) -> Result<Vec<ConnectionRequest>, Error> {
    store.transaction(|tx| {
        let principal = authenticate(tx, owner, now)?;
        if !principal.app.grants.manage_apps {
            return Err(Error::Forbidden);
        }
        expire_requests(tx, now)?;
        let mut requests = tx.connection_requests()?;
        requests.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| right.id.cmp(&left.id))
        });
        requests.truncate(32);
        Ok(requests)
    })
}

pub fn approve(
    store: &mut impl Store,
    host: &mut impl Host,
    owner: &Credential,
    id: ConnectionId,
    allow: bool,
    replace: bool,
) -> Result<Approval, Error> {
    store.transaction(|tx| {
        let principal = authenticate(tx, owner, host.now_micros())?;
        if !principal.app.grants.manage_apps {
            return Err(Error::Forbidden);
        }
        let mut request = tx
            .connection_request(id)?
            .ok_or(Error::ConnectionNotFound)?;
        if request.status != ConnectionStatus::Pending {
            return Err(match request.status {
                ConnectionStatus::Expired => Error::ConnectionExpired,
                ConnectionStatus::Denied => Error::ConnectionDenied,
                ConnectionStatus::Completed => Error::ConnectionCompleted,
                _ => Error::ConnectionPending,
            });
        }
        if request.is_expired(host.now_micros()) {
            request.status = ConnectionStatus::Expired;
            tx.update_connection_request(&request)?;
            return Err(Error::ConnectionExpired);
        }
        if !allow {
            request.status = ConnectionStatus::Denied;
            tx.update_connection_request(&request)?;
            return Ok(Approval {
                app: None,
                provider: None,
                replaced: None,
                status: ConnectionStatus::Denied,
                grants: None,
            });
        }
        let now = host.now_micros();
        let client = match request.client {
            Some(client) => {
                let existing = tx.oauth_client(client)?.ok_or(Error::InvalidRequest)?;
                if existing.name != request.name || existing.redirect != request.redirect {
                    return Err(Error::InvalidRequest);
                }
                existing
            }
            None => crate::oauth::register_client_in_tx(
                tx,
                host,
                request.name.clone(),
                request.redirect.clone(),
            )?,
        };
        let mut app = tx.app_by_id(client.id)?.ok_or(Error::InvalidRequest)?;
        if app.revoked || now >= app.expires_at {
            return Err(Error::Unauthenticated);
        }
        if app.grants != request.grants {
            tx.revoke_oauth_app(app.id)?;
            app.grants = request.grants.clone();
            tx.update_app(&app)?;
            registry::reconcile_grants(tx, app.id, &app.grants)?;
        }
        let mut provider = None;
        let mut replaced = None;
        if let Some(manifest) = &request.provider_manifest {
            let registration: ProviderRegistration =
                serde_json::from_slice(manifest).map_err(|_| Error::CorruptLog)?;
            if !app.grants.allows_provider_name(&registration.name) {
                return Err(Error::Forbidden);
            }
            if let Some(current) = tx.approved_interpreter_by_name(&registration.name)?
                && current.app != app.id
            {
                if !replace {
                    return Err(Error::InterpreterNameConflict);
                }
                tx.set_interpreter_status(
                    current.provider,
                    ProviderStatus::Superseded,
                    current.approved_at,
                )?;
                replaced = Some(current.provider);
            }
            let registration = registry::register_in_tx(tx, host, app.id, registration)?;
            if registration.status != ProviderStatus::Approved {
                return Err(Error::InterpreterNotApproved);
            }
            provider = Some(registration.provider);
        }
        request.status = ConnectionStatus::Approved;
        request.approved_at = Some(now);
        request.app = Some(app.id);
        request.provider_id = provider;
        tx.update_connection_request(&request)?;
        Ok(Approval {
            app: Some(app.id),
            provider,
            replaced,
            status: ConnectionStatus::Approved,
            grants: Some(app.grants),
        })
    })
}

pub fn complete(
    store: &mut impl Store,
    host: &mut impl Host,
    id: ConnectionId,
    secret: [u8; 32],
    verifier: &str,
) -> Result<Completion, Error> {
    store.transaction(|tx| {
        let mut request = load(tx, id, secret)?;
        if request.is_expired(host.now_micros()) {
            request.status = ConnectionStatus::Expired;
            tx.update_connection_request(&request)?;
            return Err(Error::ConnectionExpired);
        }
        match request.status {
            ConnectionStatus::Approved => {}
            ConnectionStatus::Pending => return Err(Error::ConnectionPending),
            ConnectionStatus::Denied => return Err(Error::ConnectionDenied),
            ConnectionStatus::Completed => return Err(Error::ConnectionCompleted),
            ConnectionStatus::Expired => return Err(Error::ConnectionExpired),
        }
        let expected = URL_SAFE_NO_PAD.encode(request.challenge);
        let presented = crate::oauth::pkce_challenge(verifier)?;
        if !bool::from(presented.as_bytes().ct_eq(expected.as_bytes())) {
            return Err(Error::InvalidRequest);
        }
        let app = request.app.ok_or(Error::CorruptLog)?;
        let tokens = crate::oauth::issue_for_app_in_tx(tx, host, app)?;
        request.status = ConnectionStatus::Completed;
        request.completed_at = Some(host.now_micros());
        tx.update_connection_request(&request)?;
        Ok(Completion {
            app,
            provider: request.provider_id,
            provider_name: request.provider_name()?,
            tokens,
        })
    })
}

fn load(
    tx: &mut dyn Transaction,
    id: ConnectionId,
    secret: [u8; 32],
) -> Result<ConnectionRequest, Error> {
    let request = tx
        .connection_request(id)?
        .ok_or(Error::ConnectionNotFound)?;
    if !bool::from(request.secret_verifier.ct_eq(&secret_verifier(secret))) {
        return Err(Error::ConnectionNotFound);
    }
    Ok(request)
}

fn expire_requests(tx: &mut dyn Transaction, now: i64) -> Result<(), Error> {
    tx.prune_connection_requests(now.saturating_sub(7 * 24 * 60 * 60 * 1_000_000))?;
    for mut request in tx.connection_requests()? {
        if request.is_expired(now) {
            request.status = ConnectionStatus::Expired;
            tx.update_connection_request(&request)?;
        }
    }
    Ok(())
}

fn secret_verifier(secret: [u8; 32]) -> [u8; 32] {
    Sha256::new()
        .chain_update(b"harmion:connection:v1\0")
        .chain_update(secret)
        .finalize()
        .into()
}
