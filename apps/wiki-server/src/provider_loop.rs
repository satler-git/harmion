//! Provider registration/attach loop for a node-backed Wiki server.
//!
//! `wiki-server connect` puts Provider manifests into the integrated OAuth
//! connection offer. The node stores and approves them because the same names
//! are listed in `provide_names`, then returns one `provider_id` per manifest;
//! `connect` persists the bindings in `credentials.json` (mode 0600).
//! `serve --node DIR` loads the startup snapshot and runs one attach loop per
//! registered Provider, all from the same snapshot.
//!
//! # Safety
//!
//! The loop uses only the OAuth access token from credentials, prints neither
//! access nor refresh tokens, performs no App API writes, and answers each
//! invocation from the already-loaded local snapshot.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use harmion_client::protocol::interpreter::{
    CONTRACT_VERSION, DefaultQuery, Display, ProviderFailure, ProviderId, ProviderRegistration,
    validate_provider_name, validate_registration,
};
use harmion_client::protocol::{self, ConnectionInfo};
use harmion_client::{Error as ClientError, OAuthTokenClient, ProviderClient, ProviderEvent};
use serde_json::{Value, json};
use tokio::time::sleep;

use crate::auth::{self, Credentials, ProviderBinding};
use crate::error::RpcError;
use crate::provider::{self, NAMES, ProviderOutput};
use crate::store::State;

/// Provider registered by `connect` when `--provider-name` is omitted.
pub const DEFAULT_PROVIDER_NAME: &str = "wiki/outline";

/// First reconnect delay; doubled after each consecutive failure.
const RECONNECT_BASE_MILLIS: u64 = 100;
/// Reconnect delay never exceeds this value.
pub const MAX_RECONNECT_MILLIS: u64 = 30_000;
/// `provide_names` is bounded to 32 names by the connection contract.
const MAX_PROVIDE_NAMES: usize = 32;
/// Provider failures are bounded by the interpreter contract.
const MAX_FAILURE_MESSAGE_BYTES: usize = 1024;

/// Build one manifest for the integrated connection.
///
/// Only the three names implemented by `provider::invoke` are accepted, so the
/// manifest cannot promise an operation this process cannot serve.
pub fn registration(name: &str) -> Result<ProviderRegistration, String> {
    validate_provider_name(name)
        .map_err(|error| format!("invalid provider name {name:?}: {error}"))?;
    if !NAMES.contains(&name) {
        return Err(format!(
            "unsupported Wiki provider {name:?}; choose one of {}",
            NAMES.join(", ")
        ));
    }
    let (properties, required) = if name == "wiki/transclude" {
        (
            json!({
                "id": {"type": "string", "description": "Wiki Content ID"},
                "depth": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 3,
                    "default": 1,
                    "description": "Transclusion depth (clamped to 1..=3)"
                }
            }),
            json!(["id"]),
        )
    } else {
        (
            json!({
                "id": {"type": "string", "description": "Wiki Content ID"}
            }),
            json!(["id"]),
        )
    };
    let registration = ProviderRegistration {
        name: name.to_owned(),
        contract_version: CONTRACT_VERSION,
        input_schema: json!({
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false
        }),
        output_schema: json!({
            "type": "object",
            "properties": {
                "display": {"type": "string", "description": "Markdown display text"},
                "media_type": {"const": "text/markdown"},
                "data": {"description": "Structured links, outline entries or transclusion result"}
            },
            "required": ["display", "data"]
        }),
        default_query: Some(DefaultQuery {
            kinds: vec!["wiki".to_owned()],
        }),
    };
    validate_registration(&registration)
        .map_err(|error| format!("invalid Wiki provider manifest for {name:?}: {error}"))?;
    Ok(registration)
}

/// Resolve the connect-time provider choice. Registration requires a `provide`
/// grant; `--no-provide` therefore also suppresses the manifest, while
/// `--no-provider` only skips the manifest and keeps the requested names.
pub fn registrations_for_connect(
    provide: bool,
    no_provider: bool,
    provider_name: Option<&str>,
) -> Result<Vec<ProviderRegistration>, String> {
    if !provide || no_provider {
        return Ok(Vec::new());
    }
    let names: Vec<&str> = match provider_name {
        Some(name) => vec![name],
        None => NAMES.to_vec(),
    };
    names.into_iter().map(registration).collect()
}

