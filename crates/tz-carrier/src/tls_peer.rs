use rustls::pki_types::CertificateDer;
use tz_pki::fingerprint_der;

pub fn fingerprint_peer_certificates(certs: &[CertificateDer<'_>]) -> String {
    certs
        .first()
        .map(|cert| fingerprint_der(cert.as_ref()))
        .unwrap_or_else(|| "unknown".into())
}

#[cfg(feature = "quic")]
pub fn quinn_peer_certificates(
    connection: &quinn::Connection,
) -> Option<Vec<CertificateDer<'static>>> {
    use std::any::Any;
    let identity = connection.peer_identity()?;
    if let Some(certs) = (identity.as_ref() as &dyn Any)
        .downcast_ref::<Vec<CertificateDer<'static>>>()
    {
        return Some(certs.clone());
    }
    identity
        .downcast::<Vec<CertificateDer<'static>>>()
        .ok()
        .map(|certs| *certs)
}

#[cfg(feature = "quic")]
pub fn quinn_connection_fingerprint(connection: &quinn::Connection) -> String {
    quinn_peer_certificates(connection)
        .map(|certs| fingerprint_peer_certificates(&certs))
        .unwrap_or_else(|| "unknown".into())
}
