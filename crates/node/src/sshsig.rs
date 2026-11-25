//! In-process OpenSSH public-key and SSHSIG verification.
//!
//! Android has no `ssh-keygen`, so owner assertions must be verifiable without
//! spawning a process. This module parses the exact OpenSSH wire formats and
//! verifies Ed25519 and ECDSA P-256 keys. Algorithms it does not implement
//! (RSA, FIDO2 `sk-*`, certificates) are reported as unsupported so a host with
//! `ssh-keygen` can fall back to it; nothing is verified twice.
//!
//! Format references: OpenSSH `PROTOCOL.sshsig`, RFC 5656 (ECDSA), RFC 8032.
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256, Sha512};

const SSHSIG_MAGIC: &[u8; 6] = b"SSHSIG";
const SSHSIG_VERSION: u32 = 1;
const ED25519: &str = "ssh-ed25519";
const ECDSA_P256: &str = "ecdsa-sha2-nistp256";
const CURVE_P256: &[u8] = b"nistp256";

/// Why a public key line could not be used. `Unsupported` is not a failure: the
/// host may verify it with `ssh-keygen`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeyError {
    Invalid,
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VerifyError {
    Invalid,
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyMaterial {
    Ed25519([u8; 32]),
    P256([u8; 65]),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ParsedKey {
    pub algorithm: String,
    /// The SSH wire-format public key blob (the bytes an authorized_keys line
    /// carries base64-encoded).
    pub blob: Vec<u8>,
    pub normalized: String,
    pub fingerprint: String,
    material: KeyMaterial,
}

/// Parses `algorithm base64 [comment]`. A trailing comment is ignored; the
/// normalized form is only the algorithm and exact base64 text.
pub(crate) fn parse_key_line(line: &str) -> Result<ParsedKey, KeyError> {
    let mut parts = line.split_whitespace();
    let algorithm = parts.next().ok_or(KeyError::Invalid)?;
    let encoded = parts.next().ok_or(KeyError::Invalid)?;
    if !(algorithm.starts_with("ssh-")
        || algorithm.starts_with("ecdsa-")
        || algorithm.starts_with("sk-"))
    {
        return Err(KeyError::Invalid);
    }
    let blob = STANDARD.decode(encoded).map_err(|_| KeyError::Invalid)?;
    // The declared algorithm must match the blob itself. This rejects a
    // certificate label bound to a plain key and prevents the audit and UI
    // from displaying an algorithm the key does not use.
    {
        let mut reader = Reader::new(&blob);
        let declared = reader.string().map_err(|_| KeyError::Invalid)?;
        if declared != algorithm.as_bytes() {
            return Err(KeyError::Invalid);
        }
    }
    let material = parse_key_blob(&blob).map_err(|error| match error {
        VerifyError::Invalid => KeyError::Invalid,
        VerifyError::Unsupported => KeyError::Unsupported,
    })?;
    Ok(ParsedKey {
        algorithm: algorithm.to_owned(),
        fingerprint: fingerprint(&blob),
        normalized: format!("{algorithm} {encoded}"),
        blob,
        material,
    })
}

/// `SHA256:` + unpadded standard base64 of the key blob, exactly as `ssh-keygen -l`
/// prints it.
pub(crate) fn fingerprint(blob: &[u8]) -> String {
    format!("SHA256:{}", STANDARD_NO_PAD.encode(Sha256::digest(blob)))
}

fn parse_key_blob(blob: &[u8]) -> Result<KeyMaterial, VerifyError> {
    let mut reader = Reader::new(blob);
    let algorithm = reader.string().map_err(|_| VerifyError::Invalid)?;
    let algorithm = std::str::from_utf8(algorithm).map_err(|_| VerifyError::Invalid)?;
    let material = match algorithm {
        ED25519 => {
            let key = reader.string().map_err(|_| VerifyError::Invalid)?;
            let key: [u8; 32] = key.try_into().map_err(|_| VerifyError::Invalid)?;
            KeyMaterial::Ed25519(key)
        }
        ECDSA_P256 => {
            let curve = reader.string().map_err(|_| VerifyError::Invalid)?;
            if curve != CURVE_P256 {
                return Err(VerifyError::Unsupported);
            }
            let point = reader.string().map_err(|_| VerifyError::Invalid)?;
            // Uncompressed SEC1 point, as OpenSSH requires.
            if point.len() != 65 || point[0] != 4 {
                return Err(VerifyError::Invalid);
            }
            let mut key = [0u8; 65];
            key.copy_from_slice(point);
            KeyMaterial::P256(key)
        }
        _ => return Err(VerifyError::Unsupported),
    };
    if !reader.remaining().is_empty() {
        return Err(VerifyError::Invalid);
    }
    Ok(material)
}

/// Verifies an armored SSHSIG over `statement` against one of `public_keys`.
///
/// * `Ok(Some(normalized))` — verified; the returned key is the caller's exact
///   authorized-key line form.
/// * `Ok(None)` — the signature uses an algorithm this module does not implement.
/// * `Err` — supported algorithm, but the signature is malformed, for another
///   namespace, or does not verify.
pub(crate) fn verify(
    public_keys: &[String],
    statement: &[u8],
    signature_pem: &[u8],
) -> Result<Option<String>, VerifyError> {
    let binary = decode_armor(signature_pem).map_err(|_| VerifyError::Invalid)?;
    let mut reader = Reader::new(&binary);
    if reader.take(6).map_err(|_| VerifyError::Invalid)? != SSHSIG_MAGIC {
        return Err(VerifyError::Invalid);
    }
    if reader.u32().map_err(|_| VerifyError::Invalid)? != SSHSIG_VERSION {
        return Err(VerifyError::Invalid);
    }
    let embedded_key = reader.string().map_err(|_| VerifyError::Invalid)?;
    let namespace = reader.string().map_err(|_| VerifyError::Invalid)?;
    let reserved = reader.string().map_err(|_| VerifyError::Invalid)?;
    let hash_algorithm = reader.string().map_err(|_| VerifyError::Invalid)?;
    let signature_blob = reader.string().map_err(|_| VerifyError::Invalid)?;
    if !reader.remaining().is_empty() {
        return Err(VerifyError::Invalid);
    }
    if namespace != harmion_protocol::owner::NAMESPACE.as_bytes() {
        return Err(VerifyError::Invalid);
    }
    let material = match parse_key_blob(embedded_key) {
        Ok(material) => material,
        Err(VerifyError::Unsupported) => return Ok(None),
        Err(VerifyError::Invalid) => return Err(VerifyError::Invalid),
    };
    let hash = match hash_algorithm {
        b"sha256" => Sha256::digest(statement).to_vec(),
        b"sha512" => Sha512::digest(statement).to_vec(),
        // OpenSSH defines exactly these two. Do not guess.
        _ => return Err(VerifyError::Invalid),
    };
    let signed = signed_bytes(namespace, reserved, hash_algorithm, &hash);
    verify_signature(material, &signed, signature_blob)?;
    for key in public_keys {
        if let Ok(parsed) = parse_key_line(key)
            && parsed.blob == embedded_key
        {
            return Ok(Some(parsed.normalized));
        }
    }
    Err(VerifyError::Invalid)
}

fn verify_signature(
    material: KeyMaterial,
    signed: &[u8],
    signature_blob: &[u8],
) -> Result<(), VerifyError> {
    let mut reader = Reader::new(signature_blob);
    let algorithm = reader.string().map_err(|_| VerifyError::Invalid)?;
    let signature = reader.string().map_err(|_| VerifyError::Invalid)?;
    if !reader.remaining().is_empty() {
        return Err(VerifyError::Invalid);
    }
    match material {
        KeyMaterial::Ed25519(key) => {
            if algorithm != ED25519.as_bytes() {
                return Err(VerifyError::Invalid);
            }
            let signature: [u8; 64] = signature.try_into().map_err(|_| VerifyError::Invalid)?;
            VerifyingKey::from_bytes(&key)
                .map_err(|_| VerifyError::Invalid)?
                .verify_strict(signed, &Signature::from_bytes(&signature))
                .map_err(|_| VerifyError::Invalid)?;
            Ok(())
        }
        KeyMaterial::P256(point) => {
            if algorithm != ECDSA_P256.as_bytes() {
                return Err(VerifyError::Invalid);
            }
            // SSH encodes ECDSA signatures as mpint r || mpint s; ring wants the
            // fixed-width big-endian concatenation.
            let mut ecdsa = Reader::new(signature);
            let r = ecdsa.string().map_err(|_| VerifyError::Invalid)?;
            let s = ecdsa.string().map_err(|_| VerifyError::Invalid)?;
            if !ecdsa.remaining().is_empty() {
                return Err(VerifyError::Invalid);
            }
            let mut fixed = [0u8; 64];
            fixed[..32].copy_from_slice(&mpint_fixed(r)?);
            fixed[32..].copy_from_slice(&mpint_fixed(s)?);
            ring::signature::UnparsedPublicKey::new(
                &ring::signature::ECDSA_P256_SHA256_FIXED,
                &point,
            )
            .verify(signed, &fixed)
            .map_err(|_| VerifyError::Invalid)
        }
    }
}

fn signed_bytes(namespace: &[u8], reserved: &[u8], hash_algorithm: &[u8], hash: &[u8]) -> Vec<u8> {
    let mut signed = Vec::with_capacity(
        6 + 4 * 4 + namespace.len() + reserved.len() + hash_algorithm.len() + hash.len(),
    );
    signed.extend_from_slice(SSHSIG_MAGIC);
    put_string(&mut signed, namespace);
    put_string(&mut signed, reserved);
    put_string(&mut signed, hash_algorithm);
    put_string(&mut signed, hash);
    signed
}

/// Public key line and fingerprint for a raw Ed25519 seed, in the same form as a
/// registered owner key. The seed is the private half; callers keep it wrapped.
pub(crate) fn ed25519_public_key(seed: &[u8; 32]) -> (String, String) {
    let signing = SigningKey::from_bytes(seed);
    let public = signing.verifying_key().to_bytes();
    let mut blob = Vec::new();
    put_string(&mut blob, ED25519.as_bytes());
    put_string(&mut blob, &public);
    (
        format!("{} {}", ED25519, STANDARD_NO_PAD.encode(&blob)),
        fingerprint(&blob),
    )
}

/// Signs `statement` and returns an armored SSHSIG. SHA-512 over the message is
/// what OpenSSH uses by default, so the result is indistinguishable from
/// `ssh-keygen -Y sign` output for the same key and namespace.
pub(crate) fn sign_ed25519(seed: &[u8; 32], namespace: &str, statement: &[u8]) -> String {
    let signing = SigningKey::from_bytes(seed);
    let public = signing.verifying_key().to_bytes();
    let mut public_blob = Vec::new();
    put_string(&mut public_blob, ED25519.as_bytes());
    put_string(&mut public_blob, &public);

    let hash = Sha512::digest(statement).to_vec();
    let signed = signed_bytes(namespace.as_bytes(), b"", b"sha512", &hash);
    let signature = signing.sign(&signed).to_bytes();
    let mut inner = Vec::new();
    put_string(&mut inner, ED25519.as_bytes());
    put_string(&mut inner, &signature);

    let mut binary = Vec::new();
    binary.extend_from_slice(SSHSIG_MAGIC);
    binary.extend_from_slice(&SSHSIG_VERSION.to_be_bytes());
    put_string(&mut binary, &public_blob);
    put_string(&mut binary, namespace.as_bytes());
    put_string(&mut binary, b"");
    put_string(&mut binary, b"sha512");
    put_string(&mut binary, &inner);
    armor(&binary)
}

fn armor(binary: &[u8]) -> String {
    let encoded = STANDARD.encode(binary);
    let mut output = String::from("-----BEGIN SSH SIGNATURE-----\n");
    for chunk in encoded.as_bytes().chunks(70) {
        // Base64 is ASCII; a lossy conversion cannot change these bytes.
        output.push_str(&String::from_utf8_lossy(chunk));
        output.push('\n');
    }
    output.push_str("-----END SSH SIGNATURE-----\n");
    output
}
fn put_string(output: &mut Vec<u8>, value: &[u8]) {
    output.extend_from_slice(&(value.len() as u32).to_be_bytes());
    output.extend_from_slice(value);
}

/// Decodes an mpint into a fixed 32-byte big-endian scalar, rejecting negative
/// or oversized values.
fn mpint_fixed(mpint: &[u8]) -> Result<[u8; 32], VerifyError> {
    // OpenSSH prepends 0x00 to keep a positive value positive; strip the sign
    // byte first, and only then decide whether the leading bit means negative.
    let value = if mpint.first() == Some(&0) {
        &mpint[1..]
    } else {
        if mpint.first().is_some_and(|byte| byte & 0x80 != 0) {
            return Err(VerifyError::Invalid);
        }
        mpint
    };
    if value.len() > 32 {
        return Err(VerifyError::Invalid);
    }
    let mut fixed = [0u8; 32];
    fixed[32 - value.len()..].copy_from_slice(value);
    Ok(fixed)
}

fn decode_armor(pem: &[u8]) -> Result<Vec<u8>, ()> {
    let text = std::str::from_utf8(pem).map_err(|_| ())?;
    let mut encoded = String::new();
    let mut started = false;
    let mut ended = false;
    for line in text.lines() {
        let line = line.trim();
        if line == "-----BEGIN SSH SIGNATURE-----" {
            if started {
                return Err(());
            }
            started = true;
            continue;
        }
        if line == "-----END SSH SIGNATURE-----" {
            ended = true;
            break;
        }
        if !started {
            if line.is_empty() {
                continue;
            }
            return Err(());
        }
        encoded.push_str(line);
    }
    if !started || !ended {
        return Err(());
    }
    STANDARD.decode(encoded).map_err(|_| ())
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], ()> {
        let end = self.offset.checked_add(len).ok_or(())?;
        if end > self.bytes.len() {
            return Err(());
        }
        let value = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(value)
    }

    fn u32(&mut self) -> Result<u32, ()> {
        let bytes = self.take(4)?;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn string(&mut self) -> Result<&'a [u8], ()> {
        let len = self.u32()? as usize;
        self.take(len)
    }

    fn remaining(&self) -> &'a [u8] {
        &self.bytes[self.offset..]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;

    fn ssh_keygen_available() -> bool {
        Command::new("ssh-keygen")
            .arg("-?")
            .output()
            .map(|_| true)
            .unwrap_or(false)
    }

    struct Keypair {
        public: String,
        path: std::path::PathBuf,
    }

    fn generate(directory: &Path, name: &str, key_type: &str, extra: &[&str]) -> Option<Keypair> {
        let path = directory.join(name);
        let mut command = Command::new("ssh-keygen");
        command
            .args(["-q", "-t", key_type, "-N", "", "-C", name, "-f"])
            .arg(&path);
        for arg in extra {
            command.arg(arg);
        }
        if !command.status().ok()?.success() {
            return None;
        }
        let public = std::fs::read_to_string(directory.join(format!("{name}.pub"))).ok()?;
        Some(Keypair { public, path })
    }

    fn sign(key: &Keypair, namespace: &str, statement: &[u8], directory: &Path) -> Vec<u8> {
        let path = directory.join("statement");
        std::fs::write(&path, statement).unwrap();
        let mut signature = path.clone().into_os_string();
        signature.push(".sig");
        let signature = std::path::PathBuf::from(signature);
        // ssh-keygen refuses to overwrite non-interactively; each call in a test
        // must produce a signature for its own namespace and statement.
        let _ = std::fs::remove_file(&signature);
        let status = Command::new("ssh-keygen")
            .args(["-Y", "sign", "-f"])
            .arg(&key.path)
            .args(["-n", namespace])
            .arg(&path)
            .env("LC_ALL", "C")
            .stdin(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
        std::fs::read(signature).unwrap()
    }

    fn fixture(name: &str) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("harmion-sshsig-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn verifies_ed25519_signatures_and_fingerprints() {
        if !ssh_keygen_available() {
            eprintln!("skipping: ssh-keygen is not installed");
            return;
        }
        let directory = fixture("ed25519");
        let key = generate(&directory, "ed25519", "ed25519", &[]).unwrap();
        let normalized = parse_key_line(key.public.trim()).unwrap();
        // The fingerprint must match `ssh-keygen -l` byte for byte.
        let listing = Command::new("ssh-keygen")
            .args(["-l", "-f"])
            .arg(directory.join("ed25519.pub"))
            .output()
            .unwrap();
        let expected = String::from_utf8_lossy(&listing.stdout)
            .split_whitespace()
            .nth(1)
            .unwrap()
            .to_owned();
        assert_eq!(normalized.fingerprint, expected);

        let statement = b"HOA1\0cluster=00\noperation=approve\n";
        let signature = sign(&key, "harmion-owner", statement, &directory);
        let matched = verify(&[key.public.trim().to_owned()], statement, &signature)
            .unwrap()
            .unwrap();
        assert_eq!(matched, normalized.normalized);
        assert_eq!(
            matched,
            key.public
                .split_whitespace()
                .take(2)
                .collect::<Vec<_>>()
                .join(" ")
        );

        // A different statement, namespace or key must not verify.
        assert!(
            verify(
                &[key.public.trim().to_owned()],
                b"another statement",
                &signature
            )
            .is_err()
        );
        let other = generate(&directory, "other", "ed25519", &[]).unwrap();
        assert!(verify(&[other.public.trim().to_owned()], statement, &signature).is_err());
        let foreign = sign(&key, "not-harmion-owner", statement, &directory);
        assert!(verify(&[key.public.trim().to_owned()], statement, &foreign).is_err());
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn verifies_ecdsa_p256_signatures() {
        if !ssh_keygen_available() {
            eprintln!("skipping: ssh-keygen is not installed");
            return;
        }
        let directory = fixture("p256");
        let key = generate(&directory, "p256", "ecdsa", &["-b", "256"]).unwrap();
        let parsed = parse_key_line(key.public.trim()).unwrap();
        assert_eq!(parsed.algorithm, ECDSA_P256);
        let statement = b"HOA1\0cluster=ff\noperation=retire\n";
        let signature = sign(&key, "harmion-owner", statement, &directory);
        assert!(
            verify(&[key.public.trim().to_owned()], statement, &signature)
                .unwrap()
                .is_some()
        );
        assert!(verify(&[key.public.trim().to_owned()], b"other", &signature).is_err());
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn unsupported_algorithms_are_reported_not_rejected() {
        if !ssh_keygen_available() {
            eprintln!("skipping: ssh-keygen is not installed");
            return;
        }
        let directory = fixture("rsa");
        let key = generate(&directory, "rsa", "rsa", &["-b", "2048"]).unwrap();
        assert_eq!(
            parse_key_line(key.public.trim()).unwrap_err(),
            KeyError::Unsupported
        );
        let statement = b"HOA1\0cluster=ff\noperation=export\n";
        let signature = sign(&key, "harmion-owner", statement, &directory);
        assert_eq!(
            verify(&[key.public.trim().to_owned()], statement, &signature).unwrap(),
            None
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn signed_assertions_round_trip_and_interoperate_with_openssh() {
        let seed = [9u8; 32];
        let (public, fingerprint) = ed25519_public_key(&seed);
        let statement = b"HOA1\0cluster=01\noperation=connect-approve\n";
        let signature = sign_ed25519(&seed, "harmion-owner", statement);
        // Our own verifier accepts it.
        let matched = verify(
            std::slice::from_ref(&public),
            statement,
            signature.as_bytes(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(matched, public);
        assert_eq!(parse_key_line(&public).unwrap().fingerprint, fingerprint);
        // A statement signed for another namespace is rejected.
        let foreign = sign_ed25519(&seed, "other-namespace", statement);
        assert!(verify(std::slice::from_ref(&public), statement, foreign.as_bytes()).is_err());

        if !ssh_keygen_available() {
            eprintln!("skipping OpenSSH interop: ssh-keygen is not installed");
            return;
        }
        // OpenSSH must accept a signature produced in-process.
        let directory = fixture("interop");
        std::fs::write(directory.join("allowed"), format!("owner {public}\n")).unwrap();
        std::fs::write(directory.join("statement"), statement).unwrap();
        std::fs::write(directory.join("signature"), &signature).unwrap();
        let stdin = std::fs::File::open(directory.join("statement")).unwrap();
        let output = Command::new("ssh-keygen")
            .args(["-Y", "verify", "-f"])
            .arg(directory.join("allowed"))
            .args(["-I", "owner", "-n", "harmion-owner", "-s"])
            .arg(directory.join("signature"))
            .stdin(std::process::Stdio::from(stdin))
            .env("LC_ALL", "C")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// A committed signature verifies without `ssh-keygen` being installed.
    /// This is the Android case: the node must verify an assertion even though
    /// the platform has no OpenSSH tools. The fixture is public material only.
    #[test]
    fn verifies_a_committed_ed25519_fixture_without_external_tools() {
        const PUBLIC: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIEQA48jpEO00FYkzCP5HvEDl3Jg+7s10JkfQ8b1yAS8z fixture";
        const SIGNATURE: &str = "-----BEGIN SSH SIGNATURE-----
U1NIU0lHAAAAAQAAADMAAAALc3NoLWVkMjU1MTkAAAAgRADjyOkQ7TQViTMI/ke8QOXcmD
7uzXQmR9DxvXIBLzMAAAANaGFybWlvbi1vd25lcgAAAAAAAAAGc2hhNTEyAAAAUwAAAAtz
c2gtZWQyNTUxOQAAAEDrj0nF4YKCa7ghJ3ZR+Wco3YHZEgp5qxIYNeyLiX7eYMfTyOyUWU
vhX5WBK8AmgmiF3QFNYPz8uySoGHqyWb8P
-----END SSH SIGNATURE-----
";
        const STATEMENT: &[u8] = b"HOA1\0cluster=0000000000000000000000000000000000000000000000000000000000000001\nnode=0000000000000000000000000000000000000000000000000000000000000002\noperation=approve\ntarget=-\nargs_sha256=3333333333333333333333333333333333333333333333333333333333333333\nsession=-\nchallenge=4444444444444444444444444444444444444444444444444444444444444444\nissued_at=1\nexpires_at=2\n";
        let matched = verify(
            &["ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIEQA48jpEO00FYkzCP5HvEDl3Jg+7s10JkfQ8b1yAS8z fixture".to_owned()],
            STATEMENT,
            SIGNATURE.as_bytes(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            matched,
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIEQA48jpEO00FYkzCP5HvEDl3Jg+7s10JkfQ8b1yAS8z"
        );
        let key = parse_key_line(PUBLIC).unwrap();
        assert_eq!(
            key.fingerprint,
            "SHA256:v/QrEo2RZ8N3r6YBw63Td271gOryVxomf+RIm/n0yiM"
        );
        // Any change to the statement invalidates the signature.
        let mut tampered = STATEMENT.to_vec();
        tampered[0] ^= 1;
        assert!(verify(&[PUBLIC.to_owned()], &tampered, SIGNATURE.as_bytes()).is_err());
        // The authorized key must be the exact registered one.
        let other = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIEQA48jpEO00FYkzCP5HvEDl3Jg+7s10JkfQ8b1yAS80 fixture";
        assert!(verify(&[other.to_owned()], STATEMENT, SIGNATURE.as_bytes()).is_err());
    }

    #[test]
    fn malformed_inputs_are_rejected_exactly() {
        assert_eq!(
            parse_key_line("ssh-ed25519 !!!").unwrap_err(),
            KeyError::Invalid
        );
        assert_eq!(
            parse_key_line("nonsense AAAA").unwrap_err(),
            KeyError::Invalid
        );
        assert_eq!(parse_key_line("").unwrap_err(), KeyError::Invalid);
        let armored = b"-----BEGIN SSH SIGNATURE-----\nAAAA\n-----END SSH SIGNATURE-----\n";
        assert_eq!(
            verify(&[], b"statement", armored).unwrap_err(),
            VerifyError::Invalid
        );
        assert_eq!(
            verify(&[], b"statement", b"not a signature").unwrap_err(),
            VerifyError::Invalid
        );
        assert_eq!(mpint_fixed(&[0x80; 32]).unwrap_err(), VerifyError::Invalid);
        assert_eq!(mpint_fixed(&[0xff; 33]).unwrap_err(), VerifyError::Invalid);
        assert_eq!(mpint_fixed(&[0; 1]).unwrap(), [0u8; 32]);
    }
}
