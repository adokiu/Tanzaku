use std::{pin::Pin, sync::Arc};

#[derive(Debug)]
struct SkipServerVerification;

impl rustls::client::danger::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::aws_lc_rs::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use url::Url;

pub enum Upstream {
    Plain(tokio::net::TcpStream),
    Tls(tokio_rustls::client::TlsStream<tokio::net::TcpStream>),
}

impl AsyncRead for Upstream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match &mut *self {
            Self::Plain(stream) => Pin::new(stream).poll_read(cx, buf),
            Self::Tls(stream) => Pin::new(stream).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Upstream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        match &mut *self {
            Self::Plain(stream) => Pin::new(stream).poll_write(cx, buf),
            Self::Tls(stream) => Pin::new(stream).poll_write(cx, buf),
        }
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match &mut *self {
            Self::Plain(stream) => Pin::new(stream).poll_flush(cx),
            Self::Tls(stream) => Pin::new(stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match &mut *self {
            Self::Plain(stream) => Pin::new(stream).poll_shutdown(cx),
            Self::Tls(stream) => Pin::new(stream).poll_shutdown(cx),
        }
    }
}

/// 连接上游（含 DNS、TCP、TLS 握手）的总时限。
const UPSTREAM_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

pub async fn connect_upstream(
    tunnel: &tz_proto::ClientTunnelAssign,
) -> anyhow::Result<Upstream> {
    let target = upstream_target(tunnel);
    match tokio::time::timeout(UPSTREAM_CONNECT_TIMEOUT, connect_target(tunnel)).await {
        Ok(result) => result.map_err(|error| anyhow::anyhow!("connect {target}: {error}")),
        Err(_) => Err(anyhow::anyhow!(
            "connect {target}: timed out after {}s",
            UPSTREAM_CONNECT_TIMEOUT.as_secs()
        )),
    }
}

pub fn upstream_target(tunnel: &tz_proto::ClientTunnelAssign) -> String {
    if tunnel.protocol == "http" {
        if let Some(target_url) = &tunnel.target_url {
            return target_url.clone();
        }
    }
    format!(
        "{}:{}",
        tunnel.target_host.as_deref().unwrap_or("127.0.0.1"),
        tunnel.target_port.unwrap_or(80)
    )
}

async fn connect_target(tunnel: &tz_proto::ClientTunnelAssign) -> anyhow::Result<Upstream> {
    if tunnel.protocol == "http" {
        if let Some(target_url) = &tunnel.target_url {
            return connect_from_url(target_url, tunnel.backend_tls_insecure).await;
        }
    }
    let target_host = tunnel
        .target_host
        .clone()
        .unwrap_or_else(|| "127.0.0.1".into());
    let target_port = tunnel
        .target_port
        .and_then(|value| u16::try_from(value).ok())
        .unwrap_or(80);
    Ok(Upstream::Plain(
        tokio::net::TcpStream::connect((target_host.as_str(), target_port)).await?,
    ))
}

/// `insecure` 为 true 时不校验上游证书（自签可用）；TLS 1.2 / 1.3。
async fn connect_from_url(target_url: &str, insecure: bool) -> anyhow::Result<Upstream> {
    tz_pki::init_rustls_crypto_provider();
    let url = Url::parse(target_url)?;
    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("target URL missing host"))?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    let port = url
        .port_or_known_default()
        .ok_or_else(|| anyhow::anyhow!("target URL missing port"))?;
    let tcp = tokio::net::TcpStream::connect((host.as_str(), port)).await?;
    if url.scheme() != "https" {
        return Ok(Upstream::Plain(tcp));
    }
    let builder = rustls::ClientConfig::builder_with_provider(tz_pki::aws_crypto_provider())
        .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    let config = if insecure {
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(SkipServerVerification))
            .with_no_client_auth()
    } else {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        builder.with_root_certificates(roots).with_no_client_auth()
    };
    let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
    let server_name = rustls::pki_types::ServerName::try_from(host)
        .map_err(|_| anyhow::anyhow!("invalid SNI host"))?;
    let tls = connector.connect(server_name, tcp).await?;
    Ok(Upstream::Tls(tls))
}

pub fn prepend_base_path(buffer: &mut [u8], total: usize, target_url: &Url) -> usize {
    let base = target_url.path();
    if base.is_empty() || base == "/" {
        return total;
    }
    let Ok(text) = std::str::from_utf8(&buffer[..total]) else {
        return total;
    };
    let Some(first_line_end) = text.find("\r\n") else {
        return total;
    };
    let mut parts = text[..first_line_end].split_whitespace();
    let method = parts.next().unwrap_or("GET");
    let path = parts.next().unwrap_or("/");
    let version = parts.next().unwrap_or("HTTP/1.1");
    if !matches!(method, "GET" | "HEAD" | "POST" | "PUT" | "PATCH" | "DELETE" | "OPTIONS") {
        return total;
    }
    let merged = if path.starts_with('/') {
        format!("{base}{path}")
    } else {
        format!("{base}/{path}")
    };
    let rebuilt_first = format!("{method} {merged} {version}");
    let rest = &text[first_line_end..];
    let rebuilt = format!("{rebuilt_first}{rest}");
    let bytes = rebuilt.as_bytes();
    if bytes.len() > buffer.len() {
        return total;
    }
    buffer[..bytes.len()].copy_from_slice(bytes);
    bytes.len()
}

pub fn rewrite_host_header(buffer: &mut [u8], host_rewrite: &str, target_url: &Url) -> bool {
    if host_rewrite == "$http_host" {
        return false;
    }
    let replacement = if host_rewrite == "$target_host" {
        target_url
            .host_str()
            .unwrap_or("localhost")
            .to_string()
    } else {
        host_rewrite.to_string()
    };
    let Ok(text) = std::str::from_utf8(buffer) else {
        return false;
    };
    let Some(header_end) = text.find("\r\n\r\n") else {
        return false;
    };
    let headers = &text[..header_end];
    if !headers
        .lines()
        .any(|line| line.to_ascii_lowercase().starts_with("host:"))
    {
        return false;
    }
    let mut lines: Vec<String> = headers
        .lines()
        .map(|line| {
            if line.to_ascii_lowercase().starts_with("host:") {
                format!("Host: {replacement}")
            } else {
                line.to_string()
            }
        })
        .collect();
    let body = &text[header_end..];
    let rebuilt = format!("{}\r\n{}", lines.join("\r\n"), body);
    let bytes = rebuilt.as_bytes();
    if bytes.len() > buffer.len() {
        return false;
    }
    buffer[..bytes.len()].copy_from_slice(bytes);
    true
}
