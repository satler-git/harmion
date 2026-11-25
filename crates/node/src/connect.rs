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
    /// Provider manifests already validated by protocol. New rows store a JSON
    /// array; rows written before multi-Provider support store one JSON object.
    pub provider_manifest: Option<Vec<u8>>,
    pub provider_replace: bool,
    pub status: ConnectionStatus,
    pub created_at: i64,
    pub expires_at: i64,
    pub approved_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub app: Option<AppId>,
    /// First approved Provider, retained for legacy rows and clients.
    pub provider_id: Option<ProviderId>,
    /// Every Provider approved by this connection, in offer order.
    pub provider_ids: Vec<ProviderId>,
}
impl ConnectionRequest {
    /// Decode the stored manifests. A malformed row is corrupt; the legacy
    /// single-object representation is still accepted.
    pub fn provider_registrations(&self) -> Result<Vec<ProviderRegistration>, Error> {
        let Some(bytes) = &self.provider_manifest else {
            return Ok(Vec::new());
        };
        if let Ok(registrations) = serde_json::from_slice::<Vec<ProviderRegistration>>(bytes) {
            return Ok(registrations);
        }
        serde_json::from_slice::<ProviderRegistration>(bytes)
            .map(|registration| vec![registration])
            .map_err(|_| Error::CorruptLog)
    }
    pub fn provider_names(&self) -> Result<Vec<String>, Error> {
        Ok(self
            .provider_registrations()?
            .into_iter()
            .map(|registration| registration.name)
            .collect())
    }
    pub fn provider_name(&self) -> Result<Option<String>, Error> {
        Ok(self.provider_names()?.into_iter().next())
    }
    /// All approved IDs, falling back to the legacy singular column for rows
    /// written before the `providers` column existed.
    pub fn all_provider_ids(&self) -> Vec<ProviderId> {
        if self.provider_ids.is_empty() {
            self.provider_id.into_iter().collect()
        } else {
            self.provider_ids.clone()
        }
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
    /// First registered Provider, retained for older clients.
    pub provider: Option<ProviderId>,
    /// Every Provider registered by this approval, in offer order.
    pub providers: Vec<ProviderId>,
    pub provider_names: Vec<String>,
    /// First superseded Provider, retained for older clients.
    pub replaced: Option<ProviderId>,
    /// Every Provider superseded by this approval.
    pub replaced_providers: Vec<ProviderId>,
    pub status: ConnectionStatus,
    pub grants: Option<Grants>,
}
pub struct Completion {
    pub app: AppId,
    /// First Provider connection, retained for older clients.
    pub provider: Option<ProviderId>,
    pub provider_name: Option<String>,
    /// Every Provider registered by the approval, in offer order.
    pub providers: Vec<ProviderId>,
    pub provider_names: Vec<String>,
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
    let registrations: Vec<ProviderRegistration> = offer.all_providers().cloned().collect();
    let provider_manifest = if registrations.is_empty() {
        None
    } else {
        Some(serde_json::to_vec(&registrations).map_err(|_| Error::InvalidRequest)?)
    };
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
            provider_ids: Vec::new(),
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
        let provider_names = request.provider_names()?;
        Ok(ConnectionStatusView {
            connection_id: harmion_protocol::connect::hex_connection_id(request.id),
            status: request.status.as_str(),
            name: request.name.clone(),
            web_origin: request.web_origin.clone(),
            provider_name: provider_names.first().cloned(),
            provider_names,
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
    expires_at: Option<i64>,
) -> Result<Approval, Error> {
    if let Some(expires) = expires_at {
        const MAX_TTL_MICROS: i64 = 10 * 365 * 24 * 60 * 60 * 1_000_000;
        let now = host.now_micros();
        if expires <= now || expires > now.saturating_add(MAX_TTL_MICROS) {
            return Err(Error::InvalidRequest);
        }
    }
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
                providers: Vec::new(),
                provider_names: Vec::new(),
                replaced: None,
                replaced_providers: Vec::new(),
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
        let grants_changed = app.grants != request.grants;
        if grants_changed {
            tx.revoke_oauth_app(app.id)?;
            app.grants = request.grants.clone();
        }
        let mut app_changed = grants_changed;
        if let Some(expires) = expires_at
            && app.expires_at != expires
        {
            app.expires_at = expires;
            app_changed = true;
        }
        if app_changed {
            tx.update_app(&app)?;
        }
        if grants_changed {
            registry::reconcile_grants(tx, app.id, &app.grants)?;
        }
        let mut providers = Vec::new();
        let mut provider_names = Vec::new();
        let mut replaced_providers = Vec::new();
        for registration in request.provider_registrations()? {
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
                replaced_providers.push(current.provider);
            }
            let name = registration.name.clone();
            let registration = registry::register_in_tx(tx, host, app.id, registration)?;
            if registration.status != ProviderStatus::Approved {
                return Err(Error::InterpreterNotApproved);
            }
            providers.push(registration.provider);
            provider_names.push(name);
        }
        request.status = ConnectionStatus::Approved;
        request.approved_at = Some(now);
        request.app = Some(app.id);
        request.provider_id = providers.first().copied();
        request.provider_ids = providers.clone();
        tx.update_connection_request(&request)?;
        Ok(Approval {
            app: Some(app.id),
            provider: providers.first().copied(),
            providers,
            provider_names,
            replaced: replaced_providers.first().copied(),
            replaced_providers,
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
        let provider_ids = request.all_provider_ids();
        let provider_names = request.provider_names()?;
        Ok(Completion {
            app,
            provider: provider_ids.first().copied(),
            provider_name: provider_names.first().cloned(),
            providers: provider_ids,
            provider_names,
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

#[cfg(test)]
mod tests {
    use super::*;
    use harmion_protocol::GrantSpec;
    use harmion_protocol::connect::ConnectionId;
    use serde_json::json;

    fn registration(name: &str) -> ProviderRegistration {
        ProviderRegistration {
            name: name.into(),
            contract_version: harmion_protocol::interpreter::CONTRACT_VERSION,
            input_schema: json!({"type": "object"}),
            output_schema: json!({"type": "object"}),
            default_query: None,
        }
    }

    fn request(manifest: Option<Vec<u8>>) -> ConnectionRequest {
        ConnectionRequest {
            id: ConnectionId([1; 32]),
            secret_verifier: [2; 32],
            name: "Wiki".into(),
            redirect: "http://127.0.0.1:43123/callback".into(),
            web_origin: None,
            client: None,
            challenge: [3; 32],
            state: "state".into(),
            grants: GrantSpec {
                read: Some(harmion_protocol::FilterSpec::All),
                write: None,
                call: true,
                provide: true,
                provide_names: vec!["wiki/backlinks".into(), "wiki/outline".into()],
                manage_apps: false,
                manage_nodes: false,
            }
            .into(),
            provider_manifest: manifest,
            provider_replace: false,
            status: ConnectionStatus::Pending,
            created_at: 0,
            expires_at: 1,
            approved_at: None,
            completed_at: None,
            app: None,
            provider_id: None,
            provider_ids: Vec::new(),
        }
    }

    #[test]
    fn legacy_single_object_manifest_still_parses() {
        let mut request = request(Some(
            serde_json::to_vec(&registration("wiki/backlinks")).unwrap(),
        ));
        assert_eq!(
            request.provider_names().unwrap(),
            ["wiki/backlinks".to_string()]
        );
        assert_eq!(
            request.provider_name().unwrap().as_deref(),
            Some("wiki/backlinks")
        );
        request.provider_id = Some(ProviderId([9; 32]));
        assert_eq!(request.all_provider_ids(), [ProviderId([9; 32])]);
    }

    #[test]
    fn manifest_array_round_trips_and_prefers_explicit_provider_ids() {
        let manifests = vec![registration("wiki/backlinks"), registration("wiki/outline")];
        let mut request = request(Some(serde_json::to_vec(&manifests).unwrap()));
        let manifest_ids = [ProviderId([8; 32]), ProviderId([9; 32])];
        request.provider_id = Some(manifest_ids[0]);
        request.provider_ids = manifest_ids.to_vec();
        assert_eq!(
            request.provider_names().unwrap(),
            ["wiki/backlinks".to_string(), "wiki/outline".to_string()]
        );
        assert_eq!(
            request.provider_registrations().unwrap().as_slice(),
            manifests.as_slice()
        );
        assert_eq!(request.all_provider_ids(), manifest_ids);
    }
}
