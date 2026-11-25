//! JSON operations behind the SDK's JNI surface.
//!
//! Handles belong to one connected App API endpoint and one credential. The
//! Kotlin SDK owns token storage and UI; this layer owns protocol correctness,
//! certificate pinning and the subscription cache.
use crate::spec;
use harmion_client::{Client, OAuthTokenClient, Subscription, SubscriptionNotification};
use harmion_protocol::{
    ConnectionInfo,
    connect::{
        ConnectionApproval, ConnectionCompleteRequest, ConnectionOffer, ConnectionStatusRequest,
    },
    domain::ContentId,
    unhex, unhex_bytes,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

static NEXT_CLIENT: AtomicU64 = AtomicU64::new(1);
static CLIENTS: LazyLock<Mutex<BTreeMap<u64, Arc<ClientHandle>>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

struct ClientHandle {
    tokio: tokio::runtime::Runtime,
    client: Client,
    subscriptions: Mutex<BTreeMap<u64, Subscription>>,
    next_subscription: AtomicU64,
}

fn ok(value: Value) -> String {
    let mut object = match value {
        Value::Object(object) => object,
        other => {
            let mut object = serde_json::Map::new();
            object.insert("value".into(), other);
            object
        }
    };
    object.insert("ok".into(), Value::Bool(true));
    Value::Object(object).to_string()
}

fn error_reply(code: &str, message: impl Into<String>) -> String {
    json!({"ok": false, "error": {"code": code, "message": message.into()}}).to_string()
}

fn failure(error: &harmion_client::Error) -> String {
    use harmion_client::Error as E;
    let code = match error {
        E::Io(_) => "io".to_owned(),
        E::InvalidDiscovery => "invalid_discovery".to_owned(),
        E::InvalidRequest => "invalid_request".to_owned(),
        E::InvalidResponse => "invalid_response".to_owned(),
        E::Transport => "transport".to_owned(),
        E::Timeout => "timeout".to_owned(),
        E::Http(status) => format!("http_{status}"),
        E::OAuth(code) => format!("oauth_{code}"),
        E::ProviderAttachRejected { code } => format!("provider_rejected_{code}"),
        E::Signer(_) => "signer".to_owned(),
    };
    error_reply(&code, error.to_string())
}

fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("harmion-sdk")
        .build()
        .map_err(|error| error.to_string())
}

fn with_client<T>(
    handle: u64,
    operation: impl FnOnce(&ClientHandle) -> Result<T, String>,
) -> Result<T, String> {
    let client = CLIENTS
        .lock()
        .expect("client registry")
        .get(&handle)
        .cloned()
        .ok_or_else(|| error_reply("unknown_handle", format!("no client for handle {handle}")))?;
    operation(&client)
}

/// Connects with a credential (hex 32 bytes) to one loopback App API endpoint.
pub fn connect(info: &str, credential_hex: &str) -> String {
    let info: ConnectionInfo = match serde_json::from_str(info) {
        Ok(info) => info,
        Err(error) => return error_reply("invalid_connection", error.to_string()),
    };
    let credential: [u8; 32] = match unhex(credential_hex) {
        Ok(credential) => credential,
        Err(_) => return error_reply("invalid_credential", "credential must be 32 hex bytes"),
    };
    let client = match Client::new(info, credential) {
        Ok(client) => client,
        Err(error) => return failure(&error),
    };
    let tokio = match runtime() {
        Ok(runtime) => runtime,
        Err(message) => return error_reply("io", message),
    };
    let handle = NEXT_CLIENT.fetch_add(1, Ordering::Relaxed);
    CLIENTS.lock().expect("client registry").insert(
        handle,
        Arc::new(ClientHandle {
            tokio,
            client,
            subscriptions: Mutex::new(BTreeMap::new()),
            next_subscription: AtomicU64::new(1),
        }),
    );
    ok(json!({"handle": handle}))
}

/// Unauthenticated client for connect-start/status/complete.
pub fn anonymous(info: &str) -> String {
    let info: ConnectionInfo = match serde_json::from_str(info) {
        Ok(info) => info,
        Err(error) => return error_reply("invalid_connection", error.to_string()),
    };
    let client = match Client::anonymous(info) {
        Ok(client) => client,
        Err(error) => return failure(&error),
    };
    let tokio = match runtime() {
        Ok(runtime) => runtime,
        Err(message) => return error_reply("io", message),
    };
    let handle = NEXT_CLIENT.fetch_add(1, Ordering::Relaxed);
    CLIENTS.lock().expect("client registry").insert(
        handle,
        Arc::new(ClientHandle {
            tokio,
            client,
            subscriptions: Mutex::new(BTreeMap::new()),
            next_subscription: AtomicU64::new(1),
        }),
    );
    ok(json!({"handle": handle}))
}

