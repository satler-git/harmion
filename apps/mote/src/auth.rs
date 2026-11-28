//! Mote's integrated connection and local credential store.
//!
//! Tokens live only in `<node>/mote-credentials.json` (mode 0600, owner-only
//! node directory). Access tokens are short lived; the refresh token rotates on
//! every use, so a refresh is persisted before the new access token is used.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use harmion_client::{
    Error as ClientError, OAuthTokenClient, ProtectedDir,
    protocol::{
        ConnectionInfo, FilterSpec, GrantSpec,
        connect::{
            ConnectionCompleteRequest, ConnectionOffer, ConnectionStatusRequest, validate_offer,
        },
    },
    random_id,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fmt,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{Error, Result};

pub const CREDENTIALS_FILE: &str = "mote-credentials.json";
const CREDENTIALS_LOCK: &str = "mote-credentials.lock";
const OAUTH_IDENTITY_FILE: &str = "oauth-identity.json";
const CREDENTIALS_VERSION: u32 = 1;
const MAX_CREDENTIALS_BYTES: usize = 64 * 1024;
const REFRESH_SKEW_SECONDS: i64 = 60;

#[derive(Clone, Serialize, Deserialize)]
pub struct Credentials {
    pub version: u32,
    pub client_id: String,
    pub access_token: String,
    pub refresh_token: String,
    /// Unix seconds, as reported by the node's `expires_in`.
    pub expires_at: i64,
    pub scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth_origin: Option<String>,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("client_id", &self.client_id)
            .field("access_token", &"[redacted]")
            .field("refresh_token", &"[redacted]")
            .field("expires_at", &self.expires_at)
            .field("scope", &self.scope)
            .field("oauth_origin", &self.oauth_origin)
            .finish()
    }
}

impl Credentials {
    pub fn load(directory: &Path) -> Result<Option<Self>> {
        let protected = ProtectedDir::open(directory).map_err(|error| {
            Error::Message(format!(
                "cannot open node directory {}: {error}",
                directory.display()
            ))
        })?;
        let bytes = match protected.read(CREDENTIALS_FILE, MAX_CREDENTIALS_BYTES) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(Error::Message(format!(
                    "cannot read {}: {error}",
                    directory.join(CREDENTIALS_FILE).display()
                )));
            }
        };
        let credentials: Self = serde_json::from_slice(&bytes).map_err(|_| {
            Error::Message(format!(
                "{} is not valid Mote credentials JSON",
                directory.join(CREDENTIALS_FILE).display()
            ))
        })?;
        credentials.validate()?;
        Ok(Some(credentials))
    }

    pub fn save(&self, directory: &Path) -> Result<()> {
        self.validate()?;
        let protected = ProtectedDir::open(directory).map_err(|error| {
            Error::Message(format!(
                "cannot open node directory {}: {error}",
                directory.display()
            ))
        })?;
        let mut bytes = serde_json::to_vec_pretty(self)?;
        bytes.push(b'\n');
        protected
            .replace(CREDENTIALS_FILE, &bytes)
            .map_err(|error| {
                Error::Message(format!(
                    "cannot write {} (mode 0600): {error}",
                    directory.join(CREDENTIALS_FILE).display()
                ))
            })
    }

    pub fn clear(directory: &Path) -> Result<()> {
        let _lock = lock_credentials(directory)?;
        let protected = ProtectedDir::open(directory).map_err(|error| {
            Error::Message(format!(
                "cannot open node directory {}: {error}",
                directory.display()
            ))
        })?;
        let path = directory.join(CREDENTIALS_FILE);
        match protected.verify_file(CREDENTIALS_FILE) {
            Ok(()) => match std::fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(Error::Message(format!(
                    "cannot remove {}: {error}",
                    path.display()
                ))),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(Error::Message(format!(
                "refusing to remove {}: {error}",
                path.display()
            ))),
        }
    }

    pub fn needs_refresh(&self) -> bool {
        self.expires_at <= now_seconds().saturating_add(REFRESH_SKEW_SECONDS)
    }

    pub fn token(&self) -> &str {
        &self.access_token
    }

    pub fn refresh_available(&self) -> bool {
        self.oauth_origin.is_some()
    }

    pub async fn refresh(&mut self, directory: &Path) -> Result<()> {
        // Two Mote processes can refresh at the same time. The refresh token
        // rotates on every use, so the one that loses the race would otherwise
        // persist a dead token. Reload under the lock before refreshing.
        let _lock = lock_credentials(directory)?;
        if let Some(current) = Self::load(directory)?
            && current.access_token != self.access_token
            && current.expires_at > now_seconds().saturating_add(REFRESH_SKEW_SECONDS)
        {
            *self = current;
            return Ok(());
        }
        let origin = self.oauth_origin.as_deref().ok_or_else(|| {
            Error::Message("node published no OAuth origin; run `mote connect` again".into())
        })?;
        let root = load_oauth_root(directory)?;
        let client = OAuthTokenClient::new(origin, &root).map_err(|error| {
            Error::Message(format!(
                "cannot configure OAuth refresh: {}",
                friendly_client(&error)
            ))
        })?;
        let tokens = client
            .refresh(&self.client_id, &self.refresh_token)
            .await
            .map_err(|error| {
                Error::Message(format!(
                    "OAuth refresh failed: {}; run `mote connect` again",
                    friendly_client(&error)
                ))
            })?;
        if tokens.expires_in <= 0 {
            return Err(Error::Message(
                "OAuth refresh returned no valid lifetime".into(),
            ));
        }
        self.access_token = tokens.access_token;
        self.refresh_token = tokens.refresh_token;
        self.expires_at = now_seconds().saturating_add(tokens.expires_in);
        if !tokens.scope.is_empty() {
            self.scope = tokens.scope;
        }
        self.validate()?;
        self.save(directory)
    }

    fn validate(&self) -> Result<()> {
        if self.version != CREDENTIALS_VERSION {
            return Err(Error::Message(format!(
                "unsupported Mote credentials version {}",
                self.version
            )));
        }
        if !is_hex_64(&self.client_id)
            || !is_hex_64(&self.access_token)
            || !is_hex_64(&self.refresh_token)
        {
            return Err(Error::Message(
                "Mote credentials contain an invalid token or client ID".into(),
            ));
        }
        if self.expires_at <= 0 || self.scope.trim().is_empty() {
            return Err(Error::Message(
                "Mote credentials have an invalid expiry or scope".into(),
            ));
        }
        Ok(())
    }
}

