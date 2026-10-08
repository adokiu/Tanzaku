use crate::rustls_init::{aws_crypto_provider, init_rustls_crypto_provider};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls::{ClientConfig, RootCertStore, ServerConfig, server::WebPkiClientVerifier};
use std::sync::Arc;
use thiserror::Error;

const TLS13: &[&rustls::SupportedProtocolVersion] = &[&rustls::version::TLS13];

pub const ALPN: &[u8] = b"tz/1";

#[derive(Clone)]
pub struct IdentityMaterial {
    pub certificate_chain_pem: Vec<String>,
    pub private_key_pem: String,
    /// Expiry hint from board `CertIssued.not_after` (renewal scheduling).
    pub not_after: Option<String>,
}

#[derive(Debug, Error)]
pub enum TlsError {
    #[error("TLS configuration is invalid: {0}")]
    Invalid(String),
}

pub fn build_server_config_app_layer_auth(
    identity: &IdentityMaterial,
) -> Result<Arc<ServerConfig>, TlsError> {
    init_rustls_crypto_provider();
    let certs = load_certs(&identity.certificate_chain_pem)?;
    let key = load_key(&identity.private_key_pem)?;
    let mut config = ServerConfig::builder_with_provider(aws_crypto_provider())
        .with_protocol_versions(TLS13)
        .map_err(|err| TlsError::Invalid(err.to_string()))?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|err| TlsError::Invalid(err.to_string()))?;
    config.alpn_protocols = vec![ALPN.to_vec()];
    config.max_early_data_size = 0;
    Ok(Arc::new(config))
}

pub fn build_server_config(
    identity: &IdentityMaterial,
    client_ca_pem: &str,
) -> Result<Arc<ServerConfig>, TlsError> {
    init_rustls_crypto_provider();
    let certs = load_certs(&identity.certificate_chain_pem)?;
    let key = load_key(&identity.private_key_pem)?;
    let mut roots = RootCertStore::empty();
    for cert in load_certs(&[client_ca_pem.to_string()])? {
        roots
            .add(cert)
            .map_err(|err| TlsError::Invalid(err.to_string()))?;
    }
    let verifier = WebPkiClientVerifier::builder(roots.into())
        .build()
        .map_err(|err| TlsError::Invalid(err.to_string()))?;
    let mut config = ServerConfig::builder_with_provider(aws_crypto_provider())
        .with_protocol_versions(TLS13)
        .map_err(|err| TlsError::Invalid(err.to_string()))?
        .with_client_cert_verifier(verifier)
        .with_single_cert(certs, key)
        .map_err(|err| TlsError::Invalid(err.to_string()))?;
    config.alpn_protocols = vec![ALPN.to_vec()];
    config.max_early_data_size = 0;
    Ok(Arc::new(config))
}

pub fn build_client_config(
    identity: &IdentityMaterial,
    server_ca_pem: &str,
    expected_server_name: &str,
) -> Result<(Arc<ClientConfig>, ServerName<'static>), TlsError> {
    init_rustls_crypto_provider();
    let certs = load_certs(&identity.certificate_chain_pem)?;
    let key = load_key(&identity.private_key_pem)?;
    let mut roots = RootCertStore::empty();
    for cert in load_certs(&[server_ca_pem.to_string()])? {
        roots
            .add(cert)
            .map_err(|err| TlsError::Invalid(err.to_string()))?;
    }
    let mut config = ClientConfig::builder_with_provider(aws_crypto_provider())
        .with_protocol_versions(TLS13)
        .map_err(|err| TlsError::Invalid(err.to_string()))?
        .with_root_certificates(roots)
        .with_client_auth_cert(certs, key)
        .map_err(|err| TlsError::Invalid(err.to_string()))?;
    config.alpn_protocols = vec![ALPN.to_vec()];
    config.enable_early_data = false;
    let server_name = ServerName::try_from(expected_server_name.to_string())
        .map_err(|_| TlsError::Invalid("server name is invalid".into()))?;
    Ok((Arc::new(config), server_name))
}

fn load_certs(pem_blocks: &[String]) -> Result<Vec<CertificateDer<'static>>, TlsError> {
    let mut certs = Vec::new();
    for block in pem_blocks {
        let mut reader = std::io::Cursor::new(block.as_bytes());
        for item in rustls_pemfile::certs(&mut reader) {
            let der = item.map_err(|err| TlsError::Invalid(err.to_string()))?;
            certs.push(CertificateDer::from(der));
        }
    }
    if certs.is_empty() {
        return Err(TlsError::Invalid("certificate chain is empty".into()));
    }
    Ok(certs)
}

fn load_key(pem: &str) -> Result<PrivateKeyDer<'static>, TlsError> {
    let mut reader = std::io::Cursor::new(pem.as_bytes());
    rustls_pemfile::private_key(&mut reader)
        .map_err(|err| TlsError::Invalid(err.to_string()))?
        .ok_or_else(|| TlsError::Invalid("private key is missing".into()))
}
