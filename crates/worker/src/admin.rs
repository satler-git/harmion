//! Read-only Worker admin API.
//!
//! The host enforces Cloudflare Access and the deployment-issued bearer before
//! calling this module; `bearer_matches` is the app-layer check. Only the
//! Peer-only surfaces are exposed: Overview, Members, Peers and provisioning
//! status. App data, conflicts, OAuth and Export are reported as unavailable
//! rather than fabricated.
use harmion_domain::NodeId;
use harmion_node::{
    membership_state::{self, Status},
    storage::{Store, Transaction},
};
use harmion_protocol::{hex, membership::Binding};
use serde_json::{Value, json};
use subtle::ConstantTimeEq;

pub const CONTENT_TYPE: &str = "application/json";

pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Response {
    fn json(status: u16, value: Value) -> Self {
        Self {
            status,
            body: serde_json::to_vec(&value).unwrap_or_default(),
        }
    }
    pub fn unavailable() -> Self {
        Self::json(503, json!({"error": "unavailable"}))
    }
    pub fn invalid() -> Self {
        Self::json(400, json!({"error": "invalid_request"}))
    }
    pub fn not_found() -> Self {
        Self::json(404, json!({"error": "not_found"}))
    }
    pub fn unauthorized() -> Self {
        Self::json(401, json!({"error": "unauthorized"}))
    }
}

/// Constant-time bearer comparison for the app-layer credential. An empty
/// expected token means the host has not configured remote admin access, so all
/// requests fail closed. Cloudflare Access remains the outer layer.
pub fn bearer_matches(provided: Option<&str>, expected: &str) -> bool {
    if expected.is_empty() {
        return false;
    }
    let (Some(provided), expected) = (provided, expected.as_bytes()) else {
        return false;
    };
    let provided = provided.as_bytes();
    provided.len() == expected.len() && bool::from(provided.ct_eq(expected))
}

/// Feature matrix for the UI. Native-only features are explicitly false so the
/// same Svelte application can disable them.
pub fn capabilities() -> Value {
    json!({
        "mode": "worker",
        "read_only": true,
        "auth": {"access": true, "bearer": true},
        "sections": {
            "overview": true,
            "members": true,
            "peers": true,
            "contents": false,
            "conflicts": false,
            "apps": false,
            "export": false
        },
        "operations": {
            "peer_approve": false,
            "peer_join": false,
            "sync": false,
            "provision": false
        },
        "notes": {
            "provision": "host-only provisioning; not exposed through this API",
            "data": "peer-only replica: App data, Query, Subscribe and OAuth are unavailable"
        }
    })
}

/// Reads one allow-listed, read-only operation. Unknown or write operations are
/// rejected; no request body is interpreted as authority.
pub fn handle(operation: &str, store: &mut impl Store, origin: &str, now: i64) -> Response {
    match operation {
        "capabilities" => Response::json(200, capabilities()),
        "provision-status" => match store.transaction(|tx| provision_status(tx)) {
            Ok(value) => Response::json(200, value),
            Err(_) => Response::unavailable(),
        },
        "members" => match store.transaction(|tx| members(tx)) {
            Ok(value) => Response::json(200, value),
            Err(harmion_node::Error::Unauthenticated) => Response::json(200, empty_members()),
            Err(_) => Response::unavailable(),
        },
        "peers" => match store.transaction(|tx| peers(tx)) {
            Ok(value) => Response::json(200, value),
            Err(_) => Response::unavailable(),
        },
        "overview" => match store.transaction(|tx| overview(tx, origin, now)) {
            Ok(value) => Response::json(200, value),
            Err(_) => Response::unavailable(),
        },
        _ => Response::not_found(),
    }
}

fn empty_members() -> Value {
    json!({"members": [], "pending_retirements": [], "confirmed_certificates": 0})
}

fn provision_status(tx: &mut dyn Transaction) -> Result<Value, harmion_node::Error> {
    let identity = tx.identity();
    let configured = tx.membership_identity()?.is_some() && tx.membership_root()?.is_some();
    Ok(json!({
        "configured": configured,
        "cluster_id": hex(&identity.cluster.0),
        "node_id": hex(&identity.node.0),
        "epoch": hex(&identity.epoch.0),
        "membership_identity": tx.membership_identity()?.is_some(),
        "membership_root": tx.membership_root()?.is_some(),
        "pins": tx.known_peer_identities()?.len() as u64,
    }))
}

fn state_label(status: &Status) -> &'static str {
    match status {
        Status::Active(_) => "active",
        Status::Retired => "retired",
        Status::Conflict => "conflict",
        Status::Unconfirmed => "unconfirmed",
    }
}

fn members(tx: &mut dyn Transaction) -> Result<Value, harmion_node::Error> {
    let state = membership_state::state(tx)?;
    let Some(state) = state else {
        return Ok(empty_members());
    };
    let members: Vec<Value> = state
        .nodes
        .iter()
        .map(|(node, status)| json!({"node_id": hex(&node.0), "state": state_label(status)}))
        .collect();
    let pending: Vec<Value> = state
        .pending_retirements
        .iter()
        .map(|(id, (issuer, target))| {
            json!({
                "certificate_id": hex(id),
                "issuer": hex(&issuer.0),
                "target": hex(&target.0),
            })
        })
        .collect();
    Ok(json!({
        "members": members,
        "pending_retirements": pending,
        "confirmed_certificates": state.confirmed.len() as u64,
    }))
}

