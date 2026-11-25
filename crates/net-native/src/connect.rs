//! HTTP adapters for integrated OAuth connection requests.
use crate::{Error, SharedStore, SystemHost};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use harmion_node::access::Credential;
use harmion_node::application::Host;
use harmion_node::{connect as node_connect, interpreter::registry, storage::Store};
use harmion_protocol::connect::{
    ConnectionApproval, ConnectionCompleteRequest, ConnectionCompletion, ConnectionStarted,
    ConnectionStatusRequest, ProviderConnectionInfo, ProviderTicketRequest, decode_offer,
    hex_connection_id,
};
use harmion_protocol::interpreter::ProviderId;
use harmion_protocol::{GrantSpec, hex, unhex};
use serde_json::{Value as Json, json};
use std::{
    collections::BTreeMap,
    sync::{Mutex, OnceLock},
};

const COMPLETION_CACHE_TTL: i64 = 60_000_000;
const COMPLETION_CACHE_MAX: usize = 128;
/// Unauthenticated `connect-start` requests are cheap to send and create a
/// durable pending row. Keep a per-process budget so a local page or process
/// cannot fill the 32 pending slots and lock the owner out of the connect flow.
const CONNECT_START_BURST: u32 = 60;
const CONNECT_START_WINDOW_MICROS: i64 = 60_000_000;

struct StartBudget {
    window: i64,
    count: u32,
}

fn start_budget() -> &'static Mutex<StartBudget> {
    static BUDGET: OnceLock<Mutex<StartBudget>> = OnceLock::new();
    BUDGET.get_or_init(|| {
        Mutex::new(StartBudget {
            window: 0,
            count: 0,
        })
    })
}

fn allow_connect_start(now: i64) -> bool {
    let Ok(mut budget) = start_budget().lock() else {
        return false;
    };
    if now < budget.window || now - budget.window >= CONNECT_START_WINDOW_MICROS {
        budget.window = now;
        budget.count = 0;
    }
    if budget.count >= CONNECT_START_BURST {
        return false;
    }
    budget.count += 1;
    true
}

struct CachedCompletion {
    verifier: String,
    expires_at: i64,
    response: Json,
}

fn completion_cache() -> &'static Mutex<BTreeMap<[u8; 32], CachedCompletion>> {
    static CACHE: OnceLock<Mutex<BTreeMap<[u8; 32], CachedCompletion>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn cached_completion(key: [u8; 32], verifier: &str, now: i64) -> Result<Option<Json>, Error> {
    let mut cache = completion_cache()
        .lock()
        .map_err(|_| harmion_node::Error::Storage("completion cache poisoned".into()))?;
    cache.retain(|_, entry| entry.expires_at > now);
    let Some(entry) = cache.get(&key) else {
        return Ok(None);
    };
    if entry.verifier != verifier {
        return Err(harmion_node::Error::InvalidRequest.into());
    }
    Ok(Some(entry.response.clone()))
}

fn remember_completion(key: [u8; 32], verifier: String, response: Json) {
    let Ok(mut cache) = completion_cache().lock() else {
        return;
    };
    let now = SystemHost.now_micros();
    cache.retain(|_, entry| entry.expires_at > now);
    if cache.len() >= COMPLETION_CACHE_MAX
        && let Some(oldest) = cache
            .iter()
            .min_by_key(|(_, entry)| entry.expires_at)
            .map(|(key, _)| *key)
    {
        cache.remove(&oldest);
    }
    cache.insert(
        key,
        CachedCompletion {
            verifier,
            expires_at: now.saturating_add(COMPLETION_CACHE_TTL),
            response,
        },
    );
}

