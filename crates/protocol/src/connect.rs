//! Integrated OAuth connection offer and status contract.
use crate::interpreter::{ProviderRegistration, validate_provider_name};
use crate::{CodecError, GrantSpec, hex, unhex};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use url::{Host as UrlHost, Url};

pub const CONNECTION_VERSION: u32 = 1;
pub const MAX_OFFER_BYTES: usize = 192 * 1024;
pub const MAX_CONNECTION_STATE: usize = 512;
pub const MAX_CONNECTION_NAME: usize = 128;
pub const CONNECTION_TTL_SECONDS: i64 = 5 * 60;
pub const MAX_PENDING_CONNECTIONS: usize = 32;
/// One connection offer may register at most this many Provider manifests.
pub const MAX_CONNECTION_PROVIDERS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ConnectionId(pub [u8; 32]);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionOffer {
    pub version: u32,
    pub name: String,
    pub redirect_uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_origin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    pub challenge: String,
    pub state: String,
    pub grants: GrantSpec,
    /// Legacy single-Provider manifest. New clients send `providers` instead;
    /// the two fields are mutually exclusive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderRegistration>,
    /// Provider manifests registered by one approved connection. Serialized only
    /// when non-empty; legacy `provider` remains accepted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<ProviderRegistration>,
    /// Applies to every manifest in the offer (`provider` or `providers`).
    #[serde(default)]
    pub provider_replace: bool,
}