pub fn close(handle: u64) -> String {
    match CLIENTS.lock().expect("client registry").remove(&handle) {
        Some(_) => ok(json!({"closed": true})),
        None => ok(json!({"closed": false})),
    }
}

/// Escape hatch for admin operations that are not wrapped individually. The
/// body is raw canonical bytes; the response is JSON.
pub fn call_raw(handle: u64, operation: &str, body_hex: &str) -> String {
    let body = match unhex_bytes(body_hex, 1024 * 1024) {
        Ok(body) => body,
        Err(_) => return error_reply("invalid_body", "body must be hex"),
    };
    match with_client(handle, |client| {
        client
            .tokio
            .block_on(client.client.call(operation, &body))
            .map_err(|error| failure(&error))
    }) {
        Ok(value) => ok(json!({"response": value})),
        Err(reply) => reply,
    }
}

pub fn save(handle: u64, request_json: &str) -> String {
    let request = match spec::save_request(request_json) {
        Ok(request) => request,
        Err(message) => return error_reply("invalid_request", message),
    };
    match with_client(handle, |client| {
        client
            .tokio
            .block_on(client.client.save(&request))
            .map_err(|error| failure(&error))
    }) {
        Ok(value) => ok(json!({"response": value})),
        Err(reply) => reply,
    }
}

pub fn get(handle: u64, content_hex: &str) -> String {
    let content: [u8; 32] = match unhex(content_hex) {
        Ok(content) => content,
        Err(_) => return error_reply("invalid_content_id", "content id must be 32 hex bytes"),
    };
    match with_client(handle, |client| {
        client
            .tokio
            .block_on(client.client.get(ContentId(content)))
            .map_err(|error| failure(&error))
    }) {
        Ok(value) => ok(json!({"response": value})),
        Err(reply) => reply,
    }
}

pub fn query(handle: u64, condition_json: &str, limit: u32) -> String {
    let condition = match spec::condition_from_str(condition_json) {
        Ok(condition) => condition,
        Err(message) => return error_reply("invalid_condition", message),
    };
    let limit = limit.clamp(1, 32);
    match with_client(handle, |client| {
        client
            .tokio
            .block_on(client.client.query(condition, limit))
            .map_err(|error| failure(&error))
    }) {
        Ok(page) => ok(json!({
            "items": page.items,
            "next": page.next.map(|cursor| harmion_protocol::hex(&cursor.0)),
        })),
        Err(reply) => reply,
    }
}

pub fn subscribe(handle: u64, condition_json: &str) -> String {
    let condition = match spec::condition_from_str(condition_json) {
        Ok(condition) => condition,
        Err(message) => return error_reply("invalid_condition", message),
    };
    match with_client(handle, |client| {
        let subscription = client
            .tokio
            .block_on(client.client.subscribe(condition))
            .map_err(|error| failure(&error))?;
        let id = client.next_subscription.fetch_add(1, Ordering::Relaxed);
        client
            .subscriptions
            .lock()
            .expect("subscriptions")
            .insert(id, subscription);
        Ok(id)
    }) {
        Ok(subscription) => ok(json!({"subscription": subscription})),
        Err(reply) => reply,
    }
}

fn contents(
    subscription: &Subscription,
    only: Option<&[ContentId]>,
) -> serde_json::Map<String, Value> {
    let mut map = serde_json::Map::new();
    let Some(entries) = subscription.contents() else {
        return map;
    };
    for (content, value) in entries {
        if only.is_some_and(|only| !only.contains(content)) {
            continue;
        }
        map.insert(harmion_protocol::hex(&content.0), value.clone());
    }
    map
}

