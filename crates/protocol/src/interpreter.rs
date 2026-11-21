//! Interpreter registration, call, and Provider JSON-RPC contracts.
//!
//! The registry is local to one node. A Provider receives only the invocation
//! input that the calling application selected; it never receives an edit token
//! or permission to read Harmion itself.
use crate::{CodecError, EditToken, decode_app_json, hex};
use harmion_domain::{AttributeValue, Content, ContentId};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use serde_json::{Value as Json, json};
use std::fmt;

/// The only Provider contract version accepted by this node.
pub const CONTRACT_VERSION: u32 = 1;

pub const MAX_PROVIDER_NAME_BYTES: usize = 128;
pub const MAX_KIND_BYTES: usize = 256;
pub const MAX_PROVIDER_SCHEMA_BYTES: usize = 64 * 1024;
pub const MAX_DEFAULT_QUERY_KINDS: usize = 32;
pub const MAX_REGISTRATION_BYTES: usize = 160 * 1024;
pub const MAX_CALL_REQUEST_BYTES: usize = 1024 * 1024;
pub const MAX_CALL_CONTENTS: usize = 64;
pub const MAX_CALL_INPUT_BYTES: usize = 128 * 1024;
pub const MIN_CALL_TIMEOUT_MS: u64 = 100;
pub const MAX_CALL_TIMEOUT_MS: u64 = 30_000;
pub const DEFAULT_CALL_TIMEOUT_MS: u64 = 10_000;
pub const MAX_PROVIDER_DATA_BYTES: usize = 1024 * 1024;
pub const MAX_PROVIDER_DETAILS_BYTES: usize = 256 * 1024;
pub const MAX_DISPLAY_TEXT_BYTES: usize = 64 * 1024;
pub const MAX_DISPLAY_MEDIA_TYPE_BYTES: usize = 128;
pub const MAX_PROVIDER_ERROR_CODE_BYTES: usize = 64;
pub const MAX_PROVIDER_ERROR_MESSAGE_BYTES: usize = 1024;
/// Provider WebSocket frames are bounded independently of the App response cap.
pub const MAX_PROVIDER_MESSAGE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_ACTIVE_INVOCATIONS: usize = 32;
pub const MAX_ACTIVE_INVOCATIONS_PER_APP: usize = 8;

pub const RPC_VERSION: &str = "2.0";
pub const RPC_ATTACH: &str = "interpreter.attach";
pub const RPC_INVOKE: &str = "interpreter.invoke";
pub const RPC_CANCEL: &str = "interpreter.cancel";

pub const RPC_PARSE_ERROR: i64 = -32700;
pub const RPC_INVALID_REQUEST: i64 = -32600;
pub const RPC_METHOD_NOT_FOUND: i64 = -32601;
pub const RPC_INVALID_PARAMS: i64 = -32602;
pub const RPC_INTERNAL_ERROR: i64 = -32603;
/// Attach was refused because the registration is pending or superseded.
pub const RPC_NOT_APPROVED: i64 = -32010;
/// The authenticated app does not own the provider registration.
pub const RPC_PROVIDER_MISMATCH: i64 = -32011;
/// The connection already attached to a provider.
pub const RPC_ALREADY_ATTACHED: i64 = -32012;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProviderId(pub [u8; 32]);
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InvocationId(pub [u8; 32]);

macro_rules! hex_id {
    ($ty:ident) => {
        impl Serialize for $ty {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(&hex(&self.0))
            }
        }
        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let text = String::deserialize(deserializer)?;
                crate::unhex(&text).map(Self).map_err(de::Error::custom)
            }
        }
    };
}
hex_id!(ProviderId);
hex_id!(InvocationId);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderStatus {
    Pending,
    Approved,
    Superseded,
}
impl ProviderStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Superseded => "superseded",
        }
    }
    pub fn from_storage(value: i64) -> Option<Self> {
        match value {
            0 => Some(Self::Pending),
            1 => Some(Self::Approved),
            2 => Some(Self::Superseded),
            _ => None,
        }
    }
    pub fn storage(self) -> i64 {
        match self {
            Self::Pending => 0,
            Self::Approved => 1,
            Self::Superseded => 2,
        }
    }
}