fn lock_credentials(directory: &Path) -> Result<std::fs::File> {
    let path = directory.join(CREDENTIALS_LOCK);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|error| Error::Message(format!("cannot open {}: {error}", path.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    file.lock()
        .map_err(|error| Error::Message(format!("cannot lock {}: {error}", path.display())))?;
    Ok(file)
}

pub fn is_hex_64(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn friendly_client(error: &ClientError) -> String {
    match error {
        ClientError::OAuth(code) => format!("token endpoint rejected the request ({code})"),
        ClientError::Http(400) => "token endpoint rejected the request".into(),
        ClientError::Transport => "cannot reach the node OAuth endpoint".into(),
        ClientError::Timeout => "the node OAuth endpoint timed out".into(),
        _ => "token endpoint returned an error".into(),
    }
}

pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

pub fn generate_pkce() -> Result<Pkce> {
    let verifier = URL_SAFE_NO_PAD.encode(random_id()?);
    let digest = Sha256::digest(verifier.as_bytes());
    Ok(Pkce {
        verifier,
        challenge: URL_SAFE_NO_PAD.encode(digest),
    })
}

pub fn random_state() -> Result<String> {
    Ok(harmion_client::protocol::hex(&random_id()?))
}

pub fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or(0)
}

pub fn connection_info(directory: &Path) -> Result<ConnectionInfo> {
    let protected = ProtectedDir::open(directory).map_err(|error| {
        Error::Message(format!(
            "cannot open node directory {}: {error}",
            directory.display()
        ))
    })?;
    let bytes = protected
        .read("connection.json", 150_000)
        .map_err(|error| {
            Error::Message(format!(
                "cannot read {}: {error}; is this an initialized Harmion node?",
                directory.join("connection.json").display()
            ))
        })?;
    let info: ConnectionInfo = serde_json::from_slice(&bytes).map_err(|_| {
        Error::Message(format!(
            "{} is not a valid Harmion connection file",
            directory.join("connection.json").display()
        ))
    })?;
    if info.endpoint.parse::<std::net::SocketAddr>().is_err() {
        return Err(Error::Message(format!(
            "{} contains an invalid endpoint",
            directory.join("connection.json").display()
        )));
    }
    Ok(info)
}

fn load_oauth_root(directory: &Path) -> Result<Vec<u8>> {
    let protected = ProtectedDir::open(directory).map_err(|error| {
        Error::Message(format!(
            "cannot open node directory {}: {error}",
            directory.display()
        ))
    })?;
    let bytes = protected
        .read(OAUTH_IDENTITY_FILE, 65_536)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Error::Message(format!(
                    "node has no OAuth trust root ({}); run `harmiond oauth-provision {}`",
                    directory.join(OAUTH_IDENTITY_FILE).display(),
                    directory.display()
                ))
            } else {
                Error::Message(format!(
                    "cannot read {}: {error}",
                    directory.join(OAUTH_IDENTITY_FILE).display()
                ))
            }
        })?;
    #[derive(Deserialize)]
    struct Identity {
        root_der: Vec<u8>,
    }
    let identity: Identity = serde_json::from_slice(&bytes).map_err(|_| {
        Error::Message(format!(
            "{} is not a valid OAuth identity file",
            directory.join(OAUTH_IDENTITY_FILE).display()
        ))
    })?;
    if identity.root_der.is_empty() {
        return Err(Error::Message(
            "OAuth trust root is empty; run `harmiond oauth-provision`".into(),
        ));
    }
    Ok(identity.root_der)
}