pub fn subscription_next(handle: u64, subscription: u64) -> String {
    let taken = match with_client(handle, |client| {
        client
            .subscriptions
            .lock()
            .expect("subscriptions")
            .remove(&subscription)
            .ok_or_else(|| error_reply("unknown_subscription", "no such subscription"))
    }) {
        Ok(subscription) => subscription,
        Err(reply) => return reply,
    };
    let result = with_client(handle, |client| {
        let mut state = taken;
        let notification = client
            .tokio
            .block_on(client.client.subscription_next(&mut state))
            .map_err(|error| failure(&error));
        let reply = match &notification {
            Ok(SubscriptionNotification::SnapshotChunk) => ok(json!({"kind": "snapshot_chunk"})),
            Ok(SubscriptionNotification::SnapshotEnd) => ok(json!({
                "kind": "snapshot_end",
                "contents": contents(&state, None),
            })),
            Ok(SubscriptionNotification::Changes { upserted, removed }) => ok(json!({
                "kind": "changes",
                "upserted": upserted.iter().map(|id| harmion_protocol::hex(&id.0)).collect::<Vec<_>>(),
                "removed": removed.iter().map(|id| harmion_protocol::hex(&id.0)).collect::<Vec<_>>(),
                "contents": contents(&state, Some(upserted)),
            })),
            Ok(SubscriptionNotification::Idle) => ok(json!({"kind": "idle"})),
            Ok(SubscriptionNotification::ResetRequired) => ok(json!({"kind": "reset_required"})),
            Err(reply) => reply.clone(),
        };
        client
            .subscriptions
            .lock()
            .expect("subscriptions")
            .insert(subscription, state);
        Ok(reply)
    });
    match result {
        Ok(reply) => reply,
        Err(reply) => reply,
    }
}

pub fn unsubscribe(handle: u64, subscription: u64) -> String {
    let taken = match with_client(handle, |client| {
        Ok(client
            .subscriptions
            .lock()
            .expect("subscriptions")
            .remove(&subscription))
    }) {
        Ok(subscription) => subscription,
        Err(reply) => return reply,
    };
    let Some(mut subscription) = taken else {
        return ok(json!({"unsubscribed": false}));
    };
    match with_client(handle, |client| {
        client
            .tokio
            .block_on(client.client.unsubscribe(&mut subscription))
            .map_err(|error| failure(&error))
    }) {
        Ok(()) => ok(json!({"unsubscribed": true})),
        Err(reply) => reply,
    }
}

pub fn connect_start(handle: u64, offer_json: &str) -> String {
    let offer: ConnectionOffer = match serde_json::from_str(offer_json) {
        Ok(offer) => offer,
        Err(error) => return error_reply("invalid_offer", error.to_string()),
    };
    match with_client(handle, |client| {
        client
            .tokio
            .block_on(client.client.connect_start(&offer))
            .map_err(|error| failure(&error))
    }) {
        Ok(value) => ok(json!({"response": value})),
        Err(reply) => reply,
    }
}

pub fn connect_status(handle: u64, request_json: &str) -> String {
    let request: ConnectionStatusRequest = match serde_json::from_str(request_json) {
        Ok(request) => request,
        Err(error) => return error_reply("invalid_request", error.to_string()),
    };
    match with_client(handle, |client| {
        client
            .tokio
            .block_on(client.client.connect_status(&request))
            .map_err(|error| failure(&error))
    }) {
        Ok(value) => ok(json!({"response": value})),
        Err(reply) => reply,
    }
}

pub fn connect_complete(handle: u64, request_json: &str) -> String {
    let request: ConnectionCompleteRequest = match serde_json::from_str(request_json) {
        Ok(request) => request,
        Err(error) => return error_reply("invalid_request", error.to_string()),
    };
    match with_client(handle, |client| {
        client
            .tokio
            .block_on(client.client.connect_complete(&request))
            .map_err(|error| failure(&error))
    }) {
        Ok(value) => ok(json!({"response": value})),
        Err(reply) => reply,
    }
}

pub fn connect_list(handle: u64) -> String {
    match with_client(handle, |client| {
        client
            .tokio
            .block_on(client.client.connect_list())
            .map_err(|error| failure(&error))
    }) {
        Ok(value) => ok(json!({"response": value})),
        Err(reply) => reply,
    }
}

pub fn connect_approve(handle: u64, approval_json: &str) -> String {
    let approval: ConnectionApproval = match serde_json::from_str(approval_json) {
        Ok(approval) => approval,
        Err(error) => return error_reply("invalid_request", error.to_string()),
    };
    match with_client(handle, |client| {
        client
            .tokio
            .block_on(client.client.connect_approve(&approval))
            .map_err(|error| failure(&error))
    }) {
        Ok(value) => ok(json!({"response": value})),
        Err(reply) => reply,
    }
}

pub fn sync_status(handle: u64, update_hex: &str) -> String {
    let update = if update_hex.trim().is_empty() {
        None
    } else {
        match unhex::<32>(update_hex) {
            Ok(update) => Some(harmion_protocol::domain::UpdateId(update)),
            Err(_) => return error_reply("invalid_update_id", "update id must be 32 hex bytes"),
        }
    };
    match with_client(handle, |client| {
        client
            .tokio
            .block_on(client.client.sync_status(update))
            .map_err(|error| failure(&error))
    }) {
        Ok(value) => match serde_json::to_value(value) {
            Ok(value) => ok(json!({"response": value})),
            Err(error) => error_reply("serialize", error.to_string()),
        },
        Err(reply) => reply,
    }
}

