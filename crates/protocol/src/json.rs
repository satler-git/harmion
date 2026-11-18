//! Preserve JSON objects even when their keys coincide with serde_json's private
//! arbitrary-precision number marker. Inspect raw token kinds before conversion.
use crate::{CodecError, MAX_APP_RESPONSE_BYTES};
use serde_json::{Value, value::RawValue};
use std::collections::BTreeMap;

pub fn decode_app_json(bytes: &[u8]) -> Result<Value, CodecError> {
    if bytes.len() > MAX_APP_RESPONSE_BYTES {
        return Err(CodecError::TooLarge);
    }
    let raw: &RawValue = serde_json::from_slice(bytes).map_err(|_| CodecError::InvalidJson)?;
    convert(raw, 0)
}
fn convert(raw: &RawValue, depth: usize) -> Result<Value, CodecError> {
    if depth > 128 {
        return Err(CodecError::TooDeep);
    }
    let text = raw.get();
    Ok(match text.as_bytes().first() {
        Some(b'{') => {
            let fields: BTreeMap<String, &RawValue> =
                serde_json::from_str(text).map_err(|_| CodecError::InvalidJson)?;
            Value::Object(
                fields
                    .into_iter()
                    .map(|(k, v)| Ok((k, convert(v, depth + 1)?)))
                    .collect::<Result<_, CodecError>>()?,
            )
        }
        Some(b'[') => {
            let values: Vec<&RawValue> =
                serde_json::from_str(text).map_err(|_| CodecError::InvalidJson)?;
            Value::Array(
                values
                    .into_iter()
                    .map(|v| convert(v, depth + 1))
                    .collect::<Result<_, _>>()?,
            )
        }
        _ => serde_json::from_str(text).map_err(|_| CodecError::InvalidJson)?,
    })
}