pub fn offer(
    name: &str,
    redirect_uri: &str,
    challenge: &str,
    state: &str,
) -> Result<ConnectionOffer> {
    let grants = GrantSpec {
        read: Some(FilterSpec::Kinds {
            kinds: vec!["capture".into()],
        }),
        write: Some(FilterSpec::Kinds {
            kinds: vec!["capture".into()],
        }),
        call: false,
        provide: false,
        provide_names: Vec::new(),
        manage_apps: false,
        manage_nodes: false,
    };
    let offer = ConnectionOffer {
        version: 1,
        name: name.to_owned(),
        redirect_uri: redirect_uri.to_owned(),
        web_origin: None,
        client_id: None,
        challenge: challenge.to_owned(),
        state: state.to_owned(),
        grants,
        provider: None,
        providers: Vec::new(),
        provider_replace: false,
    };
    validate_offer(&offer)
        .map_err(|error| Error::Message(format!("invalid connection offer: {error}")))?;
    Ok(offer)
}

pub fn status_request(connection_id: &str, request_secret: &str) -> ConnectionStatusRequest {
    ConnectionStatusRequest {
        connection_id: connection_id.to_owned(),
        request_secret: request_secret.to_owned(),
    }
}

pub fn complete_request(
    connection_id: &str,
    request_secret: &str,
    verifier: &str,
) -> ConnectionCompleteRequest {
    ConnectionCompleteRequest {
        connection_id: connection_id.to_owned(),
        request_secret: request_secret.to_owned(),
        code_verifier: verifier.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use harmion_client::protocol::FilterSpec;
    use std::path::PathBuf;

    fn temp_node() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mote-auth-test-{}",
            harmion_client::protocol::hex(&random_id().unwrap())
        ));
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        }
        #[cfg(not(unix))]
        std::fs::create_dir(&dir).unwrap();
        dir
    }

    #[test]
    fn pkce_and_offer_are_capture_only() {
        let pkce = generate_pkce().unwrap();
        assert_eq!(pkce.verifier.len(), 43);
        assert_eq!(pkce.challenge.len(), 43);
        assert_ne!(pkce.verifier, pkce.challenge);
        let offer = offer(
            "Mote",
            "http://127.0.0.1:12345/mote/callback",
            &pkce.challenge,
            &random_state().unwrap(),
        )
        .unwrap();
        let capture = FilterSpec::Kinds {
            kinds: vec!["capture".into()],
        };
        assert_eq!(offer.grants.read, Some(capture.clone()));
        assert_eq!(offer.grants.write, Some(capture));
        assert!(!offer.grants.call);
        assert!(!offer.grants.provide);
        assert_eq!(offer.providers.len(), 0);
    }

    #[test]
    fn credentials_round_trip_without_leaking_tokens() {
        let dir = temp_node();
        let credentials = Credentials {
            version: 1,
            client_id: "11".repeat(32),
            access_token: "22".repeat(32),
            refresh_token: "33".repeat(32),
            expires_at: now_seconds() + 600,
            scope: "read:kind:Y2FwdHVyZQ write:kind:Y2FwdHVyZQ".into(),
            oauth_origin: Some("https://127.0.0.1:4444".into()),
        };
        credentials.save(&dir).unwrap();
        let debug = format!("{credentials:?}");
        assert!(!debug.contains(&"22".repeat(32)));
        assert!(!debug.contains(&"33".repeat(32)));
        let loaded = Credentials::load(&dir).unwrap().unwrap();
        assert_eq!(loaded.client_id, credentials.client_id);
        assert_eq!(loaded.access_token, credentials.access_token);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(dir.join(CREDENTIALS_FILE))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        Credentials::clear(&dir).unwrap();
        assert!(Credentials::load(&dir).unwrap().is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}