pub(crate) async fn handle_public<S: Store + Send + 'static>(
    store: &SharedStore<S>,
    operation: &str,
    body: Vec<u8>,
) -> Result<Json, Error> {
    match operation {
        "connect-start" => {
            if !allow_connect_start(SystemHost.now_micros()) {
                return Err(Error::Capacity);
            }
            let offer = decode_offer(&body)?;
            let state = offer.state.clone();
            let started = store
                .run(move |s| node_connect::start(s, &mut SystemHost, offer))
                .await?;
            let connection_id = hex_connection_id(started.id);
            let request_secret = hex(&started.secret);
            let verification_url = store.oauth.get().and_then(|oauth| {
                let payload = json!({
                    "connection_id": connection_id,
                    "request_secret": request_secret,
                    "state": state,
                });
                serde_json::to_vec(&payload).ok().map(|bytes| {
                    let encoded = URL_SAFE_NO_PAD.encode(bytes);
                    format!("{}/connect#{encoded}", oauth.origin())
                })
            });
            Ok(serde_json::to_value(ConnectionStarted {
                connection_id,
                request_secret,
                expires_at: started.expires_at.to_string(),
                verification_url,
            })
            .map_err(|_| harmion_node::Error::InvalidRequest)?)
        }
        "connect-status" => {
            let request: ConnectionStatusRequest = decode(&body)?;
            let id = harmion_protocol::connect::connection_id(&request.connection_id)?;
            let secret = unhex(&request.request_secret)?;
            let view = store
                .run(move |s| node_connect::status(s, &mut SystemHost, id, secret))
                .await?;
            Ok(serde_json::to_value(view).map_err(|_| harmion_node::Error::InvalidRequest)?)
        }
        "connect-complete" => {
            let request: ConnectionCompleteRequest = decode(&body)?;
            let id = harmion_protocol::connect::connection_id(&request.connection_id)?;
            let secret = unhex(&request.request_secret)?;
            let verifier = request.code_verifier;
            let key = Credential::from_bytes(secret).verifier();
            let now = SystemHost.now_micros();
            if let Some(response) = cached_completion(key, &verifier, now)? {
                return Ok(response);
            }
            let verifier_for_node = verifier.clone();
            let completion = store
                .run(move |s| {
                    node_connect::complete(s, &mut SystemHost, id, secret, &verifier_for_node)
                })
                .await?;
            let provider_info =
                |provider: ProviderId, name: String| -> Result<ProviderConnectionInfo, Error> {
                    match store.oauth.get() {
                        Some(oauth) => {
                            match oauth.issue_provider_ticket(completion.app, provider, now) {
                                Ok((ticket, _)) => {
                                    let url = oauth.provider_websocket_url(&ticket);
                                    Ok(ProviderConnectionInfo {
                                        provider_id: hex(&provider.0),
                                        name,
                                        ticket: Some(ticket),
                                        websocket_url: Some(url),
                                    })
                                }
                                Err(error) => {
                                    // Tokens are already issued and the request is
                                    // single-use. Never discard them because the
                                    // optional browser ticket could not be minted.
                                    tracing::warn!(%error, "provider ticket issuance failed");
                                    Ok(ProviderConnectionInfo {
                                        provider_id: hex(&provider.0),
                                        name,
                                        ticket: None,
                                        websocket_url: None,
                                    })
                                }
                            }
                        }
                        None => Ok(ProviderConnectionInfo {
                            provider_id: hex(&provider.0),
                            name,
                            ticket: None,
                            websocket_url: None,
                        }),
                    }
                };
            // `providers` is authoritative; the singular field is a compatibility
            // fallback for rows approved before multi-Provider support.
            let provider_ids = if completion.providers.is_empty() {
                completion.provider.into_iter().collect()
            } else {
                completion.providers.clone()
            };
            let providers = provider_ids
                .iter()
                .enumerate()
                .map(|(index, provider)| {
                    provider_info(
                        *provider,
                        completion
                            .provider_names
                            .get(index)
                            .cloned()
                            .unwrap_or_default(),
                    )
                })
                .collect::<Result<Vec<_>, Error>>()?;
            let provider = providers.first().cloned();
            let tokens = completion.tokens;
            let response = serde_json::to_value(ConnectionCompletion {
                token_type: "Bearer",
                access_token: hex(tokens.access.expose_bytes()),
                refresh_token: hex(&tokens.refresh),
                expires_in: tokens.expires_in,
                scope: crate::oauth_browser::format_scope(&tokens.grants),
                client_id: hex(&completion.app.0),
                provider,
                providers,
            })
            .map_err(|_| harmion_node::Error::InvalidRequest)?;
            remember_completion(key, verifier, response.clone());
            Ok(response)
        }
        _ => Err(harmion_node::Error::InvalidRequest.into()),
    }
}

pub(crate) async fn handle_admin<S: Store + Send + 'static>(
    store: &SharedStore<S>,
    credential: Credential,
    operation: &str,
    body: Vec<u8>,
) -> Result<Json, Error> {
    match operation {
        "connect-list" if body.is_empty() => {
            let requests = store
                .run(move |s| node_connect::list(s, &credential, SystemHost.now_micros()))
                .await?;
            let connections = requests
                .iter()
                .map(connection_json)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(json!({"connections": connections}))
        }
        "connect-approve" => {
            let request: ConnectionApproval = decode(&body)?;
            let id = harmion_protocol::connect::connection_id(&request.connection_id)?;
            let allow = request.allow;
            let replace = request.replace;
            let expires_at = request
                .expires_at
                .as_deref()
                .map(str::parse::<i64>)
                .transpose()
                .map_err(|_| harmion_node::Error::InvalidRequest)?;
            let result = store
                .run(move |s| {
                    node_connect::approve(
                        s,
                        &mut SystemHost,
                        &credential,
                        id,
                        allow,
                        replace,
                        expires_at,
                    )
                })
                .await?;
            for provider in &result.replaced_providers {
                store.interpreter.close_provider(*provider);
            }
            if let (Some(app), Some(grants)) = (result.app, result.grants.clone()) {
                store.notify_app_changed(app, &grants, false).await?;
            }
            let provider_ids = result
                .providers
                .iter()
                .map(|provider| hex(&provider.0))
                .collect::<Vec<_>>();
            let provider_names = result.provider_names.clone();
            let replaced_provider_ids = result
                .replaced_providers
                .iter()
                .map(|provider| hex(&provider.0))
                .collect::<Vec<_>>();
            Ok(json!({
                "connection_id": hex_connection_id(id),
                "status": result.status.as_str(),
                "app_id": result.app.map(|app| hex(&app.0)),
                "provider_id": result.provider.map(|provider| hex(&provider.0)),
                "provider_ids": provider_ids,
                "provider_names": provider_names,
                "replaced_provider_id": result.replaced.map(|provider| hex(&provider.0)),
                "replaced_provider_ids": replaced_provider_ids,
            }))
        }
        _ => Err(harmion_node::Error::InvalidRequest.into()),
    }
}

