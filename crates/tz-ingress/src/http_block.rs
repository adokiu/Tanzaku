//! TCP 隧道上拦截浏览器 HTTP(S)：明文回 HTML；TLS 用自签证书握手后再回同一页面。

use std::{io, sync::Arc, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::TlsAcceptor;
use tz_guard::pipeline::GuardPipeline;

use crate::shared_https::SniCertResolver;
use rustls::server::ResolvesServerCert;

const HTTP_BLOCK_BODY: &str = include_str!("../preview/l4-http-block.html");
/// 首包可能分段到达；总等待窗口内反复 peek。
const FIRST_PACKET_BUDGET: Duration = Duration::from_millis(2_000);
const FIRST_PACKET_STEP: Duration = Duration::from_millis(80);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirstPacketKind {
    Http,
    Tls,
    Other,
}

pub enum BlockOutcome {
    /// 已返回提示页并关闭连接。
    Blocked,
    /// 非浏览器 HTTP(S)，把原 socket 交回继续隧道转发。
    Continue(tokio::net::TcpStream),
}

pub fn classify_first_packet(bytes: &[u8]) -> FirstPacketKind {
    if bytes.len() >= 3 && bytes[0] == 0x16 && bytes[1] == 0x03 {
        return FirstPacketKind::Tls;
    }
    const METHODS: &[&[u8]] = &[
        b"GET ", b"POST ", b"HEAD ", b"PUT ", b"DELETE ", b"OPTIONS ", b"PATCH ", b"CONNECT ",
        b"PRI ", b"TRACE ",
    ];
    if METHODS.iter().any(|method| bytes.starts_with(method)) {
        return FirstPacketKind::Http;
    }
    FirstPacketKind::Other
}

/// 字节太少或像未写完的 HTTP 方法 / TLS 记录头时，继续等待。
pub fn needs_more_first_packet_bytes(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return true;
    }
    if bytes[0] == 0x16 {
        return bytes.len() < 3;
    }
    const METHODS: &[&[u8]] = &[
        b"GET ", b"POST ", b"HEAD ", b"PUT ", b"DELETE ", b"OPTIONS ", b"PATCH ", b"CONNECT ",
        b"PRI ", b"TRACE ",
    ];
    METHODS
        .iter()
        .any(|method| bytes.len() < method.len() && method.starts_with(bytes))
}

pub fn http_block_response() -> Vec<u8> {
    format!(
        "HTTP/1.1 403 Forbidden\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\
         Content-Length: {}\r\n\
         \r\n\
         {}",
        HTTP_BLOCK_BODY.len(),
        HTTP_BLOCK_BODY
    )
    .into_bytes()
}

fn self_signed_certified_key(
    provider: &Arc<rustls::crypto::CryptoProvider>,
) -> io::Result<rustls::sign::CertifiedKey> {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

    let key_pair = rcgen::KeyPair::generate()
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err.to_string()))?;
    // 自签仅兜底按 IP / 未知 SNI 访问的拦截页；浏览器会提示不受信任，属预期（点「继续访问」后可见 HTML）。
    let mut params = rcgen::CertificateParams::new(vec![
        "localhost".into(),
        "tanzaku.invalid".into(),
        "*.tanzaku.invalid".into(),
    ])
    .map_err(|err| io::Error::new(io::ErrorKind::Other, err.to_string()))?;
    for ip in [
        std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
    ] {
        params
            .subject_alt_names
            .push(rcgen::SanType::IpAddress(ip));
    }
    let cert = params
        .self_signed(&key_pair)
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err.to_string()))?;
    let cert_der = CertificateDer::from(cert.der().to_vec());
    let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pair.serialize_der()));
    let signing = provider
        .key_provider
        .load_private_key(key_der)
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err.to_string()))?;
    Ok(rustls::sign::CertifiedKey::new(vec![cert_der], signing))
}

#[derive(Debug)]
struct FallbackOnly {
    key: Arc<rustls::sign::CertifiedKey>,
}

