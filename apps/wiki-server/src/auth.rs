//! OAuth credentials, PKCE and refresh policy for the Wiki server.
//!
//! Tokens live only in `<node>/credentials.json` (mode 0600, owner-only
//! directory). They are never written to logs, status output or RPC errors.
//! Access tokens are short lived (10 minutes on the node) and refresh tokens
//! rotate on every use, so a refresh is persisted before the new access token is
//! used for any App API request.

use std::{
    collections::BTreeSet,
    fmt,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use harmion_client::protocol::interpreter::ProviderId;
use harmion_client::protocol::{self, ConnectionInfo, FilterSpec};
use harmion_client::{Error as ClientError, OAuthTokens, ProtectedDir};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const CREDENTIALS_FILE: &str = "credentials.json";
pub const CONNECTION_FILE: &str = "connection.json";
pub const OAUTH_IDENTITY_FILE: &str = "oauth-identity.json";
const CREDENTIALS_VERSION: u32 = 1;
const MAX_CREDENTIALS_BYTES: usize = 64 * 1024;
/// Refresh shortly before the node would reject the access token.
pub const REFRESH_SKEW_SECONDS: i64 = 60;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderBinding {
    pub name: String,
    pub provider_id: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
    /// Provider registered by the integrated connection, if any. This is an
    /// identifier, not a token; it is persisted so `serve --node` can attach.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<ProviderId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<ProviderBinding>,
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
            .field("provider_id", &self.provider_id)
            .field("providers", &self.providers)
            .finish()
    }
}

struct OAuthIdentity {
    root_der: Vec<u8>,
}

#[derive(Deserialize)]
struct OAuthIdentityFile {
    root_der: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum WriteAccess {
    All,
    Kinds(BTreeSet<String>),
    None,
}

impl Credentials {
    pub fn provider_bindings(&self) -> Vec<ProviderBinding> {
        if !self.providers.is_empty() {
            return self.providers.clone();
        }
        self.provider_id
            .map(|id| ProviderBinding {
                name: "wiki/outline".to_owned(),
                provider_id: harmion_client::protocol::hex(&id.0),
            })
            .into_iter()
            .collect()
    }

    pub fn load(dir: &Path) -> Result<Self, String> {
        let directory = ProtectedDir::open(dir).map_err(|error| {
            format!(
                "cannot open node directory {}: {error} (the directory must be owner-only, mode 0700)",
                dir.display()
            )
        })?;
        let bytes = directory.read(CREDENTIALS_FILE, MAX_CREDENTIALS_BYTES).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                format!(
                    "node credentials are missing ({}); run `wiki-server connect --node {}` first",
                    dir.join(CREDENTIALS_FILE).display(),
                    dir.display()
                )
            } else if error.kind() == std::io::ErrorKind::PermissionDenied {
                format!(
                    "{} must be a regular owner-only file with mode 0600",
                    dir.join(CREDENTIALS_FILE).display()
                )
            } else {
                format!(
                    "cannot read {}: {error}",
                    dir.join(CREDENTIALS_FILE).display()
                )
            }
        })?;
        let credentials: Self = serde_json::from_slice(&bytes).map_err(|_| {
            format!(
                "{} is not valid Harmion credentials JSON",
                dir.join(CREDENTIALS_FILE).display()
            )
        })?;
        credentials.validate()?;
        Ok(credentials)
    }

    /// Atomic replace through `ProtectedDir`, which creates the temporary file
    /// with mode 0600 and rejects symlinks or shared ownership.
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        self.validate()?;
        let directory = ProtectedDir::open(dir)
            .map_err(|error| format!("cannot open node directory {}: {error}", dir.display()))?;
        let mut bytes = serde_json::to_vec_pretty(self)
            .map_err(|error| format!("cannot encode node credentials: {error}"))?;
        bytes.push(b'\n');
        directory
            .replace(CREDENTIALS_FILE, &bytes)
            .map_err(|error| {
                format!(
                    "cannot write {} (mode 0600): {error}",
                    dir.join(CREDENTIALS_FILE).display()
                )
            })
    }

    fn validate(&self) -> Result<(), String> {
        if self.version != CREDENTIALS_VERSION {
            return Err(format!(
                "unsupported credentials version {} (expected {CREDENTIALS_VERSION})",
                self.version
            ));
        }
        protocol::unhex::<32>(&self.access_token)
            .map_err(|_| "credentials contain an invalid access token".to_owned())?;
        protocol::unhex::<32>(&self.refresh_token)
            .map_err(|_| "credentials contain an invalid refresh token".to_owned())?;
        protocol::unhex::<32>(&self.client_id)
            .map_err(|_| "credentials contain an invalid client_id".to_owned())?;
        if self.expires_at <= 0 {
            return Err("credentials contain an invalid expiry".to_owned());
        }
        if let Some(origin) = &self.oauth_origin {
            let authority = origin
                .strip_prefix("https://")
                .ok_or_else(|| "credentials contain a non-HTTPS OAuth origin".to_owned())?;
            let endpoint: std::net::SocketAddr = authority
                .parse()
                .map_err(|_| "credentials contain an invalid OAuth origin".to_owned())?;
            if !endpoint.ip().is_loopback() || endpoint.port() == 0 {
                return Err("credentials contain a non-loopback OAuth origin".to_owned());
            }
        }
        Ok(())
    }

    pub fn access_bytes(&self) -> Result<[u8; 32], String> {
        protocol::unhex::<32>(&self.access_token)
            .map_err(|_| "credentials contain an invalid access token".to_owned())
    }

    /// True when the access token is expired or inside the refresh skew.
    pub fn needs_refresh(&self, now_seconds: i64) -> bool {
        self.expires_at.saturating_sub(now_seconds) <= REFRESH_SKEW_SECONDS
    }

    pub fn write_access(&self) -> WriteAccess {
        parse_write_access(&self.scope)
    }

    /// Kind names that the current grant does not allow this app to write.
    pub fn missing_write_kinds<'a>(
        &self,
        kinds: impl IntoIterator<Item = &'a String>,
    ) -> Vec<String> {
        match self.write_access() {
            WriteAccess::All => Vec::new(),
            WriteAccess::None => kinds.into_iter().cloned().collect(),
            WriteAccess::Kinds(allowed) => kinds
                .into_iter()
                .filter(|kind| !allowed.contains(*kind))
                .cloned()
                .collect(),
        }
    }

    /// Apply a rotated token pair. The refresh token changes on every use, so
    /// callers must persist the result before making the next request.
    pub fn apply_refresh(&mut self, tokens: OAuthTokens, now_seconds: i64) {
        self.access_token = tokens.access_token;
        self.refresh_token = tokens.refresh_token;
        self.expires_at = now_seconds.saturating_add(tokens.expires_in);
        self.scope = tokens.scope;
    }
}

