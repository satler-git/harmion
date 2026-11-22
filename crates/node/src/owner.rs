//! Node-verified owner approvals.
//!
//! The owner signs a canonical statement with an OpenSSH key. The node verifies
//! the SSHSIG with `ssh-keygen -Y verify` and consumes a single-use challenge.
//! No private key or agent connection is held by the node. This module is
//! independent of the admin UI: a stolen `admin.secret` alone cannot satisfy an
//! owner-required operation.

use crate::{
    Error,
    access::{self, Credential},
    storage::Transaction,
};
use harmion_protocol::{
    AppId, hex,
    owner::{self as protocol, Challenge, ChallengeRequest},
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

pub const MECHANISM_SSH: u8 = 1;
pub const MAX_PENDING_CHALLENGES: u32 = 16;
const MAX_PUBLIC_KEY_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnerCredential {
    pub id: [u8; 32],
    pub mechanism: u8,
    pub public_key: String,
    pub fingerprint: String,
    pub label: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
    pub revoked: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct OwnerPolicy {
    pub enforce: bool,
    pub recovery_hash: Option<[u8; 32]>,
    pub updated_at: i64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnerChallenge {
    pub id: [u8; 32],
    pub app: AppId,
    pub statement_hash: [u8; 32],
    pub session: Option<[u8; 32]>,
    pub operation: String,
    pub target: Option<[u8; 32]>,
    pub args_hash: [u8; 32],
    pub issued_at: i64,
    pub expires_at: i64,
    pub used_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnerAudit {
    pub seq: u64,
    pub at: i64,
    pub actor: String,
    pub operation: String,
    pub target: Option<[u8; 32]>,
    pub args_hash: [u8; 32],
    pub credential: Option<[u8; 32]>,
    pub challenge: Option<[u8; 32]>,
    pub session: Option<[u8; 32]>,
    pub result: String,
    pub prev_hash: [u8; 32],
    pub entry_hash: [u8; 32],
}

#[derive(Clone, Debug, Serialize)]
pub struct OwnerStatus {
    pub enforce: bool,
    pub recovery_configured: bool,
    pub credentials: Vec<OwnerCredentialView>,
    pub audit: Vec<OwnerAuditView>,
    /// The returned audit prefix is internally consistent. A broken chain means
    /// the local DB was modified outside the node's transaction path.
    pub audit_valid: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct OwnerCredentialView {
    pub id: String,
    pub mechanism: u8,
    pub fingerprint: String,
    pub label: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub revoked: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct OwnerAuditView {
    pub seq: String,
    pub at: String,
    pub actor: String,
    pub operation: String,
    pub target: Option<String>,
    pub args_sha256: String,
    pub credential: Option<String>,
    pub challenge: Option<String>,
    pub session: Option<String>,
    pub result: String,
    pub entry_hash: String,
}

pub fn status(tx: &mut dyn Transaction) -> Result<OwnerStatus, Error> {
    let policy = tx.owner_policy()?;
    let credentials = tx
        .owner_credentials()?
        .into_iter()
        .map(|credential| OwnerCredentialView {
            id: hex(&credential.id),
            mechanism: credential.mechanism,
            fingerprint: credential.fingerprint,
            label: credential.label,
            created_at: credential.created_at.to_string(),
            last_used_at: credential.last_used_at.map(|at| at.to_string()),
            revoked: credential.revoked,
        })
        .collect();
    let audit_entries = tx.owner_audit_entries(32)?;
    let audit_valid = audit_entries
        .iter()
        .all(|entry| audit_entry_hash(entry.prev_hash, entry) == entry.entry_hash)
        && audit_entries
            .windows(2)
            .all(|pair| pair[0].prev_hash == pair[1].entry_hash);
    let audit = audit_entries
        .into_iter()
        .map(|entry| OwnerAuditView {
            seq: entry.seq.to_string(),
            at: entry.at.to_string(),
            actor: entry.actor,
            operation: entry.operation,
            target: entry.target.map(|id| hex(&id)),
            args_sha256: hex(&entry.args_hash),
            credential: entry.credential.map(|id| hex(&id)),
            challenge: entry.challenge.map(|id| hex(&id)),
            session: entry.session.map(|id| hex(&id)),
            result: entry.result,
            entry_hash: hex(&entry.entry_hash),
        })
        .collect();
    Ok(OwnerStatus {
        enforce: policy.enforce,
        recovery_configured: policy.recovery_hash.is_some(),
        credentials,
        audit,
        audit_valid,
    })
}

/// Issue a single-use challenge for one owner-required operation.
pub fn challenge(
    tx: &mut dyn Transaction,
    credential: &Credential,
    request: ChallengeRequest,
    id: [u8; 32],
    now: i64,
) -> Result<Challenge, Error> {
    let principal = access::authenticate(tx, credential, now)?;
    if !principal.app.grants.is_administrator() {
        return Err(Error::Forbidden);
    }
    if !protocol::valid_operation(&request.operation) {
        return Err(Error::InvalidRequest);
    }
    let args_hash = harmion_protocol::unhex::<32>(&request.args_sha256)?;
    let target = request
        .target
        .as_deref()
        .map(harmion_protocol::unhex::<32>)
        .transpose()?;
    let session = request
        .session
        .as_deref()
        .map(harmion_protocol::unhex::<32>)
        .transpose()?;
    if !protocol::requires_assertion(&request.operation) {
        return Err(Error::InvalidRequest);
    }
    tx.purge_owner_challenges(now)?;
    if tx.owner_pending_challenges(principal.app.id, now)? >= MAX_PENDING_CHALLENGES {
        return Err(Error::OwnerCapacity);
    }
    let identity = tx.identity();
    let expires_at = now.saturating_add(protocol::CHALLENGE_TTL_MICROS);
    let statement = protocol::statement(
        identity.cluster.0,
        identity.node.0,
        &request.operation,
        target,
        args_hash,
        session,
        id,
        now,
        expires_at,
    );
    tx.insert_owner_challenge(&OwnerChallenge {
        id,
        app: principal.app.id,
        statement_hash: protocol::statement_hash(&statement),
        session,
        operation: request.operation,
        target,
        args_hash,
        issued_at: now,
        expires_at,
        used_at: None,
    })?;
    Ok(Challenge {
        challenge: hex(&id),
        statement: hex(&statement),
        expires_at: expires_at.to_string(),
    })
}

/// Verify and consume an owner assertion for an owner-required operation.
///
/// Returns `Ok(())` without consuming anything when the node has no enforcing
/// owner policy. Once `enforce` is set, `admin.secret` alone is insufficient.
// The explicit fields are the complete authorization context; a struct would
// make it easier to accidentally omit one at a call site.
#[allow(clippy::too_many_arguments)]
pub fn authorize(
    tx: &mut dyn Transaction,
    credential: &Credential,
    operation: &str,
    body: &[u8],
    challenge_id: Option<[u8; 32]>,
    signature: Option<&[u8]>,
    actor: &str,
    now: i64,
) -> Result<(), Error> {
    // Even when owner enforcement is off, an R2 operation must be made with an
    // administrator credential. Otherwise any 32-byte bearer could revoke an
    // owner or disable enforcement before the first owner is enrolled.
    let principal = access::authenticate(tx, credential, now)?;
    if !principal.app.grants.is_administrator() {
        return Err(Error::Forbidden);
    }
    let policy = tx.owner_policy()?;
    if !policy.enforce {
        return Ok(());
    }
    let challenge_id = challenge_id.ok_or(Error::OwnerAssertionRequired)?;
    let signature = signature.ok_or(Error::OwnerAssertionRequired)?;
    let challenge = tx
        .owner_challenge(challenge_id)?
        .ok_or(Error::OwnerChallengeExpired)?;
    if challenge.used_at.is_some() {
        return Err(Error::OwnerChallengeUsed);
    }
    if challenge.expires_at <= now || challenge.issued_at > now {
        return Err(Error::OwnerChallengeExpired);
    }
    if challenge.app != principal.app.id
        || challenge.operation != operation
        || challenge.args_hash != protocol::args_hash(body)
    {
        return Err(Error::OwnerAssertionInvalid);
    }
    let identity = tx.identity();
    let statement = protocol::statement(
        identity.cluster.0,
        identity.node.0,
        &challenge.operation,
        challenge.target,
        challenge.args_hash,
        challenge.session,
        challenge.id,
        challenge.issued_at,
        challenge.expires_at,
    );
    if protocol::statement_hash(&statement) != challenge.statement_hash {
        return Err(Error::CorruptLog);
    }
    let active: Vec<OwnerCredential> = tx
        .owner_credentials()?
        .into_iter()
        .filter(|credential| !credential.revoked)
        .collect();
    if active.is_empty() {
        return Err(Error::OwnerAssertionRequired);
    }
    let keys: Vec<String> = active
        .iter()
        .map(|credential| credential.public_key.clone())
        .collect();
    let used_key = verify_ssh(&keys, &statement, signature)?;
    if !tx.consume_owner_challenge(challenge_id, now)? {
        return Err(Error::OwnerChallengeUsed);
    }
    let mut used_credential = None;
    if let Some(mut used) = active
        .into_iter()
        .find(|credential| credential.public_key == used_key)
    {
        used_credential = Some(used.id);
        used.last_used_at = Some(now);
        tx.update_owner_credential(&used)?;
    }
    append_audit(
        tx,
        OwnerAudit {
            seq: 0,
            at: now,
            actor: actor.to_owned(),
            operation: operation.to_owned(),
            target: challenge.target,
            args_hash: challenge.args_hash,
            credential: used_credential,
            challenge: Some(challenge_id),
            session: challenge.session,
            result: "authorized".to_owned(),
            prev_hash: [0; 32],
            entry_hash: [0; 32],
        },
    )?;
    Ok(())
}

/// Register the first owner credential. Host-only: called while the daemon is
/// stopped, with the node directory opened directly.
pub fn enroll_initial(
    tx: &mut dyn Transaction,
    public_key: &str,
    label: &str,
    recovery_code: &[u8; 32],
    id: [u8; 32],
    now: i64,
) -> Result<OwnerCredential, Error> {
    if !tx.owner_credentials()?.is_empty() {
        return Err(Error::AlreadyBootstrapped);
    }
    let (public_key, fingerprint) = validate_public_key(public_key)?;
    let credential = OwnerCredential {
        id,
        mechanism: MECHANISM_SSH,
        public_key,
        fingerprint,
        label: normalize_label(label)?,
        created_at: now,
        last_used_at: None,
        revoked: false,
    };
    tx.insert_owner_credential(&credential)?;
    tx.put_owner_policy(&OwnerPolicy {
        enforce: true,
        recovery_hash: Some(recovery_hash(recovery_code)),
        updated_at: now,
    })?;
    append_audit(
        tx,
        OwnerAudit {
            seq: 0,
            at: now,
            actor: "host".to_owned(),
            operation: "owner-enroll".to_owned(),
            target: None,
            args_hash: protocol::args_hash(&[]),
            credential: Some(id),
            challenge: None,
            session: None,
            result: "enrolled".to_owned(),
            prev_hash: [0; 32],
            entry_hash: [0; 32],
        },
    )?;
    Ok(credential)
}

/// Add a backup owner credential. The caller has already verified an existing
/// owner assertion for this request.
pub fn enroll(
    tx: &mut dyn Transaction,
    public_key: &str,
    label: &str,
    id: [u8; 32],
    actor: &str,
    now: i64,
) -> Result<OwnerCredential, Error> {
    let policy = tx.owner_policy()?;
    if !policy.enforce {
        return Err(Error::OwnerAssertionRequired);
    }
    let (public_key, fingerprint) = validate_public_key(public_key)?;
    if tx
        .owner_credentials()?
        .iter()
        .any(|credential| !credential.revoked && credential.fingerprint == fingerprint)
    {
        return Err(Error::InvalidRequest);
    }
    let credential = OwnerCredential {
        id,
        mechanism: MECHANISM_SSH,
        public_key,
        fingerprint,
        label: normalize_label(label)?,
        created_at: now,
        last_used_at: None,
        revoked: false,
    };
    tx.insert_owner_credential(&credential)?;
    append_audit(
        tx,
        OwnerAudit {
            seq: 0,
            at: now,
            actor: actor.to_owned(),
            operation: "owner-enroll".to_owned(),
            target: None,
            args_hash: protocol::args_hash(&[]),
            credential: Some(id),
            challenge: None,
            session: None,
            result: "enrolled".to_owned(),
            prev_hash: [0; 32],
            entry_hash: [0; 32],
        },
    )?;
    Ok(credential)
}

pub fn revoke(tx: &mut dyn Transaction, id: [u8; 32], actor: &str, now: i64) -> Result<(), Error> {
    let credentials = tx.owner_credentials()?;
    let mut credential = credentials
        .iter()
        .find(|credential| credential.id == id)
        .cloned()
        .ok_or(Error::NotFound)?;
    if credential.revoked {
        return Ok(());
    }
    if credentials
        .iter()
        .filter(|credential| !credential.revoked)
        .count()
        == 1
        && tx.owner_policy()?.recovery_hash.is_none()
    {
        return Err(Error::Forbidden);
    }
    credential.revoked = true;
    tx.update_owner_credential(&credential)?;
    append_audit(
        tx,
        OwnerAudit {
            seq: 0,
            at: now,
            actor: actor.to_owned(),
            operation: "owner-revoke".to_owned(),
            target: None,
            args_hash: protocol::args_hash(&[]),
            credential: Some(id),
            challenge: None,
            session: None,
            result: "revoked".to_owned(),
            prev_hash: [0; 32],
            entry_hash: [0; 32],
        },
    )?;
    Ok(())
}

/// Replace every owner credential with a new one. The recovery code is a
/// one-time offline secret; a new code is returned for the caller to print.
pub fn recover(
    tx: &mut dyn Transaction,
    recovery_code: &[u8; 32],
    public_key: &str,
    label: &str,
    new_recovery_code: &[u8; 32],
    id: [u8; 32],
    now: i64,
) -> Result<OwnerCredential, Error> {
    let policy = tx.owner_policy()?;
    let expected = policy.recovery_hash.ok_or(Error::OwnerRecoveryInvalid)?;
    if expected != recovery_hash(recovery_code) {
        return Err(Error::OwnerRecoveryInvalid);
    }
    let mut credentials = tx.owner_credentials()?;
    for credential in credentials
        .iter_mut()
        .filter(|credential| !credential.revoked)
    {
        credential.revoked = true;
        tx.update_owner_credential(credential)?;
    }
    let (public_key, fingerprint) = validate_public_key(public_key)?;
    let credential = OwnerCredential {
        id,
        mechanism: MECHANISM_SSH,
        public_key,
        fingerprint,
        label: normalize_label(label)?,
        created_at: now,
        last_used_at: None,
        revoked: false,
    };
    tx.insert_owner_credential(&credential)?;
    tx.put_owner_policy(&OwnerPolicy {
        enforce: true,
        recovery_hash: Some(recovery_hash(new_recovery_code)),
        updated_at: now,
    })?;
    append_audit(
        tx,
        OwnerAudit {
            seq: 0,
            at: now,
            actor: "recovery".to_owned(),
            operation: "owner-recover".to_owned(),
            target: None,
            args_hash: protocol::args_hash(&[]),
            credential: Some(id),
            challenge: None,
            session: None,
            result: "recovered".to_owned(),
            prev_hash: [0; 32],
            entry_hash: [0; 32],
        },
    )?;
    Ok(credential)
}

pub fn set_enforce(
    tx: &mut dyn Transaction,
    enforce: bool,
    actor: &str,
    now: i64,
) -> Result<(), Error> {
    if enforce
        && !tx
            .owner_credentials()?
            .iter()
            .any(|credential| !credential.revoked)
    {
        return Err(Error::OwnerAssertionRequired);
    }
    let mut policy = tx.owner_policy()?;
    policy.enforce = enforce;
    policy.updated_at = now;
    tx.put_owner_policy(&policy)?;
    append_audit(
        tx,
        OwnerAudit {
            seq: 0,
            at: now,
            actor: actor.to_owned(),
            operation: "owner-enforce".to_owned(),
            target: None,
            args_hash: protocol::args_hash(&[]),
            credential: None,
            challenge: None,
            session: None,
            result: if enforce { "enforced" } else { "disabled" }.to_owned(),
            prev_hash: [0; 32],
            entry_hash: [0; 32],
        },
    )?;
    Ok(())
}

pub fn recovery_hash(code: &[u8; 32]) -> [u8; 32] {
    Sha256::new()
        .chain_update(b"harmion:owner-recovery:v1\0")
        .chain_update(code)
        .finalize()
        .into()
}

pub fn audit_entry_hash(previous: [u8; 32], audit: &OwnerAudit) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"harmion:owner-audit:v1\0");
    hash.update(previous);
    field(&mut hash, &audit.at.to_be_bytes());
    field(&mut hash, audit.actor.as_bytes());
    field(&mut hash, audit.operation.as_bytes());
    field(
        &mut hash,
        audit.target.as_ref().map_or(&[][..], |target| target),
    );
    field(&mut hash, &audit.args_hash);
    field(
        &mut hash,
        audit
            .credential
            .as_ref()
            .map_or(&[][..], |credential| credential),
    );
    field(
        &mut hash,
        audit
            .challenge
            .as_ref()
            .map_or(&[][..], |challenge| challenge),
    );
    field(
        &mut hash,
        audit.session.as_ref().map_or(&[][..], |session| session),
    );
    field(&mut hash, audit.result.as_bytes());
    hash.finalize().into()
}

fn field(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_be_bytes());
    hash.update(bytes);
}

fn append_audit(tx: &mut dyn Transaction, audit: OwnerAudit) -> Result<(), Error> {
    tx.append_owner_audit(&audit)?;
    Ok(())
}

fn normalize_label(label: &str) -> Result<String, Error> {
    let label = label.trim();
    if label.len() > protocol::MAX_LABEL_BYTES || label.chars().any(char::is_control) {
        return Err(Error::InvalidRequest);
    }
    if label.is_empty() {
        return Ok("owner".to_owned());
    }
    Ok(label.to_owned())
}

/// Parse and validate an OpenSSH public key, returning its normalized
/// `algorithm base64` form and `SHA256:` fingerprint.
pub fn validate_public_key(input: &str) -> Result<(String, String), Error> {
    if input.len() > MAX_PUBLIC_KEY_BYTES {
        return Err(Error::InvalidRequest);
    }
    let line = input
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .ok_or(Error::InvalidRequest)?;
    let mut parts = line.split_whitespace();
    let algorithm = parts.next().ok_or(Error::InvalidRequest)?;
    let encoded = parts.next().ok_or(Error::InvalidRequest)?;
    if !(algorithm.starts_with("ssh-")
        || algorithm.starts_with("ecdsa-")
        || algorithm.starts_with("sk-"))
    {
        return Err(Error::InvalidRequest);
    }
    let normalized = format!("{algorithm} {encoded}");
    let directory = temp_dir()?;
    let result = (|| {
        let path = directory.join("key.pub");
        fs::write(&path, format!("{normalized}\n")).map_err(storage)?;
        let output = Command::new("ssh-keygen")
            .args(["-l", "-f"])
            .arg(&path)
            .output()
            .map_err(|_| Error::OwnerVerifierUnavailable)?;
        if !output.status.success() {
            return Err(Error::InvalidRequest);
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let fingerprint = text
            .split_whitespace()
            .nth(1)
            .filter(|value| value.starts_with("SHA256:"))
            .ok_or(Error::InvalidRequest)?;
        Ok((normalized.clone(), fingerprint.to_owned()))
    })();
    let _ = fs::remove_dir_all(directory);
    result
}

/// Verify an OpenSSH SSHSIG over `statement` with one of `public_keys`.
///
/// `ssh-keygen` performs the cryptographic verification, so Ed25519, ECDSA,
/// RSA and FIDO2 `sk-*` keys use the same OpenSSH rules as the signer. The
/// matched normalized public key is returned for the audit.
pub fn verify_ssh(
    public_keys: &[String],
    statement: &[u8],
    signature_pem: &[u8],
) -> Result<String, Error> {
    if signature_pem.is_empty() || signature_pem.len() > protocol::MAX_SSH_SIGNATURE_BYTES {
        return Err(Error::OwnerAssertionInvalid);
    }
    let directory = temp_dir()?;
    let result = (|| {
        let mut allowed = String::new();
        for key in public_keys {
            allowed.push_str("owner ");
            allowed.push_str(key);
            allowed.push('\n');
        }
        fs::write(directory.join("allowed_signers"), allowed).map_err(storage)?;
        let statement_path = directory.join("statement");
        fs::write(&statement_path, statement).map_err(storage)?;
        let signature_path = directory.join("signature");
        fs::write(&signature_path, signature_pem).map_err(storage)?;
        let stdin = fs::File::open(&statement_path).map_err(storage)?;
        let output = Command::new("ssh-keygen")
            .args(["-Y", "verify", "-f"])
            .arg(directory.join("allowed_signers"))
            .args(["-I", "owner", "-n", protocol::NAMESPACE, "-s"])
            .arg(&signature_path)
            .args(["-O", "print-pubkey"])
            .stdin(Stdio::from(stdin))
            .env("LC_ALL", "C")
            .output()
            .map_err(|_| Error::OwnerVerifierUnavailable)?;
        if !output.status.success() {
            return Err(Error::OwnerAssertionInvalid);
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let line = text
            .lines()
            .map(str::trim)
            .find(|line| {
                line.starts_with("ssh-") || line.starts_with("ecdsa-") || line.starts_with("sk-")
            })
            .ok_or(Error::OwnerAssertionInvalid)?;
        let mut parts = line.split_whitespace();
        let algorithm = parts.next().ok_or(Error::OwnerAssertionInvalid)?;
        let encoded = parts.next().ok_or(Error::OwnerAssertionInvalid)?;
        let matched = format!("{algorithm} {encoded}");
        if !public_keys.iter().any(|key| key == &matched) {
            return Err(Error::OwnerAssertionInvalid);
        }
        Ok(matched)
    })();
    let _ = fs::remove_dir_all(directory);
    result
}

fn temp_dir() -> Result<PathBuf, Error> {
    let base = std::env::temp_dir();
    for attempt in 0..16u32 {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let path = base.join(format!(
            "harmion-owner-{}-{nanos}-{attempt}",
            std::process::id()
        ));
        match create_private_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(storage(error)),
        }
    }
    Err(Error::Storage("owner scratch directory unavailable".into()))
}

#[cfg(unix)]
fn create_private_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new().mode(0o700).create(path)
}

#[cfg(not(unix))]
fn create_private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir(path)
}

fn storage(error: io::Error) -> Error {
    Error::Storage(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_hash_is_domain_separated() {
        let code = [7; 32];
        assert_ne!(recovery_hash(&code), code);
        assert_ne!(recovery_hash(&code), recovery_hash(&[8; 32]));
    }

    #[test]
    fn audit_chain_changes_with_the_previous_hash() {
        let audit = OwnerAudit {
            seq: 0,
            at: 10,
            actor: "host".into(),
            operation: "owner-enroll".into(),
            target: None,
            args_hash: [1; 32],
            credential: Some([2; 32]),
            challenge: None,
            session: None,
            result: "enrolled".into(),
            prev_hash: [0; 32],
            entry_hash: [0; 32],
        };
        assert_ne!(
            audit_entry_hash([0; 32], &audit),
            audit_entry_hash([1; 32], &audit)
        );
    }
}