fn peers(tx: &mut dyn Transaction) -> Result<Value, harmion_node::Error> {
    let pins = tx.known_peer_identities()?;
    let mut result = Vec::new();
    for peer in tx.peers()? {
        let pin = pins.iter().find(|pin| pin.node == peer.node);
        let pinning = pin.map(|pin| match &pin.provenance {
            harmion_node::identity::Provenance::OwnerRegistration => "owner_registration",
            harmion_node::identity::Provenance::OwnerRecovery => "owner_recovery",
            harmion_node::identity::Provenance::AcceptedAdmission(_) => "accepted_admission",
            harmion_node::identity::Provenance::LegacyImport => "legacy_import",
        });
        let transport = match &peer.key {
            harmion_node::membership::PeerKey::TlsCertificate(certificate) => json!({
                "transport": "https",
                "certificate": hex(certificate),
                "endpoint": peer.endpoint,
            }),
            harmion_node::membership::PeerKey::Iroh(key) => json!({
                "transport": "iroh",
                "endpoint_id": hex(key),
                "relay_url": peer.endpoint,
            }),
            harmion_node::membership::PeerKey::SignedHttps(key) => json!({
                "transport": "signed_https",
                "signing_key": hex(key),
                "origin": peer.endpoint,
            }),
        };
        result.push(json!({
            "node_id": hex(&peer.node.0),
            "retired": peer.retired,
            "pinned": pin.is_some(),
            "pin": pin.map(|pin| json!({
                "signing_key": hex(&pin.signing_key),
                "provenance": pinning,
            })),
            "binding": transport,
        }));
    }
    Ok(json!({"peers": result}))
}

fn overview(
    tx: &mut dyn Transaction,
    origin: &str,
    now: i64,
) -> Result<Value, harmion_node::Error> {
    let identity = tx.identity();
    let state = membership_state::state(tx)?;
    let own = state
        .as_ref()
        .and_then(|state| state.nodes.get(&identity.node))
        .cloned();
    let own_registration = json!({
        "version": 4,
        "cluster_id": hex(&identity.cluster.0),
        "node_id": hex(&identity.node.0),
        "signing_key": match &own {
            Some(Status::Active(member)) => hex(&member.signing_key),
            _ => String::new(),
        },
        "cluster_root": tx.membership_root()?.map(|root| hex(root.bytes())),
        "binding": {
            "transport": "signed_https",
            "origin": origin,
        },
    });
    let members = members(tx)?;
    let configured = state.is_some() && tx.membership_root()?.is_some();
    // Sync observations are not produced by the peer-only adapter; the UI must
    // show them as unavailable instead of a fabricated zero.
    Ok(json!({
        "identity": own_registration,
        "mode": "worker",
        "configured": configured,
        "permissions": null,
        "capabilities": capabilities(),
        "metrics": null,
        "members": members,
        "peers": peers(tx)?,
        "peer_transfer": null,
        "sampled_at": now.to_string(),
    }))
}

/// Deterministic helper reused by tests for binding labels without exposing
/// membership bindings as identity.
pub fn binding_label(binding: &Binding) -> &'static str {
    match binding {
        Binding::Https(_) => "https",
        Binding::Iroh(_) => "iroh",
        Binding::SignedHttps(_) => "signed_https",
    }
}

pub fn node_hex(node: NodeId) -> String {
    hex(&node.0)
}

/// The `harmion-peer:v1:` registration code the owner pastes into the native
/// admin to approve this Worker node. It includes the provisioning public key
/// even while the node is still Unconfirmed, so approval does not depend on an
/// already-active membership state.
pub fn registration_code(
    tx: &mut dyn Transaction,
    origin: &str,
    signing_key: &[u8; 32],
) -> Result<Value, harmion_node::Error> {
    let identity = tx.identity();
    let registration = harmion_protocol::PeerRegistration {
        version: 4,
        cluster_id: hex(&identity.cluster.0),
        node_id: hex(&identity.node.0),
        binding: harmion_protocol::PeerBinding::SignedHttps {
            origin: origin.to_owned(),
        },
        signing_key: Some(hex(signing_key)),
        cluster_root: tx.membership_root()?.map(|root| hex(root.bytes())),
    };
    let code = encode_peer_code(&registration)?;
    Ok(json!({"code": code, "registration": registration}))
}

fn encode_peer_code(
    registration: &harmion_protocol::PeerRegistration,
) -> Result<String, harmion_node::Error> {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use std::io::Write;
    let bytes = serde_json::to_vec(registration)
        .map_err(|error| harmion_node::Error::Storage(error.to_string()))?;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(&bytes)
        .map_err(|error| harmion_node::Error::Storage(error.to_string()))?;
    let compressed = encoder
        .finish()
        .map_err(|error| harmion_node::Error::Storage(error.to_string()))?;
    Ok(format!(
        "harmion-peer:v1:z:{}",
        URL_SAFE_NO_PAD.encode(compressed)
    ))
}