impl rustls::server::ResolvesServerCert for FallbackOnly {
    fn resolve(&self, _: rustls::server::ClientHello<'_>) -> Option<Arc<rustls::sign::CertifiedKey>> {
        Some(self.key.clone())
    }
}

/// 优先按 SNI 用节点真实证书（域名访问时拦截页无警告），解析不到再回退自签。
#[derive(Debug)]
struct SniWithFallback {
    sni: Arc<SniCertResolver>,
    fallback: Arc<rustls::sign::CertifiedKey>,
}

impl rustls::server::ResolvesServerCert for SniWithFallback {
    fn resolve(&self, hello: rustls::server::ClientHello<'_>) -> Option<Arc<rustls::sign::CertifiedKey>> {
        self.sni
            .resolve(hello)
            .or_else(|| Some(self.fallback.clone()))
    }
}

fn block_page_acceptor(sni: Option<Arc<SniCertResolver>>) -> io::Result<TlsAcceptor> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let fallback = Arc::new(self_signed_certified_key(&provider)?);
    let resolver: Arc<dyn rustls::server::ResolvesServerCert> = match sni {
        Some(sni) => Arc::new(SniWithFallback { sni, fallback }),
        None => Arc::new(FallbackOnly { key: fallback }),
    };
    // 不强制 ALPN：部分浏览器/客户端对 IP 访问不带 ALPN，强制会导致握手失败 → ERR_CONNECTION_RESET。
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err.to_string()))?
        .with_no_client_auth()
        .with_cert_resolver(resolver);
    Ok(TlsAcceptor::from(Arc::new(config)))
}

fn shared_block_acceptor(sni: Option<Arc<SniCertResolver>>) -> io::Result<TlsAcceptor> {
    use std::sync::OnceLock;
    static PLAIN: OnceLock<Result<TlsAcceptor, String>> = OnceLock::new();
    static SNI: OnceLock<Result<TlsAcceptor, String>> = OnceLock::new();
    match sni {
        None => match PLAIN.get_or_init(|| block_page_acceptor(None).map_err(|err| err.to_string())) {
            Ok(acceptor) => Ok(acceptor.clone()),
            Err(message) => Err(io::Error::new(io::ErrorKind::Other, message.clone())),
        },
        Some(resolver) => {
            match SNI.get_or_init(|| {
                block_page_acceptor(Some(resolver)).map_err(|err| err.to_string())
            }) {
                Ok(acceptor) => Ok(acceptor.clone()),
                Err(message) => Err(io::Error::new(io::ErrorKind::Other, message.clone())),
            }
        }
    }
}

/// 独立端口 HTTP 隧道终结公网 TLS 用：按 SNI 选节点真实证书，未命中（如按 IP 访问）回退自签。
pub fn dedicated_tls_acceptor(sni: Option<Arc<SniCertResolver>>) -> io::Result<TlsAcceptor> {
    shared_block_acceptor(sni)
}

/// 窥探首包判断明文 HTTP / TLS；客户端在预算内不发数据或已 EOF 时返回 `Other`。
pub async fn peek_first_packet_kind(stream: &tokio::net::TcpStream) -> io::Result<FirstPacketKind> {
    let deadline = tokio::time::Instant::now() + FIRST_PACKET_BUDGET;
    let mut buf = [0_u8; 16];
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Ok(FirstPacketKind::Other);
        }
        match tokio::time::timeout(remaining.min(FIRST_PACKET_STEP), stream.peek(&mut buf)).await {
            Ok(Ok(0)) => return Ok(FirstPacketKind::Other),
            Ok(Ok(n)) => {
                if needs_more_first_packet_bytes(&buf[..n]) {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    continue;
                }
                return Ok(classify_first_packet(&buf[..n]));
            }
            Ok(Err(err)) => return Err(err),
            Err(_) => continue,
        }
    }
}

