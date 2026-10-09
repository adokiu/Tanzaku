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
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message, cert, dss,
            &rustls::crypto::aws_lc_rs::default_provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message, cert, dss,
            &rustls::crypto::aws_lc_rs::default_provider().signature_verification_algorithms,
        )
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
            // 很多上游关闭 TLS 时不发 close_notify；HTTP 报文边界由 Content-Length/chunked 保证，按 EOF 处理。
            Self::Tls(stream) => match Pin::new(stream).poll_read(cx, buf) {
                std::task::Poll::Ready(Err(error))
                    if error.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    std::task::Poll::Ready(Ok(()))
                }
                other => other,
            },
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
        Ok(result) => result.map_err(|error| error.context(format!("connect {target}"))),
        Err(_) => Err(anyhow::anyhow!(
            "connect {target}: timed out after {}s",
            UPSTREAM_CONNECT_TIMEOUT.as_secs()
        )),
    }
}

pub fn upstream_target(tunnel: &tz_proto::ClientTunnelAssign) -> String {
    if tunnel.protocol == "http" {
        if let Some(target_url) = &tunnel.target_url {
            return Url::parse(target_url)
                .map(|url| format!("{}://{}", url.scheme(), &url[url::Position::BeforeHost..url::Position::AfterPort]))
                .unwrap_or_else(|_| "invalid upstream URL".into());
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

fn host_likely_private_or_local(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".local") {
        return true;
    }
    let Ok(ip) = host.parse::<std::net::IpAddr>() else {
        return false;
    };
    match ip {
        std::net::IpAddr::V4(v4) => {
            v4.is_private() || v4.is_loopback() || v4.is_link_local() || v4.is_unspecified()
        }
        std::net::IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || (v6.segments()[0] & 0xfe00) == 0xfc00 // unique local
                || (v6.segments()[0] & 0xffc0) == 0xfe80 // link-local
        }
    }
}

/// `insecure` 为 true 时不校验上游证书（自签可用）；TLS 1.2 / 1.3。
/// 私网/本地 IP 同样校验证书；自签后端必须显式选择 insecure。
async fn connect_from_url(target_url: &str, insecure: bool) -> anyhow::Result<Upstream> {
    tz_pki::init_rustls_crypto_provider();
    let url = Url::parse(target_url)?;
    anyhow::ensure!(matches!(url.scheme(), "http" | "https"), "unsupported upstream URL scheme");
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
    if !insecure && host_likely_private_or_local(&host) {
        tracing::debug!(%host, "verifying private upstream TLS certificate");
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

/// target_url 的路径映射：目录型（`/app/`、`/api`）作为前缀；文件型（`/index.html`）仅映射根路径，
/// 其余请求落在其所在目录下。
struct TargetPath<'a> {
    /// 去掉尾部 `/` 的目录前缀；根目录为空串。
    base: &'a str,
    /// 访问 `/` 时使用的完整路径（文件型 target 指向该文件）。
    root: &'a str,
}

fn target_path(target_url: &Url) -> TargetPath<'_> {
    let path = target_url.path();
    let last = path.rsplit('/').next().unwrap_or_default();
    if last.contains('.') {
        let dir = &path[..path.len() - last.len()];
        TargetPath { base: dir.trim_end_matches('/'), root: path }
    } else {
        TargetPath { base: path.trim_end_matches('/'), root: path }
    }
}

/// 按隧道配置改写发往上游的单个请求：路径前缀与 Host。
pub fn rewrite_request<B>(
    request: &mut hyper::Request<B>,
    host_rewrite: &str,
    target_url: &Url,
) -> anyhow::Result<()> {
    let mapping = target_path(target_url);
    let original = request
        .uri()
        .path_and_query()
        .map(|value| value.as_str().to_owned())
        .unwrap_or_else(|| "/".into());
    if original.starts_with('/') && (!mapping.base.is_empty() || mapping.root != "/") {
        let rewritten = if original == "/" {
            let mut root = if mapping.root.is_empty() { "/".to_owned() } else { mapping.root.to_owned() };
            if let Some(query) = target_url.query() {
                root.push('?');
                root.push_str(query);
            }
            root
        } else {
            format!("{}{original}", mapping.base)
        };
        *request.uri_mut() = rewritten
            .parse()
            .map_err(|error| anyhow::anyhow!("invalid rewritten request path: {error}"))?;
    } else if original == "/" {
        if let Some(query) = target_url.query() {
            *request.uri_mut() = format!("/?{query}")
                .parse()
                .map_err(|error| anyhow::anyhow!("invalid rewritten request path: {error}"))?;
        }
    }
    let replacement = match host_rewrite {
        "" | "$http_host" => None,
        "$target_host" => Some(&target_url[url::Position::BeforeHost..url::Position::AfterPort]),
        value => {
            // Host 头按 RFC 9110 只允许 host[:port]；误填完整 URL 时拒绝请求而不是发出畸形 Host。
            anyhow::ensure!(
                !value.contains("://"),
                "upstream Host must not contain a scheme (use host[:port])"
            );
            Some(value)
        }
    };
    if let Some(host) = replacement {
        anyhow::ensure!(
            !host.is_empty() && host.bytes().all(|byte| byte.is_ascii_graphic()),
            "invalid upstream Host"
        );
        let value = hyper::header::HeaderValue::from_str(host)
            .map_err(|_| anyhow::anyhow!("invalid upstream Host"))?;
        request.headers_mut().insert(hyper::header::HOST, value);
    }
    Ok(())
}

/// 上游重定向指向自身内网地址或带路径前缀时，改回访客可访问的相对路径（等同 nginx `proxy_redirect default`）。
pub fn rewrite_location(location: &str, target_url: &Url) -> Option<String> {
    let origin = &target_url[..url::Position::AfterPort];
    let path = if let Some(rest) = location.strip_prefix(origin) {
        if rest.is_empty() { "/" } else if rest.starts_with('/') || rest.starts_with('?') { rest } else { return None }
    } else if location.starts_with('/') && !location.starts_with("//") {
        location
    } else {
        return None;
    };
    let base = target_path(target_url).base;
    let stripped = if base.is_empty() {
        path
    } else if let Some(rest) = path.strip_prefix(base) {
        if rest.is_empty() {
            "/"
        } else if rest.starts_with('/') || rest.starts_with('?') {
            rest
        } else {
            path
        }
    } else {
        path
    };
    let stripped = if stripped.starts_with('?') { format!("/{stripped}") } else { stripped.to_owned() };
    (stripped != location).then_some(stripped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(path: &str, host: &str) -> hyper::Request<()> {
        hyper::Request::builder()
            .uri(path)
            .header(hyper::header::HOST, host)
            .body(())
            .unwrap()
    }

    #[test]
    fn root_target_keeps_path_and_host() {
        let target = Url::parse("http://192.168.1.5:8080/").unwrap();
        let mut req = request("/assets/app.js?v=1", "public.example");
        rewrite_request(&mut req, "$http_host", &target).unwrap();
        assert_eq!(req.uri(), "/assets/app.js?v=1");
        assert_eq!(req.headers()[hyper::header::HOST], "public.example");
    }

    #[test]
    fn directory_target_prefixes_every_request() {
        let target = Url::parse("http://backend.example/api/v1").unwrap();
        let mut req = request("/users", "public.example");
        rewrite_request(&mut req, "$http_host", &target).unwrap();
        assert_eq!(req.uri(), "/api/v1/users");
        let mut root = request("/", "public.example");
        rewrite_request(&mut root, "$http_host", &target).unwrap();
        assert_eq!(root.uri(), "/api/v1");
    }

    #[test]
    fn file_target_only_maps_root() {
        let target = Url::parse("http://backend.example/app/index.html").unwrap();
        let mut root = request("/", "x");
        rewrite_request(&mut root, "", &target).unwrap();
        assert_eq!(root.uri(), "/app/index.html");
        let mut asset = request("/main.css", "x");
        rewrite_request(&mut asset, "", &target).unwrap();
        assert_eq!(asset.uri(), "/app/main.css");
    }

    #[test]
    fn host_rewrite_variants() {
        let target = Url::parse("https://[::1]:8443/").unwrap();
        let mut req = request("/", "source");
        rewrite_request(&mut req, "$target_host", &target).unwrap();
        assert_eq!(req.headers()[hyper::header::HOST], "[::1]:8443");
        let mut custom = request("/", "source");
        rewrite_request(&mut custom, "backend.local", &target).unwrap();
        assert_eq!(custom.headers()[hyper::header::HOST], "backend.local");
        let mut bad = request("/", "source");
        assert!(rewrite_request(&mut bad, "host\r\nInjected: yes", &target).is_err());
        let mut scheme = request("/", "source");
        assert!(rewrite_request(&mut scheme, "https://backend.local", &target).is_err());
    }

    #[test]
    fn location_to_backend_origin_becomes_relative() {
        let target = Url::parse("http://192.168.1.5:8080/").unwrap();
        assert_eq!(
            rewrite_location("http://192.168.1.5:8080/login?next=/", &target).as_deref(),
            Some("/login?next=/")
        );
        assert_eq!(rewrite_location("/login", &target), None);
        assert_eq!(rewrite_location("https://other.example/x", &target), None);
    }

    #[test]
    fn location_strips_target_base_path() {
        let target = Url::parse("http://backend.example/app/").unwrap();
        assert_eq!(rewrite_location("/app/login", &target).as_deref(), Some("/login"));
        assert_eq!(rewrite_location("/app", &target).as_deref(), Some("/"));
        assert_eq!(rewrite_location("/application", &target), None);
    }
}