/// A Provider's declared input selection. Node never executes it; callers merge
/// it with their own Query condition and pass the chosen snapshots to `call`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DefaultQuery {
    pub kinds: Vec<String>,
}
impl DefaultQuery {
    pub fn condition(&self) -> crate::query::Condition {
        crate::query::Condition::And(
            self.kinds
                .iter()
                .cloned()
                .map(crate::query::Condition::Kind)
                .collect(),
        )
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderRegistration {
    pub name: String,
    pub contract_version: u32,
    pub input_schema: Json,
    pub output_schema: Json,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_query: Option<DefaultQuery>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ProviderSummary {
    pub provider_id: ProviderId,
    pub name: String,
    pub contract_version: u32,
    pub status: &'static str,
    pub connected: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ProviderDescription {
    pub provider_id: ProviderId,
    pub name: String,
    pub contract_version: u32,
    pub status: &'static str,
    pub connected: bool,
    pub input_schema: Json,
    pub output_schema: Json,
    pub default_query: Option<DefaultQuery>,
}

/// One caller-selected Content basis. The token pins the snapshot; it is
/// consumed by the node and never forwarded to the Provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContentSelection {
    pub content: ContentId,
    pub token: EditToken,
}
impl Serialize for ContentSelection {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("ContentSelection", 2)?;
        state.serialize_field("id", &hex(&self.content.0))?;
        state.serialize_field("token", &hex(&self.token.0))?;
        state.end()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CallRequest {
    pub provider: String,
    pub input: Json,
    pub contents: Vec<ContentSelection>,
    pub timeout_ms: u64,
}

/// Provider-visible Content. Only the normal value selected by the caller is
/// sent; conflict details and edit tokens remain on the node.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderContent {
    pub content: Json,
    pub conflict: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Display {
    pub media_type: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderFailure {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub details: Option<Json>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ProviderOutcome {
    Success {
        data: Json,
        display: Option<Display>,
    },
    Error(ProviderFailure),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttachParams {
    pub provider_id: ProviderId,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InvokeParams {
    pub invocation_id: InvocationId,
    pub provider: String,
    pub contract_version: u32,
    pub input: Json,
    pub contents: Vec<ProviderContent>,
}

pub fn validate_provider_name(name: &str) -> Result<(), CodecError> {
    if name.is_empty()
        || name.len() > MAX_PROVIDER_NAME_BYTES
        || name.starts_with('/')
        || name.ends_with('/')
        || name.contains("//")
        || !name.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'.' | b'_' | b'-' | b'/')
        })
    {
        return Err(CodecError::InvalidFieldValue);
    }
    Ok(())
}

pub fn validate_default_query(query: &DefaultQuery) -> Result<(), CodecError> {
    if query.kinds.is_empty() || query.kinds.len() > MAX_DEFAULT_QUERY_KINDS {
        return Err(CodecError::InvalidFieldValue);
    }
    let mut seen = std::collections::BTreeSet::new();
    for kind in &query.kinds {
        if kind.is_empty() || kind.len() > MAX_KIND_BYTES || kind.chars().any(char::is_control) {
            return Err(CodecError::InvalidFieldValue);
        }
        if !seen.insert(kind) {
            return Err(CodecError::Duplicate);
        }
    }
    Ok(())
}

pub fn validate_registration(registration: &ProviderRegistration) -> Result<(), CodecError> {
    validate_provider_name(&registration.name)?;
    if registration.contract_version != CONTRACT_VERSION {
        return Err(CodecError::UnsupportedVersion(
            registration.contract_version,
        ));
    }
    if !registration.input_schema.is_object() || !registration.output_schema.is_object() {
        return Err(CodecError::InvalidFieldValue);
    }
    let schema_size = |value: &Json| {
        serde_json::to_vec(value)
            .map_err(|_| CodecError::InvalidJson)
            .and_then(|bytes| {
                if bytes.len() > MAX_PROVIDER_SCHEMA_BYTES {
                    Err(CodecError::TooLarge)
                } else {
                    Ok(())
                }
            })
    };
    schema_size(&registration.input_schema)?;
    schema_size(&registration.output_schema)?;
    if let Some(query) = &registration.default_query {
        validate_default_query(query)?;
    }
    Ok(())
}

pub fn decode_registration(bytes: &[u8]) -> Result<ProviderRegistration, CodecError> {
    if bytes.len() > MAX_REGISTRATION_BYTES {
        return Err(CodecError::TooLarge);
    }
    let value = decode_app_json(bytes)?;
    let registration: ProviderRegistration =
        serde_json::from_value(value).map_err(|_| CodecError::InvalidJson)?;
    validate_registration(&registration)?;
    Ok(registration)
}

pub fn decode_call_request(bytes: &[u8]) -> Result<CallRequest, CodecError> {
    if bytes.len() > MAX_CALL_REQUEST_BYTES {
        return Err(CodecError::TooLarge);
    }
    let value = decode_app_json(bytes)?;
    let json: CallRequestJson =
        serde_json::from_value(value).map_err(|_| CodecError::InvalidJson)?;
    let timeout_ms = json.timeout_ms.unwrap_or(DEFAULT_CALL_TIMEOUT_MS);
    if !(MIN_CALL_TIMEOUT_MS..=MAX_CALL_TIMEOUT_MS).contains(&timeout_ms) {
        return Err(CodecError::InvalidFieldValue);
    }
    validate_provider_name(&json.provider)?;
    if json.contents.len() > MAX_CALL_CONTENTS {
        return Err(CodecError::TooManyItems);
    }
    validate_json_shape(&json.input)?;
    let input_bytes = serde_json::to_vec(&json.input)
        .map_err(|_| CodecError::InvalidJson)?
        .len();
    if input_bytes > MAX_CALL_INPUT_BYTES {
        return Err(CodecError::TooLarge);
    }
    let mut contents = Vec::with_capacity(json.contents.len());
    let mut seen = std::collections::BTreeSet::new();
    for item in json.contents {
        let content = ContentId(crate::unhex(&item.id).map_err(|_| CodecError::InvalidHex)?);
        let token = EditToken(crate::unhex(&item.token).map_err(|_| CodecError::InvalidHex)?);
        if !seen.insert(content) {
            return Err(CodecError::Duplicate);
        }
        contents.push(ContentSelection { content, token });
    }
    Ok(CallRequest {
        provider: json.provider,
        input: json.input,
        contents,
        timeout_ms,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CallRequestJson {
    provider: String,
    #[serde(default)]
    input: Json,
    #[serde(default)]
    contents: Vec<ContentSelectionJson>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ContentSelectionJson {
    id: String,
    token: String,
}

pub fn decode_attach_params(params: Json) -> Result<AttachParams, CodecError> {
    let params: AttachParams =
        serde_json::from_value(params).map_err(|_| CodecError::InvalidJson)?;
    Ok(params)
}

/// Serialize a Content for a Provider. This is intentionally the same normal
/// value shape as the App API, without an edit token, basis, or candidates.
pub fn provider_content_json(content: &Content) -> Json {
    let attributes: Vec<Json> = content
        .attributes
        .iter()
        .map(|(key, value)| {
            json!({
                "namespace": key.namespace,
                "key": key.key,
                "value": attribute_json(value),
            })
        })
        .collect();
    json!({
        "id": hex(&content.id.0),
        "body": content.body,
        "attributes": attributes,
        "kinds": content.kinds,
    })
}

fn attribute_json(value: &AttributeValue) -> Json {
    match value {
        AttributeValue::Null => Json::Null,
        AttributeValue::Bool(value) => Json::Bool(*value),
        AttributeValue::Number(number) => number
            .to_string()
            .parse::<serde_json::Number>()
            .expect("validated decimal with arbitrary precision")
            .into(),
        AttributeValue::String(value) => Json::String(value.clone()),
        AttributeValue::Array(values) => Json::Array(values.iter().map(attribute_json).collect()),
        AttributeValue::Object(values) => Json::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), attribute_json(value)))
                .collect(),
        ),
    }
}

pub fn decode_provider_result(result: Json) -> Result<ProviderOutcome, CodecError> {
    let object = result.as_object().ok_or(CodecError::InvalidJson)?;
    match object.get("status").and_then(Json::as_str) {
        Some("ok") => {
            let mut data = None;
            let mut display = None;
            for (key, value) in object {
                match key.as_str() {
                    "status" => {}
                    "data" => data = Some(value.clone()),
                    "display" => {
                        if !value.is_null() {
                            let decoded: Display = serde_json::from_value(value.clone())
                                .map_err(|_| CodecError::InvalidJson)?;
                            validate_display(&decoded)?;
                            display = Some(decoded);
                        }
                    }
                    _ => return Err(CodecError::InvalidFieldValue),
                }
            }
            let data = data.ok_or(CodecError::InvalidJson)?;
            check_json_size(&data, MAX_PROVIDER_DATA_BYTES)?;
            Ok(ProviderOutcome::Success { data, display })
        }
        Some("error") => {
            let error = object.get("error").ok_or(CodecError::InvalidJson)?;
            let failure: ProviderFailure =
                serde_json::from_value(error.clone()).map_err(|_| CodecError::InvalidJson)?;
            validate_provider_failure(&failure)?;
            Ok(ProviderOutcome::Error(failure))
        }
        _ => Err(CodecError::InvalidFieldValue),
    }
}

fn check_json_size(value: &Json, limit: usize) -> Result<(), CodecError> {
    let bytes = serde_json::to_vec(value).map_err(|_| CodecError::InvalidJson)?;
    if bytes.len() > limit {
        Err(CodecError::TooLarge)
    } else {
        Ok(())
    }
}

pub fn validate_display(display: &Display) -> Result<(), CodecError> {
    let media_type = display.media_type.as_bytes();
    if media_type.is_empty()
        || media_type.len() > MAX_DISPLAY_MEDIA_TYPE_BYTES
        || !media_type.iter().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'+' | b'-' | b'.' | b'_')
        })
    {
        return Err(CodecError::InvalidFieldValue);
    }
    if display.text.len() > MAX_DISPLAY_TEXT_BYTES {
        return Err(CodecError::TooLarge);
    }
    Ok(())
}

pub fn validate_provider_failure(failure: &ProviderFailure) -> Result<(), CodecError> {
    let code = failure.code.as_bytes();
    if code.is_empty()
        || code.len() > MAX_PROVIDER_ERROR_CODE_BYTES
        || !code.iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-' | b'.')
        })
    {
        return Err(CodecError::InvalidFieldValue);
    }
    if failure.message.len() > MAX_PROVIDER_ERROR_MESSAGE_BYTES {
        return Err(CodecError::TooLarge);
    }
    if let Some(details) = &failure.details {
        check_json_size(details, MAX_PROVIDER_DETAILS_BYTES)?;
    }
    Ok(())
}

