use crate::CodecError;
use serde::{Deserialize, Serialize};

pub const MAX_APP_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
pub const TLS_NAME: &str = "harmion.local";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerRegistration {
    pub version: u32,
    pub cluster_id: String,
    pub node_id: String,
    pub binding: PeerBinding,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signing_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster_root: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "transport", rename_all = "snake_case", deny_unknown_fields)]
pub enum PeerBinding {
    Https {
        certificate: String,
        endpoint: String,
    },
    Iroh {
        endpoint_id: String,
        relay_url: Option<String>,
    },
    /// Registration v4. The node signing key is the top-level `signing_key`;
    /// `origin` is the owner-approved canonical HTTPS base origin.
    SignedHttps { origin: String },
}

/// Owner-protected discovery file. Credential is stored separately as raw bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionInfo {
    pub version: u32,
    pub cluster_id: String,
    pub node_id: String,
    pub certificate: String,
    pub endpoint: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum FilterSpec {
    All,
    Kinds { kinds: Vec<String> },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantSpec {
    pub read: Option<FilterSpec>,
    pub write: Option<FilterSpec>,
    pub manage_apps: bool,
    pub manage_nodes: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IssueAppRequest {
    pub grants: GrantSpec,
    pub expires_at: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeGrantsRequest {
    pub app_id: String,
    pub grants: GrantSpec,
    pub revoked: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OAuthRegistration {
    pub name: String,
    pub redirect_uri: String,
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
pub fn unhex_bytes(text: &str, max_bytes: usize) -> Result<Vec<u8>, CodecError> {
    if !text.len().is_multiple_of(2) || text.len() / 2 > max_bytes {
        return Err(CodecError::InvalidHex);
    }
    let digit = |b| match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        _ => Err(CodecError::InvalidHex),
    };
    text.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| Ok(digit(p[0])? * 16 + digit(p[1])?))
        .collect()
}
pub fn unhex<const N: usize>(text: &str) -> Result<[u8; N], CodecError> {
    unhex_bytes(text, N)?
        .try_into()
        .map_err(|_| CodecError::InvalidHex)
}