/// Issue a short-lived browser Provider ticket for an already approved Provider.
pub(crate) async fn provider_ticket<S: Store + Send + 'static>(
    store: &SharedStore<S>,
    credential: Credential,
    body: Vec<u8>,
) -> Result<Json, Error> {
    let provider = if body.len() == 32 {
        ProviderId(
            body.as_slice()
                .try_into()
                .map_err(|_| harmion_node::Error::InvalidRequest)?,
        )
    } else {
        let request: ProviderTicketRequest = decode(&body)?;
        ProviderId(unhex(&request.provider_id)?)
    };
    let now = SystemHost.now_micros();
    let registration = store
        .run(move |s| registry::authorize_attach(s, &credential, now, provider))
        .await?;
    let oauth = store
        .oauth
        .get()
        .cloned()
        .ok_or(harmion_node::Error::InvalidRequest)?;
    let (ticket, expires_at) = oauth.issue_provider_ticket(registration.app, provider, now)?;
    Ok(json!({
        "ticket": ticket,
        "expires_in": (expires_at - now) / 1_000_000,
        "websocket_url": oauth.provider_websocket_url(&ticket),
    }))
}

fn connection_json(request: &node_connect::ConnectionRequest) -> Result<Json, Error> {
    let providers = request.provider_registrations()?;
    let provider_names = providers
        .iter()
        .map(|registration| registration.name.clone())
        .collect::<Vec<_>>();
    let provider_ids = request
        .all_provider_ids()
        .iter()
        .map(|provider| hex(&provider.0))
        .collect::<Vec<_>>();
    Ok(json!({
        "connection_id": hex_connection_id(request.id),
        "name": request.name,
        "redirect_uri": request.redirect,
        "web_origin": request.web_origin,
        "client_id": request.client.map(|app| hex(&app.0)),
        "status": request.status.as_str(),
        "grants": GrantSpec::from(request.grants.clone()),
        "provider": providers.first(),
        "providers": providers,
        "provider_names": provider_names,
        "provider_replace": request.provider_replace,
        "app_id": request.app.map(|app| hex(&app.0)),
        "provider_id": provider_ids.first(),
        "provider_ids": provider_ids,
        "created_at": request.created_at.to_string(),
        "expires_at": request.expires_at.to_string(),
        "approved_at": request.approved_at.map(|value| value.to_string()),
        "completed_at": request.completed_at.map(|value| value.to_string()),
    }))
}

fn decode<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, Error> {
    if body.len() > 16 * 1024 {
        return Err(harmion_node::Error::InvalidRequest.into());
    }
    serde_json::from_slice(body).map_err(|_| harmion_node::Error::InvalidRequest.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use harmion_node::access::Grants;
    use harmion_protocol::connect::{ConnectionId, ConnectionStatus};
    use harmion_protocol::interpreter::{DefaultQuery, ProviderRegistration};

    fn registration(name: &str) -> ProviderRegistration {
        ProviderRegistration {
            name: name.into(),
            contract_version: 1,
            input_schema: json!({"type": "object"}),
            output_schema: json!({"type": "object"}),
            default_query: Some(DefaultQuery {
                kinds: vec!["ledger".into()],
            }),
        }
    }

    #[test]
    fn connection_json_lists_every_provider_and_keeps_the_legacy_first() {
        let providers = vec![registration("ledger/balance"), registration("wiki/outline")];
        let request = node_connect::ConnectionRequest {
            id: ConnectionId([1; 32]),
            secret_verifier: [0; 32],
            name: "Multi provider app".into(),
            redirect: "http://127.0.0.1:43123/callback".into(),
            web_origin: Some("http://127.0.0.1:43123".into()),
            client: None,
            challenge: [2; 32],
            state: "state".into(),
            grants: Grants {
                call: true,
                provide: true,
                provide_names: ["ledger/balance", "wiki/outline"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
                ..Default::default()
            },
            provider_manifest: Some(serde_json::to_vec(&providers).unwrap()),
            provider_replace: true,
            status: ConnectionStatus::Pending,
            created_at: 10,
            expires_at: 20,
            approved_at: None,
            completed_at: None,
            app: None,
            provider_id: None,
            provider_ids: Vec::new(),
        };
        let value = connection_json(&request).unwrap();
        assert_eq!(value["provider"]["name"], "ledger/balance");
        assert_eq!(value["providers"].as_array().unwrap().len(), 2);
        assert_eq!(
            value["provider_names"],
            json!(["ledger/balance", "wiki/outline"])
        );
        assert_eq!(
            value["grants"]["provide_names"],
            json!(["ledger/balance", "wiki/outline"])
        );
        assert_eq!(value["provider_replace"], true);
    }
}
