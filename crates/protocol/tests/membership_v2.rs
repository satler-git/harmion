//! Independent HMC v1/v2 byte fixtures and the explicit SignedHttps binding.
//!
//! These tests do not verify Ed25519 signatures or membership authority.
use harmion_domain::{ClusterId, NodeId};
use harmion_protocol::{
    CodecError,
    membership::{Action, Binding, Certificate, Statement},
};

macro_rules! fixture {
    ($name:literal) => {
        (
            include_str!(concat!("../../../contracts/fixtures/", $name, ".hex")),
            include_str!(concat!(
                "../../../contracts/fixtures/",
                $name,
                "-signed.hex"
            )),
            include_str!(concat!("../../../contracts/fixtures/", $name, ".sha256")),
        )
    };
}

const NAMES: [&str; 5] = [
    "hmc-v1-https",
    "hmc-v1-iroh",
    "hmc-v2-signed-https",
    "hmc-v1-retire",
    "hmc-v1-dismiss",
];

fn bytes(text: &str) -> Vec<u8> {
    let text = text.trim();
    assert!(text.len().is_multiple_of(2));
    (0..text.len() / 2)
        .map(|i| u8::from_str_radix(&text[2 * i..2 * i + 2], 16).unwrap())
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn certificate(name: &str) -> Certificate {
    let (complete, _, _) = match name {
        "hmc-v1-https" => fixture!("hmc-v1-https"),
        "hmc-v1-iroh" => fixture!("hmc-v1-iroh"),
        "hmc-v2-signed-https" => fixture!("hmc-v2-signed-https"),
        "hmc-v1-retire" => fixture!("hmc-v1-retire"),
        "hmc-v1-dismiss" => fixture!("hmc-v1-dismiss"),
        _ => panic!("unknown fixture"),
    };
    Certificate::from_bytes(&bytes(complete)).unwrap()
}

#[test]
fn all_fixtures_match_independent_bytes_signing_inputs_and_ids() {
    for name in NAMES {
        let (hex_complete, hex_signed, hash) = match name {
            "hmc-v1-https" => fixture!("hmc-v1-https"),
            "hmc-v1-iroh" => fixture!("hmc-v1-iroh"),
            "hmc-v2-signed-https" => fixture!("hmc-v2-signed-https"),
            "hmc-v1-retire" => fixture!("hmc-v1-retire"),
            "hmc-v1-dismiss" => fixture!("hmc-v1-dismiss"),
            _ => unreachable!(),
        };
        let complete = bytes(hex_complete);
        let signed = bytes(hex_signed);
        let certificate = Certificate::from_bytes(&complete).unwrap();
        assert_eq!(certificate.bytes(), complete, "{name}");
        assert_eq!(certificate.signed_bytes(), signed, "{name}");
        assert_eq!(
            certificate.statement().signing_bytes().unwrap(),
            signed,
            "{name}"
        );
        assert_eq!(hex(&certificate.id()), hash.trim(), "{name}");
        for length in 0..complete.len() {
            assert!(
                Certificate::from_bytes(&complete[..length]).is_err(),
                "{name} accepted truncation {length}"
            );
        }
        let mut trailing = complete.clone();
        trailing.push(0);
        assert_eq!(
            Certificate::from_bytes(&trailing),
            Err(CodecError::TrailingBytes),
            "{name}"
        );
    }
}

#[test]
fn v1_bindings_keep_their_original_bytes_and_version() {
    let https = certificate("hmc-v1-https");
    assert_eq!(https.statement().version(), 1);
    let Action::Admit(member) = &https.statement().action else {
        panic!("admission fixture")
    };
    assert_eq!(
        member.binding,
        Binding::Https(b"DER-TLS-CERTIFICATE-BYTES".to_vec())
    );
    assert_eq!(member.endpoint, "127.0.0.1:8443");

    let iroh = certificate("hmc-v1-iroh");
    assert_eq!(iroh.statement().version(), 1);
    let Action::Admit(member) = &iroh.statement().action else {
        panic!("admission fixture")
    };
    assert_eq!(member.binding, Binding::Iroh([8; 32]));

    for name in ["hmc-v1-retire", "hmc-v1-dismiss"] {
        let certificate = certificate(name);
        assert_eq!(certificate.statement().version(), 1, "{name}");
        assert_eq!(certificate.statement().cluster, ClusterId([1; 32]));
        assert_eq!(certificate.statement().issuer, NodeId([2; 32]));
    }
    assert!(matches!(
        certificate("hmc-v1-retire").statement().action,
        Action::Retire { ref target, .. } if *target == NodeId([5; 32])
    ));
    assert!(matches!(
        certificate("hmc-v1-dismiss").statement().action,
        Action::Dismiss { .. }
    ));
}

#[test]
fn v2_signed_https_binding_is_explicit_and_repeats_the_signing_key() {
    let certificate = certificate("hmc-v2-signed-https");
    assert_eq!(certificate.statement().version(), 2);
    let Action::Admit(member) = &certificate.statement().action else {
        panic!("admission fixture")
    };
    assert_eq!(member.signing_key, [9; 32]);
    assert_eq!(member.binding, Binding::SignedHttps([9; 32]));
    assert_eq!(member.endpoint, "https://replica.example");

    // A typed binding naming a different key is not a second credential.
    let mut poison = certificate.statement().clone();
    let Action::Admit(member) = &mut poison.action else {
        panic!("admission fixture")
    };
    member.binding = Binding::SignedHttps([77; 32]);
    assert_eq!(
        poison.signing_bytes(),
        Err(CodecError::InvalidFieldValue),
        "binding key must equal the membership signing key"
    );
}

#[test]
fn v2_layout_rejects_mixed_or_unsupported_encodings() {
    let complete = bytes(fixture!("hmc-v2-signed-https").0);
    // magic(4) + version(4) + cluster/issuer/signing_key/nonce(4*32) + action tag
    assert_eq!(&complete[..4], b"HMC\0");
    let signed_len = complete.len() - 64;
    let version = u32::from_be_bytes(complete[4..8].try_into().unwrap());
    assert_eq!(version, 2);
    assert_eq!(complete[136], 0, "Admit action tag");
    assert_eq!(complete[201], 2, "SignedHttps binding tag");

    // A v2 frame carrying an older binding tag is not silently reinterpreted.
    let mut mixed = complete.clone();
    mixed[201] = 0;
    assert!(Certificate::from_bytes(&mixed).is_err());

    // The binding key is checked against the membership signing key on decode.
    let mut mismatched = complete.clone();
    mismatched[202] ^= 1;
    assert_eq!(
        Certificate::from_bytes(&mismatched),
        Err(CodecError::InvalidFieldValue)
    );

    // Unknown versions fail closed.
    let mut future = complete.clone();
    future[7] = 3;
    assert_eq!(
        Certificate::from_bytes(&future),
        Err(CodecError::UnsupportedVersion(3))
    );

    assert_eq!(signed_len, 202 + 32 + 4 + "https://replica.example".len());
}

#[test]
fn retire_and_dismiss_are_not_upgraded_to_v2() {
    let cluster = ClusterId([1; 32]);
    let statement = Statement {
        cluster,
        issuer: NodeId([2; 32]),
        signing_key: [2; 32],
        nonce: [0; 32],
        action: Action::Retire {
            target: NodeId([5; 32]),
            retained: Default::default(),
        },
    };
    assert_eq!(statement.version(), 1);
    let certificate = Certificate::new(statement.clone(), [13; 64]).unwrap();
    let decoded = Certificate::from_bytes(certificate.bytes()).unwrap();
    assert_eq!(decoded.statement().version(), 1);
    assert_eq!(
        decoded.statement().signing_bytes(),
        statement.signing_bytes()
    );
}
