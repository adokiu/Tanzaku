use rustls::pki_types::CertificateDer;
use uuid::Uuid;

const CLIENT_SAN_PREFIX: &str = "client-";
const NODE_SAN_PREFIX: &str = "node-";

pub fn client_id_from_certificate_der(der: &[u8]) -> Option<Uuid> {
    let san = san_from_certificate_der(der)?;
    san.strip_prefix(CLIENT_SAN_PREFIX)
        .and_then(|id| Uuid::parse_str(id).ok())
}

pub fn node_id_from_certificate_der(der: &[u8]) -> Option<Uuid> {
    let san = san_from_certificate_der(der)?;
    san.strip_prefix(NODE_SAN_PREFIX)
        .and_then(|id| Uuid::parse_str(id).ok())
}

pub fn client_id_from_peer_certificates(certs: &[CertificateDer<'_>]) -> Option<Uuid> {
    certs
        .first()
        .map(|cert| cert.as_ref())
        .and_then(client_id_from_certificate_der)
}

fn san_from_certificate_der(der: &[u8]) -> Option<String> {
    use x509_parser::certificate::X509Certificate;
    use x509_parser::prelude::FromDer;
    let (_, cert) = X509Certificate::from_der(der).ok()?;
    let san = cert.subject_alternative_name().ok()??;
    for name in san.value.general_names.iter() {
        if let x509_parser::extensions::GeneralName::DNSName(dns) = name {
            return Some((*dns).to_string());
        }
    }
    None
}
