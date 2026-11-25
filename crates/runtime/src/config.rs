//! Persisted node configuration.
//!
//! `node.json` version 1 is the existing on-disk format shared with the Linux
//! daemon. Android uses the same files so a node directory stays portable
//! between hosts. Android-only settings live in the app layer, not here.
use crate::{Error, Result};
use harmion_domain::{ClusterId, NodeId};
use harmion_node::storage::Identity;
use harmion_protocol::{LogEpoch, unhex};
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

pub const CONFIG_VERSION: u32 = 1;
pub const CONFIG_FILE: &str = "node.json";
pub const CONNECTION_FILE: &str = "connection.json";
pub const PEER_FILE: &str = "peer.json";
pub const DATABASE_FILE: &str = "node.db";
pub const ADMIN_SECRET_FILE: &str = "admin.secret";
pub const CERTIFICATE_FILE: &str = "certificate.der";
pub const PRIVATE_KEY_FILE: &str = "private-key.der";
pub const IROH_SECRET_FILE: &str = "iroh.secret";
pub const OAUTH_IDENTITY_FILE: &str = "oauth-identity.json";
pub const MAX_CONFIG_BYTES: usize = 4096;
pub const MAX_SECRET_BYTES: usize = 65536;

fn enabled() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeConfig {
    pub version: u32,
    pub cluster_id: String,
    pub node_id: String,
    pub log_epoch: String,
    pub app_listen: SocketAddr,
    #[serde(default)]
    pub oauth_listen: Option<SocketAddr>,
    pub peer_listen: SocketAddr,
    pub sync_seconds: u64,
    #[serde(default)]
    pub use_iroh: bool,
    #[serde(default)]
    pub relay_urls: Option<Vec<String>>,
    #[serde(default = "enabled")]
    pub public_lookup: bool,
    #[serde(default)]
    pub founder: bool,
}

impl NodeConfig {
    pub fn new_founder(cluster: ClusterId, node: NodeId, epoch: LogEpoch) -> Self {
        Self {
            version: CONFIG_VERSION,
            cluster_id: harmion_protocol::hex(&cluster.0),
            node_id: harmion_protocol::hex(&node.0),
            log_epoch: harmion_protocol::hex(&epoch.0),
            app_listen: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            oauth_listen: Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)),
            peer_listen: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            sync_seconds: 30,
            use_iroh: true,
            relay_urls: None,
            public_lookup: true,
            founder: true,
        }
    }

    pub fn identity(&self) -> Result<Identity> {
        if self.version != CONFIG_VERSION {
            return Err(Error::UnsupportedConfig(self.version));
        }
        Ok(Identity {
            cluster: ClusterId(unhex(&self.cluster_id)?),
            node: NodeId(unhex(&self.node_id)?),
            epoch: LogEpoch(unhex(&self.log_epoch)?),
        })
    }

    /// Rejects listener and transport combinations that would expose the App
    /// API beyond loopback or make peer sync ambiguous.
    pub fn validate(&self) -> Result<()> {
        if !self.app_listen.ip().is_loopback()
            || self.peer_listen.ip().is_unspecified()
            || self.sync_seconds > 3600
        {
            return Err(Error::InvalidListenerConfig);
        }
        if self
            .oauth_listen
            .is_some_and(|address| address.ip() != IpAddr::V4(Ipv4Addr::LOCALHOST))
        {
            return Err(Error::InvalidOAuthListener);
        }
        if self.use_iroh
            && !self.public_lookup
            && self.relay_urls.as_ref().is_none_or(|urls| urls.is_empty())
        {
            return Err(Error::MissingRelay);
        }
        Ok(())
    }

    /// Applies host overrides before validation. `None` keeps the saved value.
    pub fn apply(&mut self, overrides: &ConfigOverrides) {
        if let Some(address) = overrides.app_listen {
            self.app_listen = address;
        }
        if let Some(address) = overrides.oauth_listen {
            self.oauth_listen = address;
        }
        if let Some(address) = overrides.peer_listen {
            self.peer_listen = address;
        }
        if let Some(seconds) = overrides.sync_seconds {
            self.sync_seconds = seconds;
        }
        if let Some(use_iroh) = overrides.use_iroh {
            self.use_iroh = use_iroh;
        }
        if let Some(urls) = &overrides.relay_urls {
            self.relay_urls = Some(urls.clone());
        }
        if let Some(public_lookup) = overrides.public_lookup {
            self.public_lookup = public_lookup;
        }
        if let Some(founder) = overrides.founder {
            self.founder = founder;
        }
    }
}

/// Runtime-only listener and transport overrides. Nothing here is persisted on
/// its own: the resolved configuration is written back to `node.json`.
#[derive(Clone, Debug, Default)]
pub struct ConfigOverrides {
    pub app_listen: Option<SocketAddr>,
    pub oauth_listen: Option<Option<SocketAddr>>,
    pub peer_listen: Option<SocketAddr>,
    pub sync_seconds: Option<u64>,
    pub use_iroh: Option<bool>,
    pub relay_urls: Option<Vec<String>>,
    pub public_lookup: Option<bool>,
    pub founder: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> NodeConfig {
        NodeConfig::new_founder(ClusterId([7; 32]), NodeId([9; 32]), LogEpoch([3; 32]))
    }

    #[test]
    fn round_trips_and_validates_defaults() {
        let config = config();
        config.validate().unwrap();
        let encoded = serde_json::to_vec(&config).unwrap();
        let decoded: NodeConfig = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, config);
        assert_eq!(decoded.identity().unwrap().node, NodeId([9; 32]));
        assert!(decoded.founder);
    }

    #[test]
    fn rejects_reachable_app_api_and_wildcard_peer() {
        let mut value = config();
        value.app_listen = "0.0.0.0:8080".parse().unwrap();
        assert!(matches!(
            value.validate(),
            Err(Error::InvalidListenerConfig)
        ));
        let mut value = config();
        value.peer_listen = "0.0.0.0:0".parse().unwrap();
        assert!(matches!(
            value.validate(),
            Err(Error::InvalidListenerConfig)
        ));
    }

    #[test]
    fn applies_overrides_independently() {
        let mut config = config();
        let overrides = ConfigOverrides {
            sync_seconds: Some(0),
            oauth_listen: Some(None),
            ..ConfigOverrides::default()
        };
        config.apply(&overrides);
        assert_eq!(config.sync_seconds, 0);
        assert_eq!(config.oauth_listen, None);
        assert_eq!(config.app_listen, SocketAddr::from(([127, 0, 0, 1], 0)));
    }

    #[test]
    fn unknown_config_fields_are_rejected() {
        let mut value = serde_json::to_value(config()).unwrap();
        value["android_policy"] = serde_json::json!({"wifi_only": true});
        assert!(serde_json::from_value::<NodeConfig>(value).is_err());
    }
}