fn parse_write_access(scope: &str) -> WriteAccess {
    let mut kinds = BTreeSet::new();
    let mut found = false;
    for grant in scope.split_whitespace() {
        if grant == "write:all" {
            return WriteAccess::All;
        }
        let Some(encoded) = grant.strip_prefix("write:kind:") else {
            continue;
        };
        let Ok(decoded) = URL_SAFE_NO_PAD.decode(encoded) else {
            continue;
        };
        let Ok(kind) = String::from_utf8(decoded) else {
            continue;
        };
        if !kind.is_empty() {
            kinds.insert(kind);
            found = true;
        }
    }
    if found {
        WriteAccess::Kinds(kinds)
    } else {
        WriteAccess::None
    }
}

pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

/// RFC 7636 S256 verifier/challenge. A 32-byte random verifier encodes to 43
/// base64url characters, inside the 43..=128 contract range.
pub fn generate_pkce() -> Result<Pkce, String> {
    let verifier =
        URL_SAFE_NO_PAD.encode(harmion_client::random_id().map_err(|error| error.to_string())?);
    let digest = Sha256::digest(verifier.as_bytes());
    Ok(Pkce {
        challenge: URL_SAFE_NO_PAD.encode(digest),
        verifier,
    })
}

pub fn random_state() -> Result<String, String> {
    Ok(protocol::hex(
        &harmion_client::random_id().map_err(|error| error.to_string())?,
    ))
}

pub fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or(0)
}

pub fn connection_info(dir: &Path) -> Result<ConnectionInfo, String> {
    let directory = ProtectedDir::open(dir)
        .map_err(|error| format!("cannot open node directory {}: {error}", dir.display()))?;
    let bytes = directory.read(CONNECTION_FILE, 150_000).map_err(|error| {
        format!(
            "cannot read {}: {error}; is this an initialized Harmion node?",
            dir.join(CONNECTION_FILE).display()
        )
    })?;
    let info: ConnectionInfo = serde_json::from_slice(&bytes).map_err(|_| {
        format!(
            "{} is not a valid Harmion connection file",
            dir.join(CONNECTION_FILE).display()
        )
    })?;
    if info.endpoint.parse::<std::net::SocketAddr>().is_err() {
        return Err(format!(
            "{} contains an invalid endpoint",
            dir.join(CONNECTION_FILE).display()
        ));
    }
    Ok(info)
}

