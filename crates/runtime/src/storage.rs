//! Owner-only node directory access shared by the daemon and Android.
//!
//! The protection rules live in `harmion-local`; this module maps them to the
//! named files of a node and adds the small amount of parsing the runtime needs.
pub use crate::config::{
    ADMIN_SECRET_FILE, CERTIFICATE_FILE, CONFIG_FILE, CONNECTION_FILE, DATABASE_FILE,
    IROH_SECRET_FILE, MAX_CONFIG_BYTES, MAX_SECRET_BYTES, OAUTH_IDENTITY_FILE, PEER_FILE,
    PRIVATE_KEY_FILE,
};
use crate::{Error, Result, config::NodeConfig};
use harmion_local::ProtectedDir;
use harmion_net_native::tls::BrowserTls;
use serde::{Deserialize, Serialize};
use std::{io, path::Path};

/// Open an initialized node directory.
pub fn open_node(root: impl AsRef<Path>) -> Result<ProtectedDir> {
    let directory = ProtectedDir::open(root)?;
    if directory.path().join("snapshot.json").is_file() {
        return Err(Error::SnapshotDirectory);
    }
    Ok(directory)
}

pub fn read_config(directory: &ProtectedDir) -> Result<NodeConfig> {
    let bytes = directory.read(CONFIG_FILE, MAX_CONFIG_BYTES)?;
    Ok(serde_json::from_slice(&bytes)?)
}

pub fn write_config(directory: &ProtectedDir, config: &NodeConfig) -> Result<()> {
    directory.replace(CONFIG_FILE, &serde_json::to_vec(config)?)?;
    Ok(())
}

/// The database and every SQLite sidecar must stay owner-only. A sidecar
/// created by an older run at 0644 would otherwise leak data.
pub fn verify_database_files(directory: &ProtectedDir) -> io::Result<()> {
    directory.verify_file(DATABASE_FILE)?;
    for sidecar in ["node.db-wal", "node.db-shm", "node.db-journal"] {
        match directory.verify_file(sidecar) {
            Ok(()) => (),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Create an empty database file with mode 0600 before SQLite opens it.
/// Otherwise SQLite creates 0644 under a permissive umask and the next open
/// fails the owner-only check.
pub fn create_database_file(directory: &ProtectedDir) -> Result<()> {
    directory.write_new(DATABASE_FILE, &[])?;
    Ok(())
}

pub fn read_secret<const N: usize>(
    directory: &ProtectedDir,
    file: &'static str,
) -> Result<[u8; N]> {
    let bytes = directory.read(file, MAX_SECRET_BYTES)?;
    bytes.try_into().map_err(|_| Error::InvalidKeyLength(file))
}

pub fn tls_identity(directory: &ProtectedDir) -> Result<harmion_net_native::tls::TlsIdentity> {
    Ok(harmion_net_native::tls::TlsIdentity::from_der(
        directory.read(CERTIFICATE_FILE, MAX_SECRET_BYTES)?,
        directory.read(PRIVATE_KEY_FILE, MAX_SECRET_BYTES)?,
    )?)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OAuthIdentity {
    pub root_der: Vec<u8>,
    pub leaf_der: Vec<u8>,
    pub private_key_der: Vec<u8>,
}

/// Load the browser OAuth TLS identity, provisioning it on first use, and
/// return both the identity and its PEM trust root.
pub fn oauth_tls(directory: &ProtectedDir) -> Result<(BrowserTls, String)> {
    match directory.read(OAUTH_IDENTITY_FILE, MAX_SECRET_BYTES) {
        Ok(bytes) => {
            let identity: OAuthIdentity = serde_json::from_slice(&bytes)?;
            let tls = BrowserTls {
                root_der: identity.root_der,
                leaf_der: identity.leaf_der,
                private_key_der: identity.private_key_der,
            };
            tls.server_config()?;
            let pem = tls.root_pem();
            Ok((tls, pem))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // Keep the daemon's historical label: the first four node-id bytes.
            let label = match read_config(directory) {
                Ok(config) if config.node_id.len() >= 8 => {
                    format!("OAuth {}", &config.node_id[..8])
                }
                _ => "OAuth harmion".to_owned(),
            };
            let tls = BrowserTls::generate(&label)?;
            write_oauth_identity(directory, &tls, false)?;
            let pem = tls.root_pem();
            Ok((tls, pem))
        }
        Err(error) => Err(error.into()),
    }
}

pub fn write_oauth_identity(
    directory: &ProtectedDir,
    tls: &BrowserTls,
    replace: bool,
) -> Result<()> {
    let bytes = serde_json::to_vec(&OAuthIdentity {
        root_der: tls.root_der.clone(),
        leaf_der: tls.leaf_der.clone(),
        private_key_der: tls.private_key_der.clone(),
    })?;
    if replace {
        directory.replace(OAUTH_IDENTITY_FILE, &bytes)?;
    } else {
        directory.write_new(OAUTH_IDENTITY_FILE, &bytes)?;
    }
    Ok(())
}

/// Read the untyped administrative credential. Only the host process may call
/// this; it is never a response body on the App API.
pub fn admin_secret(directory: &ProtectedDir) -> Result<[u8; 32]> {
    read_secret(directory, ADMIN_SECRET_FILE)
}

pub fn iroh_secret(directory: &ProtectedDir) -> Result<[u8; 32]> {
    read_secret(directory, IROH_SECRET_FILE)
}
