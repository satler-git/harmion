//! Loopback HTTPS identity shared by native browser-facing applications.
use rustls::{
    RootCertStore, ServerConfig,
    pki_types::{CertificateDer, PrivatePkcs8KeyDer},
};
use std::io::{self, Error};

fn transport(error: impl std::error::Error + Send + Sync + 'static) -> Error {
    io::Error::other(error)
}

/// Separate browser trust root and loopback-only server identity.
/// The issuing key is discarded after provisioning.
pub struct BrowserTls {
    pub root_der: Vec<u8>,
    pub leaf_der: Vec<u8>,
    pub private_key_der: Vec<u8>,
}

impl BrowserTls {
    pub fn generate(label: &str) -> Result<Self, Error> {
        use rcgen::{
            BasicConstraints, CidrSubnet, DnType, GeneralSubtree, IsCa, KeyUsagePurpose,
            NameConstraints,
        };
        let root_key = rcgen::KeyPair::generate().map_err(transport)?;
        let mut root = rcgen::CertificateParams::default();
        root.distinguished_name
            .push(DnType::CommonName, format!("Harmion {label}"));
        root.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        root.key_usages = vec![KeyUsagePurpose::KeyCertSign];
        root.name_constraints = Some(NameConstraints {
            permitted_subtrees: vec![GeneralSubtree::IpAddress(CidrSubnet::V4(
                [127, 0, 0, 1],
                [255; 4],
            ))],
            excluded_subtrees: vec![GeneralSubtree::DnsName(String::new())],
        });
        let now = time::OffsetDateTime::now_utc();
        root.not_before = now - time::Duration::minutes(5);
        root.not_after = now + time::Duration::days(365);
        let root_der = root
            .self_signed(&root_key)
            .map_err(transport)?
            .der()
            .to_vec();
        let issuer = rcgen::Issuer::new(root, root_key);
        let key = rcgen::KeyPair::generate().map_err(transport)?;
        let mut leaf =
            rcgen::CertificateParams::new(vec!["127.0.0.1".into()]).map_err(transport)?;
        leaf.distinguished_name
            .push(DnType::CommonName, format!("Harmion {label} loopback"));
        leaf.not_before = now - time::Duration::minutes(5);
        leaf.not_after = now + time::Duration::days(365);
        leaf.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        leaf.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];
        leaf.use_authority_key_identifier_extension = true;
        let leaf_der = leaf
            .signed_by(&key, &issuer)
            .map_err(transport)?
            .der()
            .to_vec();
        Ok(Self {
            root_der,
            leaf_der,
            private_key_der: key.serialize_der(),
        })
    }

    pub fn server_config(&self) -> Result<ServerConfig, Error> {
        RootCertStore::empty()
            .add(CertificateDer::from(self.root_der.clone()))
            .map_err(transport)?;
        let leaf = CertificateDer::from(self.leaf_der.clone());
        let key = PrivatePkcs8KeyDer::from(self.private_key_der.clone());
        ServerConfig::builder_with_provider(rustls::crypto::ring::default_provider().into())
            .with_safe_default_protocol_versions()
            .map_err(transport)?
            .with_no_client_auth()
            .with_single_cert(vec![leaf], key.into())
            .map_err(transport)
    }

    pub fn root_pem(&self) -> String {
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        let encoded = STANDARD.encode(&self.root_der);
        let mut pem = String::from("-----BEGIN CERTIFICATE-----\n");
        for row in encoded.as_bytes().chunks(64) {
            pem.push_str(std::str::from_utf8(row).expect("base64 is ASCII"));
            pem.push('\n');
        }
        pem.push_str("-----END CERTIFICATE-----\n");
        pem
    }
}
