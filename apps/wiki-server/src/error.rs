//! JSON-RPC error with optional structured `data`.

use serde_json::{Value, json};

#[derive(Debug, Clone)]
pub struct RpcError {
    pub code: u16,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcError {
    pub fn new(code: u16, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(400, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(404, message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(409, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(500, message)
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(501, message)
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    pub fn to_json(&self, request_id: u64) -> Value {
        let mut error = json!({ "code": self.code, "message": self.message });
        if let Some(data) = &self.data {
            error["data"] = data.clone();
        }
        json!({ "id": request_id, "error": error })
    }
}

pub type RpcResult<T> = Result<T, RpcError>;

pub fn required_str(params: &Value, key: &str) -> RpcResult<String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| RpcError::bad_request(format!("missing string parameter {key:?}")))
}

pub fn optional_str(params: &Value, key: &str) -> RpcResult<Option<String>> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(RpcError::bad_request(format!(
            "parameter {key:?} must be a string"
        ))),
    }
}

pub fn required_u64(params: &Value, key: &str) -> RpcResult<u64> {
    params
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| RpcError::bad_request(format!("missing integer parameter {key:?}")))
}

pub fn optional_u64(params: &Value, key: &str) -> RpcResult<Option<u64>> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| RpcError::bad_request(format!("parameter {key:?} must be an integer"))),
    }
}

pub fn optional_bool(params: &Value, key: &str) -> RpcResult<Option<bool>> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_bool()
            .map(Some)
            .ok_or_else(|| RpcError::bad_request(format!("parameter {key:?} must be a boolean"))),
    }
}

pub fn optional_str_array(params: &Value, key: &str) -> RpcResult<Vec<String>> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str().map(str::to_owned).ok_or_else(|| {
                    RpcError::bad_request(format!("parameter {key:?} must be an array of strings"))
                })
            })
            .collect(),
        Some(_) => Err(RpcError::bad_request(format!(
            "parameter {key:?} must be an array of strings"
        ))),
    }
}
