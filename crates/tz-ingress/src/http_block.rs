//! TCP 隧道上拦截浏览器 HTTP(S)：明文回 HTML；TLS 用自签证书握手后再回同一页面。

use std::{io, sync::Arc, time::Duration};
use tokio::io::AsyncWriteExt;
use tokio_rustls::TlsAcceptor;

const HTTP_BLOCK_BODY: &str = include_str!("../preview/l4-http-block.html");

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

fn self_signed_acceptor() -> io::Result<TlsAcceptor> {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

    let key_pair = rcgen::KeyPair::generate()
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err.to_string()))?;
    // 自签仅用于展示拦截页；浏览器会提示不受信任，属预期（点「继续访问」后可见 HTML）。
    let mut params = rcgen::CertificateParams::new(vec![
        "localhost".into(),
        "tanzaku.invalid".into(),
    ])
    .map_err(|err| io::Error::new(io::ErrorKind::Other, err.to_string()))?;
    params.subject_alt_names.push(rcgen::SanType::IpAddress(
        std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
    ));
    params.subject_alt_names.push(rcgen::SanType::IpAddress(
        std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
    ));
    let cert = params
        .self_signed(&key_pair)
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err.to_string()))?;
    let cert_der = CertificateDer::from(cert.der().to_vec());
    let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pair.serialize_der()));

    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err.to_string()))?
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], key_der)
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err.to_string()))?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(TlsAcceptor::from(Arc::new(config)))
}

fn shared_acceptor() -> io::Result<&'static TlsAcceptor> {
    use std::sync::OnceLock;
    static ACCEPTOR: OnceLock<Result<TlsAcceptor, String>> = OnceLock::new();
    match ACCEPTOR.get_or_init(|| self_signed_acceptor().map_err(|err| err.to_string())) {
        Ok(acceptor) => Ok(acceptor),
        Err(message) => Err(io::Error::new(io::ErrorKind::Other, message.clone())),
    }
}

/// 窥探首包；HTTP/TLS 返回提示页，其它流量把 socket 交回。
pub async fn maybe_block_browser_http(
    mut stream: tokio::net::TcpStream,
) -> io::Result<BlockOutcome> {
    let mut buf = [0_u8; 16];
    let n = match tokio::time::timeout(Duration::from_millis(800), stream.peek(&mut buf)).await {
        Ok(Ok(n)) => n,
        Ok(Err(err)) => return Err(err),
        Err(_) => return Ok(BlockOutcome::Continue(stream)),
    };
    if n < 3 {
        return Ok(BlockOutcome::Continue(stream));
    }
    match classify_first_packet(&buf[..n]) {
        FirstPacketKind::Http => {
            stream.write_all(&http_block_response()).await?;
            let _ = stream.shutdown().await;
            Ok(BlockOutcome::Blocked)
        }
        FirstPacketKind::Tls => {
            let acceptor = shared_acceptor()?.clone();
            let mut tls =
                match tokio::time::timeout(Duration::from_secs(8), acceptor.accept(stream)).await {
                    Ok(Ok(tls)) => tls,
                    Ok(Err(err)) => return Err(err),
                    Err(_) => {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "tls handshake for block page timed out",
                        ));
                    }
                };
            tls.write_all(&http_block_response()).await?;
            let _ = tls.shutdown().await;
            Ok(BlockOutcome::Blocked)
        }
        FirstPacketKind::Other => Ok(BlockOutcome::Continue(stream)),
    }
}