/// Read the OAuth trust root from the same owner-only file that
/// `harmiond oauth-root` prints as PEM. The daemon stores the CA DER here so no
/// separate trust-store import is needed for native app refreshes.
pub fn load_oauth_root(dir: &Path) -> Result<Vec<u8>, String> {
    let directory = ProtectedDir::open(dir)
        .map_err(|error| format!("cannot open node directory {}: {error}", dir.display()))?;
    let bytes = directory
        .read(OAUTH_IDENTITY_FILE, 65_536)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                format!(
                    "node has no OAuth trust root ({}); run `harmiond oauth-provision {}` and retry",
                    dir.join(OAUTH_IDENTITY_FILE).display(),
                    dir.display()
                )
            } else {
                format!(
                    "cannot read {}: {error}",
                    dir.join(OAUTH_IDENTITY_FILE).display()
                )
            }
        })?;
    let identity: OAuthIdentityFile = serde_json::from_slice(&bytes).map_err(|_| {
        format!(
            "{} is not a valid OAuth identity file",
            dir.join(OAUTH_IDENTITY_FILE).display()
        )
    })?;
    let root = OAuthIdentity {
        root_der: identity.root_der,
    };
    if root.root_der.is_empty() {
        return Err("OAuth identity file contains an empty trust root".to_owned());
    }
    Ok(root.root_der)
}

/// Return true exactly once for a 401: the caller refreshes and retries. A
/// second 401 means the refreshed credential is also rejected (revoked app or
/// grant change), so it must be surfaced as a re-connect error.
pub fn retry_after_unauthorized(attempt: u32, error: &ClientError) -> bool {
    attempt == 0 && matches!(error, ClientError::Http(401))
}

/// Map a refresh failure to an actionable message. `invalid_grant` (and a bare
/// HTTP 400 from the token endpoint) means the refresh family is gone; all
/// other failures are transient and leave the on-disk credentials intact.
pub fn refresh_failure_message(dir: &Path, error: &ClientError) -> String {
    let reconnect = format!(
        "the node refresh token was consumed, expired or revoked; run `wiki-server connect --node {}` again",
        dir.display()
    );
    match error {
        ClientError::OAuth(code) if code == "invalid_grant" => reconnect,
        ClientError::Http(400) => reconnect,
        ClientError::Transport | ClientError::Timeout => format!(
            "cannot reach the node OAuth endpoint ({}); the saved credentials were not changed",
            dir.display()
        ),
        ClientError::Http(status) => format!(
            "node OAuth endpoint returned HTTP {status}; the saved credentials were not changed"
        ),
        _ => format!("node OAuth refresh failed ({error}); the saved credentials were not changed"),
    }
}