/// Exchange a refresh token against the node's OAuth origin.
pub fn oauth_refresh(origin: &str, root_hex: &str, client_id: &str, refresh_token: &str) -> String {
    let root = match unhex_bytes(root_hex, 65536) {
        Ok(root) => root,
        Err(_) => return error_reply("invalid_root", "root must be hex"),
    };
    let client = match OAuthTokenClient::new(origin, &root) {
        Ok(client) => client,
        Err(error) => return failure(&error),
    };
    let tokio = match runtime() {
        Ok(runtime) => runtime,
        Err(message) => return error_reply("io", message),
    };
    match tokio.block_on(client.refresh(client_id, refresh_token)) {
        Ok(tokens) => ok(json!({
            "access_token": tokens.access_token,
            "refresh_token": tokens.refresh_token,
            "expires_in": tokens.expires_in,
            "scope": tokens.scope,
        })),
        Err(error) => failure(&error),
    }
}

/// Random bytes for request ids, PKCE verifiers and connection ids.
pub fn random_hex(bytes: u32) -> String {
    let bytes = bytes.clamp(1, 64) as usize;
    let mut output = Vec::with_capacity(bytes);
    while output.len() < bytes {
        match harmion_client::random_id() {
            Ok(random) => output.extend_from_slice(&random),
            Err(_) => return error_reply("entropy", "random source unavailable"),
        }
    }
    output.truncate(bytes);
    ok(json!({"hex": harmion_protocol::hex(&output)}))
}

/// Requests an owner challenge for an operation. `target` and `session` are
/// optional 32-byte hex ids; the body is hex. The returned statement is what the
/// owner signs.
pub fn owner_challenge(
    handle: u64,
    operation: &str,
    target_hex: &str,
    body_hex: &str,
    session_hex: &str,
) -> String {
    let body = match unhex_bytes(body_hex, 1024 * 1024) {
        Ok(body) => body,
        Err(_) => return error_reply("invalid_body", "body must be hex"),
    };
    let target = match parse_optional_id(target_hex) {
        Ok(target) => target,
        Err(_) => return error_reply("invalid_id", "target must be 32 hex bytes or empty"),
    };
    let session = match parse_optional_id(session_hex) {
        Ok(session) => session,
        Err(_) => return error_reply("invalid_id", "session must be 32 hex bytes or empty"),
    };
    let mut request = serde_json::Map::new();
    request.insert("operation".into(), json!(operation));
    if let Some(target) = target {
        request.insert("target".into(), json!(harmion_protocol::hex(&target)));
    }
    request.insert(
        "args_sha256".into(),
        json!(harmion_protocol::hex(&harmion_protocol::owner::args_hash(
            &body
        ))),
    );
    if let Some(session) = session {
        request.insert("session".into(), json!(harmion_protocol::hex(&session)));
    }
    let request = match serde_json::to_vec(&Value::Object(request)) {
        Ok(request) => request,
        Err(error) => return error_reply("invalid_request", error.to_string()),
    };
    match with_client(handle, |client| {
        client
            .tokio
            .block_on(client.client.call("owner-challenge", &request))
            .map_err(|error| failure(&error))
    }) {
        Ok(value) => ok(json!({"response": value})),
        Err(reply) => reply,
    }
}

/// Calls an operation with an owner assertion (armored SSHSIG).
pub fn call_with_owner(
    handle: u64,
    operation: &str,
    body_hex: &str,
    challenge_hex: &str,
    signature_pem: &str,
) -> String {
    let body = match unhex_bytes(body_hex, 1024 * 1024) {
        Ok(body) => body,
        Err(_) => return error_reply("invalid_body", "body must be hex"),
    };
    let challenge: [u8; 32] = match unhex(challenge_hex) {
        Ok(challenge) => challenge,
        Err(_) => return error_reply("invalid_challenge", "challenge must be 32 hex bytes"),
    };
    if signature_pem.is_empty()
        || signature_pem.len() > harmion_protocol::owner::MAX_SSH_SIGNATURE_BYTES
    {
        return error_reply("invalid_signature", "signature must be an armored SSHSIG");
    }
    let assertion = harmion_client::OwnerAssertion {
        challenge,
        signature: signature_pem.as_bytes().to_vec(),
    };
    match with_client(handle, |client| {
        client
            .tokio
            .block_on(client.client.call_with(operation, &body, Some(&assertion)))
            .map_err(|error| failure(&error))
    }) {
        Ok(value) => ok(json!({"response": value})),
        Err(reply) => reply,
    }
}

