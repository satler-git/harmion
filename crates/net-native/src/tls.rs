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

pub use harmion_local_web::BrowserTls;

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