pub fn registration_for_connect(
    provide: bool,
    no_provider: bool,
    provider_name: Option<&str>,
) -> Result<Option<ProviderRegistration>, String> {
    Ok(
        registrations_for_connect(provide, no_provider, provider_name)?
            .into_iter()
            .next(),
    )
}

/// The grant names to request. All three built-in names are requested when the
/// caller did not enumerate any; the registered name is always present.
pub fn requested_provide_names(
    registered: Option<&str>,
    requested: &[String],
) -> Result<Vec<String>, String> {
    let mut names = if requested.is_empty() {
        NAMES.map(str::to_owned).to_vec()
    } else {
        requested.to_vec()
    };
    if let Some(name) = registered
        && !names.iter().any(|existing| existing == name)
    {
        names.push(name.to_owned());
    }
    if names.len() > MAX_PROVIDE_NAMES {
        return Err(format!(
            "at most {MAX_PROVIDE_NAMES} provider names can be requested (got {})",
            names.len()
        ));
    }
    Ok(names)
}

/// Extract all persisted Provider bindings from a `connect-complete` response
/// and verify the node returned the requested names. No token is touched here.
pub fn completion_providers(
    completion: &Value,
    expected_names: &[String],
) -> Result<Vec<ProviderBinding>, String> {
    let entries = completion
        .get("providers")
        .and_then(Value::as_array)
        .cloned()
        .or_else(|| {
            completion
                .get("provider")
                .filter(|value| !value.is_null())
                .map(|value| vec![value.clone()])
        })
        .unwrap_or_default();
    let mut bindings = Vec::new();
    for entry in entries {
        let name = entry
            .get("name")
            .and_then(Value::as_str)
            .ok_or("connect-complete returned a provider without name")?
            .to_owned();
        let id = entry
            .get("provider_id")
            .and_then(Value::as_str)
            .ok_or("connect-complete returned a provider without provider_id")?;
        protocol::unhex::<32>(id)
            .map_err(|_| "connect-complete returned an invalid provider_id".to_owned())?;
        if !expected_names.is_empty() && !expected_names.iter().any(|expected| expected == &name) {
            return Err(format!(
                "node registered provider {name:?}, expected one of {expected_names:?}"
            ));
        }
        bindings.push(ProviderBinding {
            name,
            provider_id: id.to_owned(),
        });
    }
    Ok(bindings)
}

pub fn completion_provider_id(
    completion: &Value,
    expected_name: Option<&str>,
) -> Result<Option<ProviderId>, String> {
    let provider = match completion.get("provider") {
        None | Some(Value::Null) => return Ok(None),
        Some(provider) => provider,
    };
    let text = provider
        .get("provider_id")
        .and_then(Value::as_str)
        .ok_or("connect-complete returned a provider without provider_id")?;
    let bytes = protocol::unhex::<32>(text)
        .map_err(|_| "connect-complete returned an invalid provider_id".to_owned())?;
    if let Some(expected) = expected_name
        && let Some(name) = provider.get("name").and_then(Value::as_str)
        && name != expected
    {
        return Err(format!(
            "node registered provider {name:?}, expected {expected:?}"
        ));
    }
    Ok(Some(ProviderId(bytes)))
}

/// Run one invocation through the same implementation as local `provider_call`.
pub fn dispatch(
    state: &State,
    name: &str,
    input: &Value,
) -> Result<ProviderOutput, ProviderFailure> {
    provider::invoke(state, name, input).map_err(|error| provider_failure(&error))
}

fn provider_failure(error: &RpcError) -> ProviderFailure {
    let code = match error.code {
        400 => "invalid_input",
        404 => "not_found",
        409 => "conflict",
        413 => "too_large",
        500 => "internal",
        503 => "unavailable",
        _ => "provider_error",
    };
    ProviderFailure {
        code: code.to_owned(),
        message: bounded_message(&error.message),
        details: error.data.clone(),
    }
}