fn parse_optional_id(text: &str) -> Result<Option<[u8; 32]>, ()> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    unhex::<32>(text).map(Some).map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_handles_and_bad_inputs_are_json_errors() {
        assert_eq!(
            serde_json::from_str::<Value>(&call_raw(9999, "get", "")).unwrap()["error"]["code"],
            "unknown_handle"
        );
        assert_eq!(
            serde_json::from_str::<Value>(&get(9999, "zz")).unwrap()["error"]["code"],
            "invalid_content_id"
        );
        assert_eq!(
            serde_json::from_str::<Value>(&connect("{}", "00")).unwrap()["error"]["code"],
            "invalid_connection"
        );
        assert_eq!(
            serde_json::from_str::<Value>(&oauth_refresh("https://example.com", "00", "aa", "bb"))
                .unwrap()["error"]["code"],
            "invalid_discovery"
        );
        assert!(random_hex(32).contains("\"hex\""));
    }

    #[test]
    fn query_limits_are_clamped_before_authentication() {
        // The condition parses, then the handle lookup fails: clamp happens first
        // so a bad limit never reaches the network.
        let reply = query(9999, r#"{"type":"and","conditions":[]}"#, 999);
        assert_eq!(
            serde_json::from_str::<Value>(&reply).unwrap()["error"]["code"],
            "unknown_handle"
        );
        assert_eq!(
            serde_json::from_str::<Value>(&query(9999, "nope", 1)).unwrap()["error"]["code"],
            "invalid_condition"
        );
    }
}

/// Returns the conflict view (edit token and candidate groups) for a content.
pub fn conflicts(handle: u64, content_hex: &str) -> String {
    let content: [u8; 32] = match unhex(content_hex) {
        Ok(content) => content,
        Err(_) => return error_reply("invalid_content_id", "content id must be 32 hex bytes"),
    };
    match with_client(handle, |client| {
        client
            .tokio
            .block_on(client.client.call("conflicts", &content))
            .map_err(|error| failure(&error))
    }) {
        Ok(value) => ok(json!({"response": value})),
        Err(reply) => reply,
    }
}

fn merge_request(
    content_hex: &str,
    token_hex: &str,
    left_hex: &str,
    right_hex: &str,
    base_hex: &str,
) -> Result<harmion_protocol::resolution::MergeInputRequest, String> {
    let content = unhex::<32>(content_hex).map_err(|_| "content id must be 32 hex bytes")?;
    let token = unhex::<32>(token_hex).map_err(|_| "token must be 32 hex bytes")?;
    let left = unhex::<32>(left_hex).map_err(|_| "left version must be 32 hex bytes")?;
    let right = unhex::<32>(right_hex).map_err(|_| "right version must be 32 hex bytes")?;
    let part = if base_hex.trim().is_empty() {
        harmion_protocol::resolution::MergePart::Base(None)
    } else {
        let base = unhex::<32>(base_hex).map_err(|_| "base version must be 32 hex bytes")?;
        harmion_protocol::resolution::MergePart::Base(Some(harmion_domain::UpdateId(base)))
    };
    Ok(harmion_protocol::resolution::MergeInputRequest {
        content: harmion_domain::ContentId(content),
        token: harmion_protocol::EditToken(token),
        left: harmion_domain::UpdateId(left),
        right: harmion_domain::UpdateId(right),
        part,
    })
}

pub fn merge_input(
    handle: u64,
    content_hex: &str,
    token_hex: &str,
    left_hex: &str,
    right_hex: &str,
    base_hex: &str,
) -> String {
    let request = match merge_request(content_hex, token_hex, left_hex, right_hex, base_hex) {
        Ok(request) => request,
        Err(message) => return error_reply("invalid_request", message),
    };
    match with_client(handle, |client| {
        client
            .tokio
            .block_on(client.client.merge_input(request))
            .map_err(|error| failure(&error))
    }) {
        Ok(value) => ok(json!({"response": value})),
        Err(reply) => reply,
    }
}

pub fn merge_plan(
    handle: u64,
    content_hex: &str,
    token_hex: &str,
    left_hex: &str,
    right_hex: &str,
    base_hex: &str,
) -> String {
    let request = match merge_request(content_hex, token_hex, left_hex, right_hex, base_hex) {
        Ok(request) => request,
        Err(message) => return error_reply("invalid_request", message),
    };
    match with_client(handle, |client| {
        client
            .tokio
            .block_on(client.client.merge_plan(request))
            .map_err(|error| failure(&error))
    }) {
        Ok(value) => ok(json!({"response": value})),
        Err(reply) => reply,
    }
}