pub fn rpc_request(id: &Json, method: &str, params: Json) -> Json {
    json!({"jsonrpc": RPC_VERSION, "id": id, "method": method, "params": params})
}
pub fn rpc_notification(method: &str, params: Json) -> Json {
    json!({"jsonrpc": RPC_VERSION, "method": method, "params": params})
}
pub fn rpc_result(id: &Json, result: Json) -> Json {
    json!({"jsonrpc": RPC_VERSION, "id": id, "result": result})
}
pub fn rpc_error(id: &Json, code: i64, message: &str) -> Json {
    json!({"jsonrpc": RPC_VERSION, "id": id, "error": {"code": code, "message": message}})
}

pub enum RpcMessage {
    Request {
        id: Json,
        method: String,
        params: Json,
    },
    Notification {
        method: String,
        params: Json,
    },
    Response {
        id: Json,
        result: Result<Json, RpcFailure>,
    },
}
#[derive(Clone, Debug, PartialEq)]
pub struct RpcFailure {
    pub code: i64,
    pub message: String,
    pub data: Option<Json>,
}

/// Parse a bounded JSON-RPC message. Provider connections exchange one JSON
/// value per WebSocket text message, so this has no streaming state.
pub fn decode_rpc(bytes: &[u8]) -> Result<RpcMessage, CodecError> {
    if bytes.len() > MAX_PROVIDER_MESSAGE_BYTES {
        return Err(CodecError::TooLarge);
    }
    let value = decode_app_json(bytes)?;
    let object = value.as_object().ok_or(CodecError::InvalidJson)?;
    if object.get("jsonrpc").and_then(Json::as_str) != Some(RPC_VERSION) {
        return Err(CodecError::InvalidFieldValue);
    }
    let has_id = object.contains_key("id");
    let id = object.get("id").cloned().unwrap_or(Json::Null);
    if has_id && !(id.is_string() || id.is_number() || id.is_null()) {
        return Err(CodecError::InvalidFieldValue);
    }
    let method = object.get("method").and_then(Json::as_str);
    let has_result = object.contains_key("result");
    let has_error = object.contains_key("error");
    match (method, has_id, has_result, has_error) {
        (Some(method), true, false, false) => {
            validate_method(method)?;
            let params = object.get("params").cloned().unwrap_or(Json::Null);
            Ok(RpcMessage::Request {
                id,
                method: method.to_owned(),
                params,
            })
        }
        (Some(method), false, false, false) => {
            validate_method(method)?;
            let params = object.get("params").cloned().unwrap_or(Json::Null);
            Ok(RpcMessage::Notification {
                method: method.to_owned(),
                params,
            })
        }
        (None, true, true, false) => Ok(RpcMessage::Response {
            id,
            result: Ok(object.get("result").cloned().unwrap_or(Json::Null)),
        }),
        (None, true, false, true) => {
            let error = object.get("error").ok_or(CodecError::InvalidJson)?;
            let error = decode_rpc_failure(error)?;
            Ok(RpcMessage::Response {
                id,
                result: Err(error),
            })
        }
        _ => Err(CodecError::InvalidFieldValue),
    }
}
fn validate_method(method: &str) -> Result<(), CodecError> {
    if method.is_empty() || method.len() > 128 || method.chars().any(char::is_control) {
        return Err(CodecError::InvalidFieldValue);
    }
    Ok(())
}
fn decode_rpc_failure(value: &Json) -> Result<RpcFailure, CodecError> {
    let object = value.as_object().ok_or(CodecError::InvalidJson)?;
    let code = object
        .get("code")
        .and_then(Json::as_i64)
        .ok_or(CodecError::InvalidJson)?;
    let message = object
        .get("message")
        .and_then(Json::as_str)
        .ok_or(CodecError::InvalidJson)?
        .to_owned();
    if message.len() > MAX_PROVIDER_ERROR_MESSAGE_BYTES {
        return Err(CodecError::TooLarge);
    }
    Ok(RpcFailure {
        code,
        message,
        data: object.get("data").cloned(),
    })
}

/// Strict form validation for caller-supplied JSON. Keeps the object/array depth
/// bounded without reimplementing number parsing.
pub fn validate_json_shape(value: &Json) -> Result<(), CodecError> {
    fn visit(value: &Json, depth: usize, items: &mut usize) -> Result<(), CodecError> {
        if depth > 128 {
            return Err(CodecError::TooDeep);
        }
        *items = items.checked_add(1).ok_or(CodecError::TooManyItems)?;
        if *items > 32_768 {
            return Err(CodecError::TooManyItems);
        }
        match value {
            Json::Array(values) => {
                for value in values {
                    visit(value, depth + 1, items)?;
                }
            }
            Json::Object(values) => {
                for value in values.values() {
                    visit(value, depth + 1, items)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    let mut items = 0;
    visit(value, 0, &mut items)
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex(&self.0))
    }
}
impl fmt::Display for InvocationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex(&self.0))
    }
}
