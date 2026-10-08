use anyhow::Context;
use rcgen::{CertificateParams, CertificateSigningRequestParams, KeyPair};
use sha2::{Digest, Sha256};

pub struct IssuedCertificate {
    pub certificate_pem: String,
    pub fingerprint: String,
    pub not_after: String,
}

pub fn sign_csr_pem_with_ca(
    ca_cert_pem: &str,
    ca_key_pem: &str,
    csr_pem: &str,
) -> anyhow::Result<IssuedCertificate> {
    let ca_key = KeyPair::from_pem(ca_key_pem).context("internal CA key is invalid")?;
    let ca_params =
        CertificateParams::from_ca_cert_pem(ca_cert_pem).context("internal CA certificate is invalid")?;
    let ca_cert = ca_params
        .self_signed(&ca_key)
        .context("internal CA signing material is invalid")?;
    sign_csr_pem(&ca_cert, &ca_key, csr_pem)
}

pub fn sign_csr_pem(
    ca_cert: &rcgen::Certificate,
    ca_key: &KeyPair,
    csr_pem: &str,
) -> anyhow::Result<IssuedCertificate> {
    let mut csr_params = CertificateSigningRequestParams::from_pem(csr_pem)
        .context("CSR PEM is invalid or uses unsupported extensions")?;
    let now = time::OffsetDateTime::now_utc();
    csr_params.params.not_before = now - time::Duration::hours(1);
    csr_params.params.not_after = now + time::Duration::days(30);
    let signed = csr_params
        .signed_by(ca_cert, ca_key)
        .context("failed to sign CSR")?;
    let certificate_pem = signed.pem();
    Ok(IssuedCertificate {
        fingerprint: cert_fingerprint(&certificate_pem),
        not_after: parse_not_after(&certificate_pem).unwrap_or_else(|| "unknown".into()),
        certificate_pem,
    })
}

pub fn cert_fingerprint(pem: &str) -> String {
    let mut reader = std::io::Cursor::new(pem.as_bytes());
    let Some(der) = rustls_pemfile::certs(&mut reader)
        .filter_map(|item| item.ok())
        .next()
    else {
        return fingerprint_der(pem.as_bytes());
    };
    fingerprint_der(&der)
}

pub fn fingerprint_der(der: &[u8]) -> String {
    let digest = Sha256::digest(der);
    digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn parse_not_after(pem: &str) -> Option<String> {
    use x509_parser::pem::parse_x509_pem;
    use x509_parser::prelude::FromDer;
    let (_, pem) = parse_x509_pem(pem.as_bytes()).ok()?;
    let (_, cert) = x509_parser::certificate::X509Certificate::from_der(&pem.contents).ok()?;
    Some(cert.validity().not_after.to_string())
}