fn bounded_message(message: &str) -> String {
    if message.len() <= MAX_FAILURE_MESSAGE_BYTES {
        return message.to_owned();
    }
    let mut end = MAX_FAILURE_MESSAGE_BYTES;
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    message[..end].to_owned()
}

/// Bounded exponential backoff for reconnects. The delay is reset after a
/// successful attach, so a healthy connection that later closes retries quickly.
pub fn reconnect_delay(attempt: u32) -> Duration {
    let factor = 1u64 << attempt.min(9);
    Duration::from_millis(
        RECONNECT_BASE_MILLIS
            .saturating_mul(factor)
            .min(MAX_RECONNECT_MILLIS),
    )
}

/// Load the persisted provider registration and start the attach loop on its own
/// thread. A missing credential provider ID is not fatal for `serve`: the local
/// JSON-RPC endpoint still works, and the reason is printed for the operator.
pub fn spawn(dir: PathBuf, state: Arc<Mutex<State>>) -> Vec<thread::JoinHandle<()>> {
    let credentials = match Credentials::load(&dir) {
        Ok(credentials) => credentials,
        Err(error) => {
            eprintln!("provider: cannot load node credentials: {error}");
            return Vec::new();
        }
    };
    let bindings = credentials.provider_bindings();
    if bindings.is_empty() {
        eprintln!(
            "provider: no Provider is registered in {}; run `wiki-server connect --node {}` \
             (registration is enabled by default) to attach providers",
            dir.join(auth::CREDENTIALS_FILE).display(),
            dir.display()
        );
        return Vec::new();
    }
    let info = match auth::connection_info(&dir) {
        Ok(info) => info,
        Err(error) => {
            eprintln!("provider: cannot read the node connection: {error}");
            return Vec::new();
        }
    };
    let mut handles = Vec::new();
    for binding in bindings {
        let provider_id = match protocol::unhex::<32>(&binding.provider_id) {
            Ok(bytes) => ProviderId(bytes),
            Err(_) => {
                eprintln!(
                    "provider: credentials contain an invalid provider_id for {}",
                    binding.name
                );
                continue;
            }
        };
        eprintln!(
            "provider: starting attach loop for {} ({})",
            binding.name,
            protocol::hex(&provider_id.0)
        );
        let credentials = credentials.clone();
        let info = info.clone();
        let dir = dir.clone();
        let state = Arc::clone(&state);
        handles.push(thread::spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    eprintln!("provider: cannot build the attach runtime: {error}");
                    return;
                }
            };
            runtime.block_on(
                ProviderLoop {
                    dir,
                    info,
                    provider_id,
                    credentials,
                    state,
                }
                .run(),
            );
        }));
    }
    handles
}

struct ProviderLoop {
    dir: PathBuf,
    info: ConnectionInfo,
    provider_id: ProviderId,
    credentials: Credentials,
    state: Arc<Mutex<State>>,
}

impl ProviderLoop {
    async fn run(mut self) {
        let mut failures = 0u32;
        loop {
            if self.credentials.needs_refresh(auth::now_seconds())
                && let Err(message) = self.refresh().await
            {
                eprintln!("provider: {message}");
            }
            let access = match self.credentials.access_bytes() {
                Ok(access) => access,
                Err(message) => {
                    eprintln!("provider: {message}");
                    return;
                }
            };
            match ProviderClient::connect(self.info.clone(), access, self.provider_id).await {
                Ok(mut client) => {
                    failures = 0;
                    eprintln!("provider: attached");
                    if let Err(error) = self.serve(&mut client).await {
                        eprintln!("provider: connection ended: {error}");
                    }
                    let _ = client.close().await;
                }
                Err(error) => {
                    eprintln!("provider: attach failed: {error}");
                    if matches!(error, ClientError::Http(401)) {
                        if let Err(message) = self.refresh().await {
                            eprintln!("provider: {message}");
                        }
                    }
                }
            }
            let delay = reconnect_delay(failures);
            failures = failures.saturating_add(1);
            eprintln!("provider: reconnecting in {delay:?}");
            sleep(delay).await;
        }
    }

