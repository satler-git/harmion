use crate::{Error, transport};
use harmion_node::membership::{MAX_CERTIFICATE_BYTES, Peer};
use rustls::{
    ClientConfig, RootCertStore, ServerConfig,
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
};
use std::sync::Arc;

/// Routing address is separate from the authenticated, pinned node certificate.
pub const SERVER_NAME: &str = "harmion.local";

pub struct TlsIdentity {
    certificate: Vec<u8>,
    private_key: Vec<u8>,
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
            .push(DnType::CommonName, format!("Harmion OAuth {label}"));
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
            .push(DnType::CommonName, "Harmion OAuth loopback");
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
        validate_certificate(&self.root_der)?;
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
impl TlsIdentity {
    pub fn generate() -> Result<Self, Error> {
        let key = rcgen::KeyPair::generate().map_err(transport)?;
        let mut params =
            rcgen::CertificateParams::new(vec![SERVER_NAME.to_owned()]).map_err(transport)?;
        params.extended_key_usages = vec![
            rcgen::ExtendedKeyUsagePurpose::ServerAuth,
            rcgen::ExtendedKeyUsagePurpose::ClientAuth,
        ];
        params.key_usages = vec![rcgen::KeyUsagePurpose::DigitalSignature];
        let certificate = params.self_signed(&key).map_err(transport)?.der().to_vec();
        Self::from_der(certificate, key.serialize_der())
    }
    pub fn from_der(certificate: Vec<u8>, private_key: Vec<u8>) -> Result<Self, Error> {
        validate_certificate(&certificate)?;
        let identity = Self {
            certificate,
            private_key,
        };
        identity.app_server_config()?; // rustls also checks the certificate/key pair.
        Ok(identity)
    }
    pub fn certificate(&self) -> &[u8] {
        &self.certificate
    }
    /// Only for the host's owner-protected secret file, never a response or log.
    pub fn private_key_der(&self) -> &[u8] {
        &self.private_key
    }
    fn key(&self) -> PrivateKeyDer<'static> {
        PrivatePkcs8KeyDer::from(self.private_key.clone()).into()
    }
    fn chain(&self) -> Vec<CertificateDer<'static>> {
        vec![CertificateDer::from(self.certificate.clone())]
    }

    pub fn app_server_config(&self) -> Result<ServerConfig, Error> {
        ServerConfig::builder_with_provider(rustls::crypto::ring::default_provider().into())
            .with_safe_default_protocol_versions()
            .map_err(transport)?
            .with_no_client_auth()
            .with_single_cert(self.chain(), self.key())
            .map_err(transport)
    }
    pub fn peer_server_config(&self, peers: &[Peer]) -> Result<ServerConfig, Error> {
        let mut roots = RootCertStore::empty();
        for peer in peers
            .iter()
            .filter(|p| matches!(p.key, harmion_node::membership::PeerKey::TlsCertificate(_)))
        {
            roots
                .add(CertificateDer::from(peer.key.tls_certificate()?.to_vec()))
                .map_err(transport)?;
        }
        let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
            Arc::new(roots),
            rustls::crypto::ring::default_provider().into(),
        )
        .build()
        .map_err(transport)?;
        ServerConfig::builder_with_provider(rustls::crypto::ring::default_provider().into())
            .with_safe_default_protocol_versions()
            .map_err(transport)?
            .with_client_cert_verifier(verifier)
            .with_single_cert(self.chain(), self.key())
            .map_err(transport)
    }
    pub fn peer_client_config(&self, peer: &Peer) -> Result<ClientConfig, Error> {
        client_builder(peer.key.tls_certificate()?)?
            .with_client_auth_cert(self.chain(), self.key())
            .map_err(transport)
    }
}

pub fn validate_certificate(certificate: &[u8]) -> Result<(), Error> {
    if certificate.is_empty() || certificate.len() > MAX_CERTIFICATE_BYTES {
        return Err(Error::InvalidResponse);
    }
    RootCertStore::empty()
        .add(CertificateDer::from(certificate.to_vec()))
        .map_err(transport)
}
fn client_builder(
    certificate: &[u8],
) -> Result<rustls::ConfigBuilder<ClientConfig, rustls::client::WantsClientCert>, Error> {
    validate_certificate(certificate)?;
    let mut roots = RootCertStore::empty();
    roots
        .add(CertificateDer::from(certificate.to_vec()))
        .map_err(transport)?;
    Ok(
        ClientConfig::builder_with_provider(rustls::crypto::ring::default_provider().into())
            .with_safe_default_protocol_versions()
            .map_err(transport)?
            .with_root_certificates(roots),
    )
}
pub fn app_client_config(certificate: &[u8]) -> Result<ClientConfig, Error> {
    Ok(client_builder(certificate)?.with_no_client_auth())
}