/// A `write` filter from the CLI. `None` keeps the caller's requested kinds.
pub fn filter_spec(all: bool, kinds: Vec<String>) -> Option<FilterSpec> {
    if all {
        Some(FilterSpec::All)
    } else if kinds.is_empty() {
        None
    } else {
        Some(FilterSpec::Kinds { kinds })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};
    use std::sync::atomic::{AtomicU64, Ordering};
    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    const ACCESS: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    const REFRESH: &str = "2222222222222222222222222222222222222222222222222222222222222222";
    const CLIENT: &str = "3333333333333333333333333333333333333333333333333333333333333333";

    fn credentials() -> Credentials {
        Credentials {
            version: 1,
            client_id: CLIENT.to_owned(),
            access_token: ACCESS.to_owned(),
            refresh_token: REFRESH.to_owned(),
            expires_at: 1_000,
            scope: "read:kind:d2lraQ write:all".to_owned(),
            oauth_origin: Some("https://127.0.0.1:8788".to_owned()),
            provider_id: None,
            providers: Vec::new(),
        }
    }

    struct TempDir {
        path: std::path::PathBuf,
    }
    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "harmion-wiki-auth-{name}-{}-{}-{}",
                std::process::id(),
                now_seconds(),
                TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            Self { path }
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn pkce_matches_the_rfc_7636_s256_vector() {
        // RFC 7636 Appendix B.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        let generated = generate_pkce().unwrap();
        assert_eq!(generated.verifier.len(), 43);
        assert_eq!(generated.challenge.len(), 43);
        assert_eq!(
            URL_SAFE_NO_PAD
                .encode(Sha256::digest(generated.verifier.as_bytes()))
                .as_str(),
            generated.challenge
        );
        assert!(
            generated
                .verifier
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~'))
        );
    }

    #[test]
    fn refresh_is_selected_before_expiry_using_the_skew() {
        let credentials = credentials();
        assert!(!credentials.needs_refresh(0));
        assert!(!credentials.needs_refresh(939)); // 61 seconds left
        assert!(credentials.needs_refresh(940)); // exactly the 60 second skew
        assert!(credentials.needs_refresh(1_001));
        let mut refreshed = credentials.clone();
        refreshed.apply_refresh(
            OAuthTokens {
                access_token: CLIENT.to_owned(),
                refresh_token: REFRESH.to_owned(),
                expires_in: 600,
                scope: "write:kind:d2lraQ".to_owned(),
            },
            1_000,
        );
        assert_eq!(refreshed.expires_at, 1_600);
        assert!(!refreshed.needs_refresh(1_539));
        assert!(refreshed.needs_refresh(1_540));
    }

    #[test]
    fn credentials_round_trip_with_mode_0600() {
        let directory = TempDir::new("roundtrip");
        let credentials = credentials();
        credentials.save(&directory.path).unwrap();
        let path = directory.path.join(CREDENTIALS_FILE);
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "credentials.json must be owner-only");
        assert_eq!(Credentials::load(&directory.path).unwrap(), credentials);
        assert!(!format!("{credentials:?}").contains(ACCESS));
        assert!(!format!("{credentials:?}").contains(REFRESH));
    }

    #[test]
    #[test]
    fn credentials_persist_all_provider_bindings() {
        let directory = TempDir::new("providers");
        let mut value = credentials();
        value.providers = vec![
            ProviderBinding {
                name: "wiki/backlinks".to_owned(),
                provider_id: "11".repeat(32),
            },
            ProviderBinding {
                name: "wiki/outline".to_owned(),
                provider_id: "22".repeat(32),
            },
        ];
        value.save(&directory.path).unwrap();
        let loaded = Credentials::load(&directory.path).unwrap();
        assert_eq!(loaded.provider_bindings(), value.providers);
    }

    fn credentials_persist_the_registered_provider_id() {
        let directory = TempDir::new("provider");
        let mut credentials = credentials();
        credentials.provider_id = Some(ProviderId([0x2a; 32]));
        credentials.save(&directory.path).unwrap();
        let path = directory.path.join(CREDENTIALS_FILE);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let loaded = Credentials::load(&directory.path).unwrap();
        assert_eq!(loaded.provider_id, Some(ProviderId([0x2a; 32])));
        // Credentials written before provider registration stay readable.
        let mut old = serde_json::to_value(credentials).unwrap();
        old.as_object_mut().unwrap().remove("provider_id");
        fs::write(&path, serde_json::to_vec_pretty(&old).unwrap()).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            Credentials::load(&directory.path).unwrap().provider_id,
            None
        );
    }

    #[test]
    fn credentials_with_permissive_mode_are_rejected() {
        let directory = TempDir::new("mode");
        credentials().save(&directory.path).unwrap();
        let path = directory.path.join(CREDENTIALS_FILE);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let error = Credentials::load(&directory.path).unwrap_err();
        assert!(error.contains("0600"), "{error}");
    }

    #[test]
    fn missing_credentials_tell_the_user_to_connect() {
        let directory = TempDir::new("missing");
        let error = Credentials::load(&directory.path).unwrap_err();
        assert!(error.contains("wiki-server connect"), "{error}");
    }

    #[test]
    fn write_scope_accepts_only_granted_kinds() {
        let mut credentials = credentials();
        assert_eq!(credentials.write_access(), WriteAccess::All);
        assert!(
            credentials
                .missing_write_kinds(["wiki".to_owned()].iter())
                .is_empty()
        );

        credentials.scope = "read:kind:d2lraQ write:kind:d2lraQ write:kind:bGVkZ2Vy".to_owned();
        assert_eq!(
            credentials.write_access(),
            WriteAccess::Kinds(["wiki".to_owned(), "ledger".to_owned()].into())
        );
        assert!(
            credentials
                .missing_write_kinds(["wiki".to_owned(), "ledger".to_owned()].iter())
                .is_empty()
        );
        assert_eq!(
            credentials.missing_write_kinds(
                ["wiki".to_owned(), "ledger".to_owned(), "free".to_owned()].iter()
            ),
            ["free".to_owned()]
        );

        credentials.scope = "read:kind:d2lraQ".to_owned();
        assert_eq!(credentials.write_access(), WriteAccess::None);
        assert_eq!(
            credentials.missing_write_kinds(["wiki".to_owned()].iter()),
            ["wiki".to_owned()]
        );
    }

    #[test]
    fn only_the_first_401_retries() {
        assert!(retry_after_unauthorized(0, &ClientError::Http(401)));
        assert!(!retry_after_unauthorized(1, &ClientError::Http(401)));
        assert!(!retry_after_unauthorized(0, &ClientError::Http(403)));
        assert!(!retry_after_unauthorized(0, &ClientError::Http(410)));
        assert!(!retry_after_unauthorized(0, &ClientError::Http(429)));
    }

    #[test]
    fn invalid_grant_is_a_reconnect_error() {
        let directory = TempDir::new("reconnect");
        let message = refresh_failure_message(
            &directory.path,
            &ClientError::OAuth("invalid_grant".to_owned()),
        );
        assert!(message.contains("wiki-server connect"), "{message}");
    }
}