    async fn serve(&self, client: &mut ProviderClient) -> Result<(), ClientError> {
        loop {
            match client.next().await? {
                ProviderEvent::Invocation { invocation, params } => {
                    let outcome = {
                        let state = self.state.lock().expect("state lock");
                        dispatch(&state, &params.provider, &params.input)
                    };
                    match outcome {
                        Ok(output) => {
                            client
                                .succeed(
                                    invocation,
                                    output.data,
                                    Some(Display {
                                        media_type: output.media_type.to_owned(),
                                        text: output.display,
                                    }),
                                )
                                .await?;
                        }
                        Err(failure) => client.fail(invocation, failure).await?,
                    }
                }
                ProviderEvent::Cancel { invocation } => {
                    // Wiki providers are synchronous snapshot reads, so no
                    // partial result is left to cancel by the time the next
                    // event is read. Consume the notification explicitly so a
                    // later reconnection never replays canceled work.
                    eprintln!(
                        "provider: cancellation received for {}",
                        protocol::hex(&invocation.0)
                    );
                }
                ProviderEvent::Closed => return Ok(()),
            }
        }
    }

    /// Refresh and persist a rotated token pair before the next attach. The
    /// stored credentials are only replaced after the new pair is on disk.
    async fn refresh(&mut self) -> Result<(), String> {
        let origin = self.credentials.oauth_origin.clone().ok_or_else(|| {
            format!(
                "the node published no OAuth origin; run `wiki-server connect --node {}` again",
                self.dir.display()
            )
        })?;
        let root = auth::load_oauth_root(&self.dir)?;
        let oauth = OAuthTokenClient::new(&origin, &root)
            .map_err(|error| format!("cannot configure the node OAuth client: {error}"))?;
        let tokens = oauth
            .refresh(&self.credentials.client_id, &self.credentials.refresh_token)
            .await
            .map_err(|error| auth::refresh_failure_message(&self.dir, &error))?;
        let mut updated = self.credentials.clone();
        updated.apply_refresh(tokens, auth::now_seconds());
        updated
            .save(&self.dir)
            .map_err(|message| format!("refreshed credentials could not be saved: {message}"))?;
        self.credentials = updated;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn manifest_defaults_to_outline_and_rejects_unknown_names() {
        let outline = registration(DEFAULT_PROVIDER_NAME).unwrap();
        assert_eq!(outline.name, "wiki/outline");
        assert_eq!(outline.contract_version, CONTRACT_VERSION);
        assert!(outline.input_schema.is_object());
        assert!(outline.output_schema.is_object());
        assert_eq!(
            outline.default_query,
            Some(DefaultQuery {
                kinds: vec!["wiki".to_owned()]
            })
        );

        let transclude = registration("wiki/transclude").unwrap();
        assert_eq!(transclude.input_schema["properties"]["depth"]["maximum"], 3);
        assert!(
            registration("wiki/nope")
                .unwrap_err()
                .contains("unsupported Wiki provider")
        );
        assert!(registration("../escape").is_err());
    }

    #[test]
    fn connect_choice_keeps_all_three_names_and_the_registered_name() {
        let registration = registration_for_connect(true, false, Some("wiki/transclude")).unwrap();
        let name = registration.as_ref().map(|manifest| manifest.name.as_str());
        assert_eq!(
            requested_provide_names(name, &[]).unwrap(),
            NAMES.map(str::to_owned).to_vec()
        );

        // A caller-provided list keeps its order and gains the registered name.
        assert_eq!(
            requested_provide_names(
                Some("wiki/transclude"),
                &["wiki/outline".to_owned(), "admin/inspect".to_owned()]
            )
            .unwrap(),
            ["wiki/outline", "admin/inspect", "wiki/transclude"]
        );

        assert_eq!(
            requested_provide_names(Some("wiki/outline"), &["wiki/outline".to_owned()]).unwrap(),
            ["wiki/outline"]
        );
        assert!(
            registration_for_connect(false, false, None)
                .unwrap()
                .is_none()
        );
        assert!(
            registration_for_connect(true, true, Some("wiki/outline"))
                .unwrap()
                .is_none()
        );
        assert!(registration_for_connect(true, false, Some("wiki/nope")).is_err());
    }

    #[test]
    fn websocket_dispatch_matches_local_provider_call() {
        let state = State::load(None).unwrap();
        for name in NAMES {
            let input = json!({"id": "p-home", "depth": 2});
            let local = provider::call(&state, name, &input).unwrap();
            let attached = dispatch(&state, name, &input).unwrap();
            assert_eq!(local["data"], attached.data, "{name} data drifted");
            assert_eq!(local["display"], attached.display, "{name} display drifted");
            assert_eq!(
                local["media_type"], attached.media_type,
                "{name} media type drifted"
            );
        }
    }

    #[test]
    fn dispatch_turns_local_errors_into_provider_failures() {
        let state = State::load(None).unwrap();
        let unknown = dispatch(&state, "wiki/nope", &json!({"id": "p-home"})).unwrap_err();
        assert_eq!(unknown.code, "not_found");
        assert!(unknown.message.contains("unknown Wiki provider"));

        let missing_id = dispatch(&state, "wiki/outline", &json!({})).unwrap_err();
        assert_eq!(missing_id.code, "invalid_input");
        assert!(missing_id.message.contains("id"));
    }

    #[test]
    fn completion_extracts_and_checks_the_provider_id() {
        let id = "11".repeat(32);
        let completion = json!({
            "provider": {"provider_id": id, "name": "wiki/outline"}
        });
        assert_eq!(
            completion_provider_id(&completion, Some("wiki/outline"))
                .unwrap()
                .unwrap(),
            ProviderId([0x11; 32])
        );
        assert!(
            completion_provider_id(&completion, Some("wiki/backlinks"))
                .unwrap_err()
                .contains("expected")
        );
        assert!(
            completion_provider_id(&json!({"provider": null}), None)
                .unwrap()
                .is_none()
        );
        assert!(completion_provider_id(&json!({}), None).unwrap().is_none());
        assert!(completion_provider_id(&json!({"provider": {}}), None).is_err());
    }

    #[test]
    fn connect_defaults_to_all_three_manifests() {
        let names = registrations_for_connect(true, false, None)
            .unwrap()
            .into_iter()
            .map(|manifest| manifest.name)
            .collect::<Vec<_>>();
        assert_eq!(names, NAMES.map(str::to_owned).to_vec());
        assert_eq!(
            registrations_for_connect(true, false, Some("wiki/transclude"))
                .unwrap()
                .len(),
            1
        );
        assert!(
            registrations_for_connect(false, false, None)
                .unwrap()
                .is_empty()
        );
        assert!(
            registrations_for_connect(true, true, None)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn completion_returns_every_provider() {
        let completion = json!({
            "providers": [
                {"provider_id": "11".repeat(32), "name": "wiki/backlinks"},
                {"provider_id": "22".repeat(32), "name": "wiki/outline"},
                {"provider_id": "33".repeat(32), "name": "wiki/transclude"}
            ]
        });
        let expected = NAMES.map(str::to_owned).to_vec();
        let bindings = completion_providers(&completion, &expected).unwrap();
        assert_eq!(bindings.len(), 3);
        assert_eq!(bindings[1].name, "wiki/outline");
        assert_eq!(bindings[2].provider_id, "33".repeat(32));
        let legacy = json!({"provider": {"provider_id": "44".repeat(32), "name": "wiki/outline"}});
        assert_eq!(completion_providers(&legacy, &[]).unwrap().len(), 1);
        assert!(completion_providers(&json!({}), &[]).unwrap().is_empty());
    }

    #[test]
    fn reconnect_delay_is_exponential_and_bounded() {
        assert_eq!(reconnect_delay(0), Duration::from_millis(100));
        assert_eq!(reconnect_delay(1), Duration::from_millis(200));
        assert_eq!(reconnect_delay(2), Duration::from_millis(400));
        assert_eq!(
            reconnect_delay(9),
            Duration::from_millis(MAX_RECONNECT_MILLIS)
        );
        assert_eq!(
            reconnect_delay(u32::MAX),
            Duration::from_millis(MAX_RECONNECT_MILLIS)
        );
    }
}