impl ConnectionOffer {
    /// Every requested manifest, legacy singular first and then the vector.
    /// Validation rejects offers where both sources are non-empty.
    pub fn all_providers(&self) -> impl Iterator<Item = &ProviderRegistration> {
        self.provider.iter().chain(self.providers.iter())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionStatus {
    Pending,
    Approved,
    Denied,
    Completed,
    Expired,
}
impl ConnectionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Denied => "denied",
            Self::Completed => "completed",
            Self::Expired => "expired",
        }
    }
    pub fn from_storage(value: i64) -> Option<Self> {
        match value {
            0 => Some(Self::Pending),
            1 => Some(Self::Approved),
            2 => Some(Self::Denied),
            3 => Some(Self::Completed),
            4 => Some(Self::Expired),
            _ => None,
        }
    }
    pub fn storage(self) -> i64 {
        match self {
            Self::Pending => 0,
            Self::Approved => 1,
            Self::Denied => 2,
            Self::Completed => 3,
            Self::Expired => 4,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ConnectionStarted {
    pub connection_id: String,
    pub request_secret: String,
    pub expires_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification_url: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ConnectionStatusView {
    pub connection_id: String,
    pub status: &'static str,
    pub name: String,
    pub web_origin: Option<String>,
    /// First Provider name, retained for older clients.
    pub provider_name: Option<String>,
    /// Every Provider name requested by this connection, in offer order.
    pub provider_names: Vec<String>,
    pub expires_at: String,
}

pub fn validate_offer(offer: &ConnectionOffer) -> Result<(), CodecError> {
    if offer.version != CONNECTION_VERSION {
        return Err(CodecError::UnsupportedVersion(offer.version));
    }
    if offer.name.is_empty()
        || offer.name.len() > MAX_CONNECTION_NAME
        || offer.name.chars().any(char::is_control)
    {
        return Err(CodecError::InvalidFieldValue);
    }
    validate_redirect(&offer.redirect_uri)?;
    if let Some(origin) = &offer.web_origin {
        validate_web_origin(origin, &offer.redirect_uri)?;
    }
    if let Some(client) = &offer.client_id {
        unhex::<32>(client)?;
    }
    if offer.challenge.len() != 43 {
        return Err(CodecError::InvalidFieldValue);
    }
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(&offer.challenge)
        .map_err(|_| CodecError::InvalidFieldValue)?;
    if challenge.len() != 32 {
        return Err(CodecError::InvalidFieldValue);
    }
    if offer.state.is_empty()
        || offer.state.len() > MAX_CONNECTION_STATE
        || offer.state.chars().any(char::is_control)
    {
        return Err(CodecError::InvalidFieldValue);
    }
    let grants = &offer.grants;
    if grants.manage_apps
        || grants.manage_nodes
        || (grants.read.is_none() && grants.write.is_none() && !grants.call && !grants.provide)
    {
        return Err(CodecError::InvalidFieldValue);
    }
    if grants.provide {
        if grants.provide_names.is_empty() || grants.provide_names.len() > 32 {
            return Err(CodecError::InvalidFieldValue);
        }
        let mut seen = std::collections::BTreeSet::new();
        for name in &grants.provide_names {
            validate_provider_name(name)?;
            if !seen.insert(name) {
                return Err(CodecError::Duplicate);
            }
        }
    } else if !grants.provide_names.is_empty() {
        return Err(CodecError::InvalidFieldValue);
    }
    if offer.provider.is_some() && !offer.providers.is_empty() {
        return Err(CodecError::InvalidFieldValue);
    }
    if offer.providers.len() > MAX_CONNECTION_PROVIDERS {
        return Err(CodecError::InvalidFieldValue);
    }
    let mut names = std::collections::BTreeSet::new();
    for provider in offer.all_providers() {
        crate::interpreter::validate_registration(provider)?;
        if !grants.provide_names.contains(&provider.name) {
            return Err(CodecError::InvalidFieldValue);
        }
        if !names.insert(provider.name.as_str()) {
            return Err(CodecError::Duplicate);
        }
    }
    Ok(())
}

pub fn validate_redirect(redirect: &str) -> Result<(), CodecError> {
    if redirect.len() > 2048 || !redirect.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(CodecError::InvalidFieldValue);
    }
    let parsed = Url::parse(redirect).map_err(|_| CodecError::InvalidFieldValue)?;
    if parsed.scheme() != "http"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
    {
        return Err(CodecError::InvalidFieldValue);
    }
    let loopback = match parsed.host() {
        Some(UrlHost::Ipv4(ip)) => ip.is_loopback(),
        Some(UrlHost::Ipv6(ip)) => ip.is_loopback(),
        _ => false,
    };
    if !loopback || parsed.port() == Some(0) {
        return Err(CodecError::InvalidFieldValue);
    }
    Ok(())
}

pub fn validate_web_origin(origin: &str, redirect: &str) -> Result<(), CodecError> {
    let url = Url::parse(origin).map_err(|_| CodecError::InvalidFieldValue)?;
    if url.scheme() != "http"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || url.port() == Some(0)
    {
        return Err(CodecError::InvalidFieldValue);
    }
    let loopback = match url.host() {
        Some(UrlHost::Ipv4(ip)) => ip.is_loopback(),
        Some(UrlHost::Ipv6(ip)) => ip.is_loopback(),
        _ => false,
    };
    if !loopback {
        return Err(CodecError::InvalidFieldValue);
    }
    let redirect = Url::parse(redirect).map_err(|_| CodecError::InvalidFieldValue)?;
    if redirect.origin() != url.origin() {
        return Err(CodecError::InvalidFieldValue);
    }
    Ok(())
}

pub fn decode_offer(bytes: &[u8]) -> Result<ConnectionOffer, CodecError> {
    if bytes.len() > MAX_OFFER_BYTES {
        return Err(CodecError::TooLarge);
    }
    let value = crate::decode_app_json_strict(bytes)?;
    let offer: ConnectionOffer =
        serde_json::from_value(value).map_err(|_| CodecError::InvalidJson)?;
    validate_offer(&offer)?;
    Ok(offer)
}

pub fn connection_id(text: &str) -> Result<ConnectionId, CodecError> {
    unhex(text).map(ConnectionId)
}
pub fn hex_connection_id(id: ConnectionId) -> String {
    hex(&id.0)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionStatusRequest {
    pub connection_id: String,
    pub request_secret: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionCompleteRequest {
    pub connection_id: String,
    pub request_secret: String,
    pub code_verifier: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ProviderConnectionInfo {
    pub provider_id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ticket: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub websocket_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ConnectionCompletion {
    pub token_type: &'static str,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
    pub scope: String,
    pub client_id: String,
    /// First Provider connection, retained for older clients.
    pub provider: Option<ProviderConnectionInfo>,
    /// Every Provider registered by the approval, in offer order.
    pub providers: Vec<ProviderConnectionInfo>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionApproval {
    pub connection_id: String,
    pub allow: bool,
    #[serde(default)]
    pub replace: bool,
    /// Optional authorization deadline (unix micros) chosen by the owner.
    #[serde(default)]
    pub expires_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderTicketRequest {
    pub provider_id: String,
}
