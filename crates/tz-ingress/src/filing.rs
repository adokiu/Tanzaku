//! 中国大陆节点：未过白域名的 HTTP(S) 返回备案提示页。

use std::{io, sync::Arc};

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::sign::CertifiedKey;
use tokio_rustls::TlsAcceptor;

const FILING_HTML: &str = include_str!("../preview/filing.html");

#[derive(Debug, Clone, Default)]
pub struct FilingGate {
    pub enabled: bool,
    pub whitelist: Vec<String>,
}

impl FilingGate {
    /// 域名未过白时拦截。IP Host 不拦截。
    pub fn blocks(&self, host: &str) -> bool {
        if !self.enabled {
            return false;
        }
        let name = host_name(host);
        if name.is_empty() || name.parse::<std::net::IpAddr>().is_ok() {
            return false;
        }
        !self.allows_domain(name)
    }

    pub fn allows_domain(&self, name: &str) -> bool {
        let name = name.trim().trim_end_matches('.').to_ascii_lowercase();
        self.whitelist.iter().any(|entry| {
            let entry = entry
                .trim()
                .trim_end_matches('.')
                .trim_start_matches("*.")
                .to_ascii_lowercase();
            !entry.is_empty() && (name == entry || name.ends_with(&format!(".{entry}")))
        })
    }
}

fn host_name(host: &str) -> &str {
    let host = host.trim();
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(host);
    }
    host.split(':').next().unwrap_or(host)
}

pub fn filing_http_bytes() -> Vec<u8> {
    let body = FILING_HTML.as_bytes();
    format!(
        "HTTP/1.1 403 Forbidden\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\
         Content-Length: {}\r\n\
         \r\n",
        body.len()
    )
    .into_bytes()
    .into_iter()
    .chain(body.iter().copied())
    .collect()
}

pub fn filing_html() -> &'static str {
    FILING_HTML
}

struct BlockTls {
    acceptor: TlsAcceptor,
    certified: Arc<CertifiedKey>,
}

fn block_tls() -> io::Result<&'static BlockTls> {
    use std::sync::OnceLock;
    static TLS: OnceLock<Result<BlockTls, String>> = OnceLock::new();
    match TLS.get_or_init(|| build_block_tls().map_err(|err| err.to_string())) {
        Ok(tls) => Ok(tls),
        Err(message) => Err(io::Error::new(io::ErrorKind::Other, message.clone())),
    }
}

pub fn block_acceptor() -> io::Result<TlsAcceptor> {
    Ok(block_tls()?.acceptor.clone())
}

pub fn block_certified_key() -> io::Result<Arc<CertifiedKey>> {
    Ok(block_tls()?.certified.clone())
}

fn build_block_tls() -> io::Result<BlockTls> {
    let key_pair = rcgen::KeyPair::generate()
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err.to_string()))?;
    let mut params = rcgen::CertificateParams::new(vec!["tanzaku.invalid".into()])
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err.to_string()))?;
    let cert = params
        .self_signed(&key_pair)
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err.to_string()))?;
    let cert_der = CertificateDer::from(cert.der().to_vec());
    let key_der_bytes = key_pair.serialize_der();
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let signing = provider
        .key_provider
        .load_private_key(PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
            key_der_bytes.clone(),
        )))
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err.to_string()))?;
    let certified = Arc::new(CertifiedKey::new(vec![cert_der.clone()], signing));
    let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der_bytes));
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err.to_string()))?
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], key_der)
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err.to_string()))?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(BlockTls {
        acceptor: TlsAcceptor::from(Arc::new(config)),
        certified,
    })
}
