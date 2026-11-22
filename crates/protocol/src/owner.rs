//! Canonical owner-approval statements and risk classes.
//!
//! The node verifies a signature over these bytes. The format is deliberately
//! independent of JSON key order: every field is length-delimited by fixed
//! widths or a single line. A signature is only valid for one cluster/node,
//! operation, body hash, session, challenge and time window.

use crate::hex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// SSH signature namespace. This prevents a signature for another protocol or
/// another application from being accepted as an owner approval.
pub const NAMESPACE: &str = "harmion-owner";
pub const CHALLENGE_HEADER: &str = "x-harmion-owner-challenge";
pub const SIGNATURE_HEADER: &str = "x-harmion-owner-signature";
pub const CHALLENGE_TTL_MICROS: i64 = 120 * 1_000_000;
pub const MAX_SSH_SIGNATURE_BYTES: usize = 16 * 1024;
pub const MAX_OPERATION_BYTES: usize = 64;
pub const MAX_LABEL_BYTES: usize = 128;

const STATEMENT_PREFIX: &[u8] = b"HOA1\0";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Risk {
    /// Read-only observation.
    Read,
    /// Ordinary content mutation.
    Content,
    /// Privilege, trust or data-export change. Requires an owner approval when
    /// the node has an owner policy with `enforce` set.
    Owner,
}

/// Risk of a node App API operation. This is a node-operation mapping, not the
/// admin server's route names.
pub fn risk(operation: &str) -> Risk {
    match operation {
        "change-grants"
        | "issue-app"
        | "approve"
        | "join"
        | "join-recovery"
        | "trust-recovery-peer"
        | "retire"
        | "confirm-retirement"
        | "dismiss-retirement"
        | "provision-manifest"
        | "connect-approve"
        | "interpreter-approve"
        | "oauth-consent"
        | "export"
        | "owner-enroll"
        | "owner-revoke"
        | "owner-enforce" => Risk::Owner,
        "save"
        | "resolve"
        | "merge-input"
        | "oauth-register"
        | "sync"
        | "interpreter-register"
        | "interpreter-call"
        | "interpreter-cancel"
        | "subscribe"
        | "resume"
        | "unsubscribe" => Risk::Content,
        _ => Risk::Read,
    }
}

pub fn requires_assertion(operation: &str) -> bool {
    risk(operation) == Risk::Owner
}

/// Hash of the exact request body bytes. The node recomputes this before
/// executing; a signature cannot be moved to a different body.
pub fn args_hash(body: &[u8]) -> [u8; 32] {
    Sha256::digest(body).into()
}

/// Canonical bytes signed by the owner.
// The fields are deliberately explicit: every one is part of the signed
// domain, and a struct would make accidental field omission harder to spot.
#[allow(clippy::too_many_arguments)]
pub fn statement(
    cluster: [u8; 32],
    node: [u8; 32],
    operation: &str,
    target: Option<[u8; 32]>,
    args_hash: [u8; 32],
    session: Option<[u8; 32]>,
    challenge: [u8; 32],
    issued_at: i64,
    expires_at: i64,
) -> Vec<u8> {
    let target = target.map_or_else(|| "-".to_owned(), |id| hex(&id));
    let session = session.map_or_else(|| "-".to_owned(), |id| hex(&id));
    let mut bytes = Vec::with_capacity(STATEMENT_PREFIX.len() + 256 + operation.len());
    bytes.extend_from_slice(STATEMENT_PREFIX);
    bytes.extend_from_slice(format!("cluster={}\n", hex(&cluster)).as_bytes());
    bytes.extend_from_slice(format!("node={}\n", hex(&node)).as_bytes());
    bytes.extend_from_slice(format!("operation={operation}\n").as_bytes());
    bytes.extend_from_slice(format!("target={target}\n").as_bytes());
    bytes.extend_from_slice(format!("args_sha256={}\n", hex(&args_hash)).as_bytes());
    bytes.extend_from_slice(format!("session={session}\n").as_bytes());
    bytes.extend_from_slice(format!("challenge={}\n", hex(&challenge)).as_bytes());
    bytes.extend_from_slice(format!("issued_at={issued_at}\n").as_bytes());
    bytes.extend_from_slice(format!("expires_at={expires_at}\n").as_bytes());
    bytes
}

pub fn statement_hash(statement: &[u8]) -> [u8; 32] {
    Sha256::digest(statement).into()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChallengeRequest {
    pub operation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub args_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Challenge {
    pub challenge: String,
    /// Hex of the canonical bytes to sign. Returned by the node so the signer
    /// never has to reconstruct a different encoding.
    pub statement: String,
    /// Decimal microseconds since the Unix epoch.
    pub expires_at: String,
}

pub fn valid_operation(operation: &str) -> bool {
    !operation.is_empty()
        && operation.len() <= MAX_OPERATION_BYTES
        && operation
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn risk_classes_cover_the_admin_surface() {
        for operation in [
            "change-grants",
            "approve",
            "retire",
            "connect-approve",
            "interpreter-approve",
            "oauth-consent",
            "export",
            "owner-enroll",
        ] {
            assert_eq!(risk(operation), Risk::Owner, "{operation}");
        }
        assert_eq!(risk("save"), Risk::Content);
        assert_eq!(risk("sync"), Risk::Content);
        assert_eq!(risk("get"), Risk::Read);
        assert!(!requires_assertion("owner-recover"));
    }

    #[test]
    fn statement_is_stable_and_binds_every_field() {
        let cluster = [1; 32];
        let node = [2; 32];
        let args = [3; 32];
        let session = [4; 32];
        let challenge = [5; 32];
        let base = statement(
            cluster,
            node,
            "change-grants",
            None,
            args,
            Some(session),
            challenge,
            10,
            20,
        );
        assert_eq!(
            base,
            statement(
                cluster,
                node,
                "change-grants",
                None,
                args,
                Some(session),
                challenge,
                10,
                20
            )
        );
        assert_ne!(
            base,
            statement(
                cluster,
                node,
                "change-grants",
                Some([9; 32]),
                args,
                Some(session),
                challenge,
                10,
                20
            )
        );
        assert_ne!(
            base,
            statement(
                cluster,
                node,
                "change-grants",
                None,
                args,
                None,
                challenge,
                10,
                20
            )
        );
        assert_ne!(
            base,
            statement(
                cluster,
                node,
                "change-grants",
                None,
                args,
                Some(session),
                challenge,
                10,
                21
            )
        );
        assert!(base.starts_with(b"HOA1\0cluster="));
    }

    #[test]
    fn operation_validation_is_strict() {
        assert!(valid_operation("owner-enroll"));
        assert!(!valid_operation("Owner-Enroll"));
        assert!(!valid_operation("owner_enroll"));
        assert!(!valid_operation(""));
        assert!(!valid_operation(&"a".repeat(MAX_OPERATION_BYTES + 1)));
    }
}
