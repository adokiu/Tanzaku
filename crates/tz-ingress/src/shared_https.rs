use arc_swap::ArcSwap;
use rustls::pki_types::CertificateDer;
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use std::{
    collections::HashMap,
    io,
    sync::{Arc, RwLock},
};
use tokio_rustls::TlsAcceptor;
use tz_guard::pipeline::GuardPipeline;
use tz_net::{budget::Budget, pool::BufferPool, rate::RateClock};

use crate::shutdown::IngressShutdown;
use tz_proto::NodeTlsCertificate;

#[derive(Clone)]
pub struct SharedHttpsListener {
    pub bind_addr: String,
    pub port: u16,
    pub routes: Arc<ArcSwap<HashMap<String, crate::shared_http::SharedHttpRoute>>>,
    pub resolver: Arc<SniCertResolver>,
    pub guard: Arc<GuardPipeline>,
    pub rate_clock: RateClock,
    pub shutdown: IngressShutdown,
    pub filing: Arc<ArcSwap<crate::filing::FilingGate>>,
}

#[derive(Debug)]
pub struct SniCertResolver {
    keys: RwLock<HashMap<String, Arc<CertifiedKey>>>,
    filing: Arc<ArcSwap<crate::filing::FilingGate>>,
}

impl SniCertResolver {
    pub fn new(filing: Arc<ArcSwap<crate::filing::FilingGate>>) -> Self {
        Self {
            keys: RwLock::new(HashMap::new()),
            filing,
        }
    }

    pub fn reload(&self, materials: &[NodeTlsCertificate]) {
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let mut map = HashMap::new();
        for material in materials {
            let Ok(certified) = build_certified_key(provider.clone(), material) else {
                tracing::warn!(domains = ?material.domains, "共享 HTTPS 证书装载失败");
                continue;
            };
            let certified = Arc::new(certified);
            for domain in &material.domains {
                map.insert(domain.to_ascii_lowercase(), certified.clone());
            }
        }
        if let Ok(mut guard) = self.keys.write() {
            *guard = map;
        }
    }
}

impl ResolvesServerCert for SniCertResolver {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let name = hello.server_name()?.to_ascii_lowercase();
        if self.filing.load().blocks(&name) {
            return crate::filing::block_certified_key().ok();
        }
        let keys = self.keys.read().ok()?;
        lookup_certified(&keys, &name)
    }
}

fn lookup_certified(
    keys: &HashMap<String, Arc<CertifiedKey>>,
    name: &str,
) -> Option<Arc<CertifiedKey>> {
    if let Some(certified) = keys.get(name) {
        return Some(certified.clone());
    }
    let rest = name.split_once('.')?.1;
    if rest.contains('.') {
        if let Some(certified) = keys.get(&format!("*.{rest}")) {
            return Some(certified.clone());
        }
    }
    None
}

fn build_certified_key(
    provider: Arc<rustls::crypto::CryptoProvider>,
    material: &NodeTlsCertificate,
) -> Result<CertifiedKey, rustls::Error> {
    let mut cert_reader = std::io::Cursor::new(material.certificate_pem.as_bytes());
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_reader)
        .filter_map(|item| item.ok())
        .map(CertificateDer::from)
        .collect();
    if certs.is_empty() {
        return Err(rustls::Error::General("empty certificate".into()));
    }
    let mut key_reader = std::io::Cursor::new(material.private_key_pem.as_bytes());
    let key = rustls_pemfile::private_key(&mut key_reader)
        .map_err(|_| rustls::Error::General("invalid private key".into()))?
        .ok_or(rustls::Error::General("missing private key".into()))?;
    let signing = provider
        .key_provider
        .load_private_key(key)
        .map_err(|_| rustls::Error::General("private key rejected".into()))?;
    Ok(CertifiedKey::new(certs, signing))
}

pub async fn serve(listener: SharedHttpsListener) -> io::Result<()> {
    let resolver = listener.resolver.clone();
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err.to_string()))?
        .with_no_client_auth()
        .with_cert_resolver(resolver);
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let socket = tokio::net::TcpListener::bind((listener.bind_addr.as_str(), listener.port)).await?;
    let budget = Budget::new(64 * 1024 * 1024);
    let pool = BufferPool::new(
        std::num::NonZeroUsize::new(32 * 1024).expect("chunk"),
        std::num::NonZeroUsize::new(256).expect("count"),
        &budget,
    )
    .map_err(|_| io::Error::from(io::ErrorKind::Other))?;
    loop {
        let (inbound, peer) = tokio::select! {
            _ = listener.shutdown.cancelled() => return Ok(()),
            accepted = socket.accept() => accepted?,
        };
        let acceptor = acceptor.clone();
        let ctx = Arc::new(crate::http_l7::SharedRequestContext {
            routes: listener.routes.clone(),
            guard: listener.guard.clone(),
            pool: pool.clone(),
            rate_clock: listener.rate_clock.clone(),
            filing: listener.filing.clone(),
        });
        tokio::spawn(async move {
            let tls = {
                let _permit = match ctx.guard.acquire_tls_handshake() {
                    Ok(permit) => permit,
                    Err(verdict) => {
                        ctx.guard.record_if_denied(&verdict, peer.ip(), None);
                        return;
                    }
                };
                match tokio::time::timeout(
                    std::time::Duration::from_secs(10),
                    acceptor.accept(inbound),
                )
                .await
                {
                    Ok(Ok(tls)) => tls,
                    Ok(Err(err)) => {
                        tracing::debug!(%peer, %err, "shared https handshake failed");
                        return;
                    }
                    Err(_) => {
                        tracing::debug!(%peer, "shared https handshake timed out");
                        return;
                    }
                }
            };
            let _ = crate::http_l7::serve_inbound_hyper(tls, peer, ctx, "https").await;
        });
    }
}