async fn write_block_response_and_close<S>(stream: &mut S) -> io::Result<()>
where
    S: AsyncReadExt + AsyncWriteExt + Unpin,
{
    let response = http_block_response();
    stream.write_all(&response).await?;
    stream.flush().await?;
    let _ = stream.shutdown().await;
    // 排空对端可能仍在发送的请求，降低「响应未读完就被 RST」的概率。
    let mut discard = [0_u8; 2_048];
    let _ = tokio::time::timeout(Duration::from_millis(250), async {
        loop {
            match stream.read(&mut discard).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
    })
    .await;
    Ok(())
}

/// 窥探首包；HTTP/TLS 返回提示页，其它流量把 socket 交回。
/// `cert_resolver` 提供时优先按 SNI 用节点真实证书，域名访问无浏览器警告。
pub async fn maybe_block_browser_http(
    mut stream: tokio::net::TcpStream,
    guard: &GuardPipeline,
    cert_resolver: Option<Arc<SniCertResolver>>,
) -> io::Result<BlockOutcome> {
    let deadline = tokio::time::Instant::now() + FIRST_PACKET_BUDGET;
    let mut buf = [0_u8; 16];
    let n = loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Ok(BlockOutcome::Continue(stream));
        }
        let wait = remaining.min(FIRST_PACKET_STEP);
        let peeked = match tokio::time::timeout(wait, stream.peek(&mut buf)).await {
            Ok(Ok(n)) => n,
            Ok(Err(err)) => return Err(err),
            // 单步超时后不在无时限下再次 peek，避免卡死；回到总预算循环。
            Err(_) => {
                continue;
            }
        };
        if peeked == 0 {
            continue;
        }
        if needs_more_first_packet_bytes(&buf[..peeked]) {
            tokio::time::sleep(Duration::from_millis(20)).await;
            continue;
        }
        break peeked;
    };
    if n < 3 && classify_first_packet(&buf[..n]) == FirstPacketKind::Other {
        return Ok(BlockOutcome::Continue(stream));
    }
    match classify_first_packet(&buf[..n]) {
        FirstPacketKind::Http => {
            write_block_response_and_close(&mut stream).await?;
            Ok(BlockOutcome::Blocked)
        }
        FirstPacketKind::Tls => {
            let _permit = match guard.acquire_tls_handshake() {
                Ok(permit) => permit,
                Err(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::ConnectionRefused,
                        "tls handshake limit reached while serving block page",
                    ));
                }
            };
            let acceptor = shared_block_acceptor(cert_resolver)?;
            let mut tls =
                match tokio::time::timeout(Duration::from_secs(8), acceptor.accept(stream)).await {
                    Ok(Ok(tls)) => tls,
                    Ok(Err(err)) => {
                        // 握手失败时返回错误，由调用方关闭；避免半开连接导致浏览器 RST。
                        return Err(io::Error::new(
                            io::ErrorKind::ConnectionReset,
                            format!("tls handshake for block page failed: {err}"),
                        ));
                    }
                    Err(_) => {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "tls handshake for block page timed out",
                        ));
                    }
                };
            write_block_response_and_close(&mut tls).await?;
            Ok(BlockOutcome::Blocked)
        }
        FirstPacketKind::Other => Ok(BlockOutcome::Continue(stream)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incomplete_method_waits_for_more_bytes() {
        assert!(needs_more_first_packet_bytes(b""));
        assert!(needs_more_first_packet_bytes(b"G"));
        assert!(needs_more_first_packet_bytes(b"GE"));
        assert!(needs_more_first_packet_bytes(b"GET"));
        assert!(!needs_more_first_packet_bytes(b"GET "));
        assert!(needs_more_first_packet_bytes(&[0x16]));
        assert!(needs_more_first_packet_bytes(&[0x16, 0x03]));
        assert!(!needs_more_first_packet_bytes(&[0x16, 0x03, 0x01]));
        assert!(!needs_more_first_packet_bytes(b"SSH-2.0"));
    }

    #[test]
    fn classify_recognizes_http_and_tls() {
        assert_eq!(classify_first_packet(b"GET / HTTP/1.1\r\n"), FirstPacketKind::Http);
        assert_eq!(
            classify_first_packet(&[0x16, 0x03, 0x01, 0x00]),
            FirstPacketKind::Tls
        );
        assert_eq!(classify_first_packet(b"SSH-2.0"), FirstPacketKind::Other);
    }
}
