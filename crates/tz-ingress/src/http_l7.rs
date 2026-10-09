#[cfg(feature = "http")]
use std::{convert::Infallible, net::SocketAddr, sync::Arc};

#[cfg(feature = "http")]
use bytes::Bytes;
#[cfg(feature = "http")]
use http_body_util::{BodyExt, Full, combinators::BoxBody};
#[cfg(feature = "http")]
use hyper::body::Incoming;
#[cfg(feature = "http")]
use hyper::header::{self, HeaderMap, HeaderName, HeaderValue};
#[cfg(feature = "http")]
use hyper::client::conn::http1::SendRequest;
#[cfg(feature = "http")]
use hyper::server::conn::http1;
#[cfg(feature = "http")]
use hyper::service::service_fn;
#[cfg(feature = "http")]
use hyper::{Request, Response, StatusCode, Uri};
#[cfg(feature = "http")]
use hyper_util::rt::TokioIo;
#[cfg(feature = "http")]
use tokio::io::{AsyncRead, AsyncWrite};
#[cfg(feature = "http")]
use tz_carrier::{registry::CarrierSession, types::StreamHeader};
#[cfg(feature = "http")]
use tz_guard::types::{ConnCtx, HttpCtx, SourceTrust};
#[cfg(feature = "http")]
use tz_net::pool::BufferPool;

#[cfg(feature = "http")]
use crate::{
    limits::pump_options,
    shared_http::{ConnSlot, SharedHttpRoute},
    traffic::TunnelTrafficAccounting,
};
#[cfg(feature = "http")]
use tz_guard::pipeline::GuardPipeline;
#[cfg(feature = "http")]
use tz_net::rate::RateClock;

#[cfg(feature = "http")]
type BoxError = Box<dyn std::error::Error + Send + Sync>;
#[cfg(feature = "http")]
type Body = BoxBody<Bytes, BoxError>;

#[cfg(feature = "http")]
pub fn is_websocket_upgrade(prefix: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(prefix) else {
        return false;
    };
    let Some(header_end) = text.find("\r\n\r\n") else {
        return false;
    };
    let headers = &text[..header_end];
    let mut connection_upgrade = false;
    let mut upgrade_ws = false;
    for line in headers.lines().skip(1) {
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("connection:") && lower.contains("upgrade") {
            connection_upgrade = true;
        }
        if lower.starts_with("upgrade:") && lower.contains("websocket") {
            upgrade_ws = true;
        }
    }
    connection_upgrade && upgrade_ws
}

#[cfg(feature = "http")]
pub struct SharedRequestContext {
    pub routes: Arc<arc_swap::ArcSwap<std::collections::HashMap<String, SharedHttpRoute>>>,
    pub guard: Arc<GuardPipeline>,
    pub pool: BufferPool,
    pub rate_clock: RateClock,
    pub filing: Arc<arc_swap::ArcSwap<crate::filing::FilingGate>>,
}

/// 访客一条 TCP 连接的状态：防护只在首次命中某隧道时按「新连接」计数；
/// 上游 carrier 流在该连接的多个请求间复用（keep-alive）。
#[cfg(feature = "http")]
#[derive(Default)]
struct InboundConn {
    admitted: tokio::sync::Mutex<Option<uuid::Uuid>>,
    upstream: tokio::sync::Mutex<Option<(uuid::Uuid, SendRequest<Incoming>)>>,
}

#[cfg(feature = "http")]
impl InboundConn {
    /// 返回 true 表示放行；同一连接对同一隧道只做一次 `check_conn`。
    async fn admit(
        &self,
        guard: &GuardPipeline,
        peer: SocketAddr,
        tunnel_id: uuid::Uuid,
    ) -> bool {
        let mut admitted = self.admitted.lock().await;
        if *admitted == Some(tunnel_id) {
            return true;
        }
        let ctx = ConnCtx {
            peer: peer.ip(),
            trust: SourceTrust::Tcp,
            trusted_proxy: false,
            tunnel_id: Some(tunnel_id),
        };
        let verdict = guard.check_conn(&ctx);
        if guard.record_if_denied(&verdict, peer.ip(), Some(tunnel_id)) {
            return false;
        }
        *admitted = Some(tunnel_id);
        true
    }
}

/// 共享入口（HTTP 或已终结 TLS 的 HTTPS）：按 Host 选隧道后反向代理。
#[cfg(feature = "http")]
pub async fn serve_inbound_hyper<S>(
    stream: S,
    peer: SocketAddr,
    ctx: Arc<SharedRequestContext>,
    scheme: &'static str,
) -> std::io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let io = TokioIo::new(stream);
    let conn = Arc::new(InboundConn::default());
    let service = service_fn(move |request| {
        let ctx = ctx.clone();
        let conn = conn.clone();
        async move { handle_shared_request(request, peer, ctx, scheme, conn).await }
    });
    http1::Builder::new()
        .serve_connection(io, service)
        .with_upgrades()
        .await
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err.to_string()))
}

#[cfg(feature = "http")]
async fn handle_shared_request(
    request: Request<Incoming>,
    peer: SocketAddr,
    ctx: Arc<SharedRequestContext>,
    scheme: &'static str,
    conn: Arc<InboundConn>,
) -> Result<Response<Body>, Infallible> {
    let Some(host) = request_host(&request) else {
        return Ok(text_response(StatusCode::BAD_REQUEST, "missing host"));
    };
    if ctx.filing.load().blocks(&host) {
        return Ok(filing_html_response());
    }
    let name = host_name(&host);
    let Some(route) = ctx.routes.load().get(&name).cloned() else {
        return Ok(text_response(StatusCode::NOT_FOUND, "domain not found"));
    };
    let tunnel_id = Some(route.tunnel_id);
    if !conn.admit(&ctx.guard, peer, route.tunnel_id).await {
        route.traffic.record_reject_guard();
        return Ok(text_response(StatusCode::FORBIDDEN, "denied"));
    }
    let http_ctx = HttpCtx {
        peer: peer.ip(),
        trust: SourceTrust::Tcp,
        header_bytes: estimate_header_bytes(request.headers(), request.uri()),
        header_count: request.headers().len(),
        request_rate_slot: 0,
    };
    let http_verdict = ctx.guard.check_http(&http_ctx);
    if ctx.guard.record_if_denied(&http_verdict, peer.ip(), tunnel_id) {
        route.traffic.record_reject_guard();
        return Ok(text_response(StatusCode::TOO_MANY_REQUESTS, "http guard rejected"));
    }
    let slot_route = route.clone();
    let target = ProxyTarget {
        tunnel_id: route.tunnel_id,
        session: route.client_session.clone(),
        speed_limit_mbps: route.speed_limit_mbps,
        traffic: route.traffic.clone(),
        pool: ctx.pool.clone(),
        rate_clock: ctx.rate_clock.clone(),
        acquire_slot: Box::new(move || {
            slot_route
                .try_acquire()
                .then(|| Box::new(ConnSlot(Arc::new(slot_route.clone()))) as Box<dyn Send>)
        }),
    };
    Ok(proxy_request(request, peer, scheme, target, &conn).await)
}

#[cfg(feature = "http")]
struct ProxyTarget {
    tunnel_id: uuid::Uuid,
    session: Arc<dyn CarrierSession>,
    speed_limit_mbps: i64,
    traffic: Arc<TunnelTrafficAccounting>,
    pool: BufferPool,
    rate_clock: RateClock,
    /// 新开一条 carrier 流前占用隧道并发名额；名额随该流结束释放。
    acquire_slot: Box<dyn Fn() -> Option<Box<dyn Send>> + Send + Sync>,
}

/// 复用 carrier 流前等待其空闲的上限；超时视为已坏，改开新流。
#[cfg(feature = "http")]
const UPSTREAM_READY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// 为访客连接打开一条 carrier 流并在其上建立 HTTP/1 客户端连接。
/// hyper 负责报文边界（Content-Length / chunked / 升级），字节经 pump 进出 carrier，沿用限速与流量统计。
#[cfg(feature = "http")]
async fn open_upstream(target: &ProxyTarget, peer: SocketAddr) -> Result<SendRequest<Incoming>, Response<Body>> {
    let Some(slot) = (target.acquire_slot)() else {
        return Err(text_response(StatusCode::TOO_MANY_REQUESTS, "too many connections"));
    };
    let header = StreamHeader {
        tunnel_id: target.tunnel_id,
        src_addr: peer,
    };
    let carrier = match target.session.open_stream(header).await {
        Ok(carrier) => carrier,
        Err(error) => {
            tracing::warn!(tunnel_id = %target.tunnel_id, ?error, "http proxy: carrier stream open failed");
            return Err(text_response(StatusCode::BAD_GATEWAY, "carrier unavailable"));
        }
    };
    let (proxy_io, carrier_io) = tokio::io::duplex(64 * 1024);
    let options = pump_options(
        &target.rate_clock,
        target.speed_limit_mbps,
        target.traffic.meter.clone(),
    );
    let pool = target.pool.clone();
    let traffic = target.traffic.clone();
    tokio::spawn(async move {
        let _slot = slot;
        tokio::select! {
            _ = tz_net::pump::pump(carrier_io, carrier, pool, options) => {}
            _ = traffic.connections_closed() => {}
        }
    });
    let (sender, connection) =
        match hyper::client::conn::http1::Builder::new().handshake(TokioIo::new(proxy_io)).await {
            Ok(parts) => parts,
            Err(error) => {
                tracing::warn!(tunnel_id = %target.tunnel_id, ?error, "http proxy: upstream handshake failed");
                return Err(text_response(StatusCode::BAD_GATEWAY, "upstream unavailable"));
            }
        };
    let tunnel_id = target.tunnel_id;
    tokio::spawn(async move {
        if let Err(error) = connection.with_upgrades().await {
            tracing::debug!(%tunnel_id, ?error, "http proxy: upstream connection ended");
        }
    });
    Ok(sender)
}

#[cfg(feature = "http")]
async fn proxy_request(
    mut request: Request<Incoming>,
    peer: SocketAddr,
    scheme: &'static str,
    target: ProxyTarget,
    conn: &InboundConn,
) -> Response<Body> {
    let tunnel_id = target.tunnel_id;
    let public_host = request_host(&request);
    let wants_upgrade = request_wants_upgrade(&request);
    let client_upgrade = wants_upgrade.then(|| hyper::upgrade::on(&mut request));
    prepare_upstream_request(&mut request, peer, scheme, wants_upgrade);

    // 升级请求独占新流（升级后该流不再承载 HTTP 请求）；普通请求优先复用本连接已有的流。
    let reused = if wants_upgrade {
        None
    } else {
        conn.upstream
            .lock()
            .await
            .take()
            .filter(|(id, _)| *id == tunnel_id)
            .map(|(_, sender)| sender)
    };
    let reused = match reused {
        Some(mut sender) => {
            let ready = tokio::time::timeout(UPSTREAM_READY_TIMEOUT, sender.ready()).await;
            matches!(ready, Ok(Ok(()))).then_some(sender)
        }
        None => None,
    };
    let mut sender = match reused {
        Some(sender) => sender,
        None => match open_upstream(&target, peer).await {
            Ok(sender) => sender,
            Err(response) => return response,
        },
    };

    let mut response = match sender.try_send_request(request).await {
        Ok(response) => response,
        Err(mut error) => match error.take_message() {
            // 复用的流恰好已关闭、请求未发出：换新流重发一次。
            Some(request) => {
                sender = match open_upstream(&target, peer).await {
                    Ok(sender) => sender,
                    Err(response) => return response,
                };
                match sender.send_request(request).await {
                    Ok(response) => response,
                    Err(error) => {
                        tracing::warn!(%tunnel_id, ?error, "http proxy: upstream request failed");
                        return text_response(StatusCode::BAD_GATEWAY, "upstream request failed");
                    }
                }
            }
            None => {
                tracing::warn!(%tunnel_id, error = ?error.into_error(), "http proxy: upstream request failed");
                return text_response(StatusCode::BAD_GATEWAY, "upstream request failed");
            }
        },
    };

    let switching = response.status() == StatusCode::SWITCHING_PROTOCOLS;
    if switching {
        if let Some(client_upgrade) = client_upgrade {
            let upstream_upgrade = hyper::upgrade::on(&mut response);
            tokio::spawn(async move {
                let (Ok(client), Ok(upstream)) = tokio::join!(client_upgrade, upstream_upgrade) else {
                    return;
                };
                let mut client = TokioIo::new(client);
                let mut upstream = TokioIo::new(upstream);
                let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
            });
        }
    } else {
        *conn.upstream.lock().await = Some((tunnel_id, sender));
    }
    strip_hop_by_hop(response.headers_mut(), switching);
    rewrite_public_location(response.headers_mut(), public_host.as_deref(), scheme);
    response.map(|body| body.map_err(|error| Box::new(error) as BoxError).boxed())
}

#[cfg(feature = "http")]
fn prepare_upstream_request(
    request: &mut Request<Incoming>,
    peer: SocketAddr,
    scheme: &'static str,
    upgrade: bool,
) {
    let authority = request.uri().authority().map(|value| value.as_str().to_owned());
    let origin_form = request
        .uri()
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or("/")
        .parse::<Uri>()
        .unwrap_or_else(|_| Uri::from_static("/"));
    *request.uri_mut() = origin_form;
    *request.version_mut() = hyper::Version::HTTP_11;

    let headers = request.headers_mut();
    if !headers.contains_key(header::HOST) {
        if let Some(value) = authority.and_then(|value| HeaderValue::from_str(&value).ok()) {
            headers.insert(header::HOST, value);
        }
    }
    strip_hop_by_hop(headers, upgrade);
    if upgrade {
        headers.insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
    }

    let peer_ip = peer.ip().to_string();
    let forwarded_for = match headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
    {
        Some(existing) if !existing.trim().is_empty() => format!("{existing}, {peer_ip}"),
        _ => peer_ip.clone(),
    };
    if let Ok(value) = HeaderValue::from_str(&forwarded_for) {
        headers.insert(HeaderName::from_static("x-forwarded-for"), value);
    }
    if !headers.contains_key("x-real-ip") {
        if let Ok(value) = HeaderValue::from_str(&peer_ip) {
            headers.insert(HeaderName::from_static("x-real-ip"), value);
        }
    }
    // 以节点实际终结的协议为准，不信任访客自带的值。
    headers.insert(
        HeaderName::from_static("x-forwarded-proto"),
        HeaderValue::from_static(scheme),
    );
    if !headers.contains_key("x-forwarded-host") {
        if let Some(host) = headers.get(header::HOST).cloned() {
            headers.insert(HeaderName::from_static("x-forwarded-host"), host);
        }
    }
}

/// RFC 9110 §7.6.1 逐跳头；升级握手时保留 Connection/Upgrade。
#[cfg(feature = "http")]
fn strip_hop_by_hop(headers: &mut HeaderMap, keep_upgrade: bool) {
    let listed: Vec<HeaderName> = headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|name| HeaderName::from_bytes(name.trim().as_bytes()).ok())
        .filter(|name| !(keep_upgrade && name == header::UPGRADE))
        .collect();
    for name in listed {
        headers.remove(name);
    }
    for name in ["keep-alive", "proxy-connection", "te", "trailer"] {
        headers.remove(name);
    }
    if !keep_upgrade {
        headers.remove(header::CONNECTION);
        headers.remove(header::UPGRADE);
    }
}

#[cfg(feature = "http")]
fn request_wants_upgrade(request: &Request<Incoming>) -> bool {
    let connection_upgrade = request
        .headers()
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .any(|value| value.split(',').any(|token| token.trim().eq_ignore_ascii_case("upgrade")));
    connection_upgrade && request.headers().contains_key(header::UPGRADE)
}

#[cfg(feature = "http")]
fn request_host(request: &Request<Incoming>) -> Option<String> {
    request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .or_else(|| request.uri().authority().map(|value| value.as_str().to_owned()))
        .filter(|value| !value.trim().is_empty())
}

#[cfg(feature = "http")]
fn host_name(host: &str) -> String {
    let host = host.trim().to_ascii_lowercase();
    if host.starts_with('[') {
        return host;
    }
    host.split(':').next().unwrap_or_default().to_owned()
}

#[cfg(feature = "http")]
fn filing_html_response() -> Response<Body> {
    Response::builder()
        .status(StatusCode::FORBIDDEN)
        .header("content-type", "text/html; charset=utf-8")
        .header("cache-control", "no-store")
        .body(full_body(Bytes::from_static(
            crate::filing::filing_html().as_bytes(),
        )))
        .expect("response")
}

fn text_response(status: StatusCode, message: &str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("content-type", "text/plain; charset=utf-8")
        .body(full_body(Bytes::from(format!("{message}\n"))))
        .expect("response")
}

#[cfg(feature = "http")]
fn full_body(bytes: Bytes) -> Body {
    Full::new(bytes)
        .map_err(|never| -> BoxError { match never {} })
        .boxed()
}

#[cfg(feature = "http")]
fn estimate_header_bytes(headers: &HeaderMap, uri: &Uri) -> usize {
    let path = uri.path_and_query().map(|value| value.as_str()).unwrap_or("/");
    headers
        .iter()
        .map(|(name, value)| name.as_str().len() + value.len() + 4)
        .sum::<usize>()
        + path.len()
        + 16
}

#[cfg(feature = "http")]
pub struct DedicatedEndpoint {
    pub tunnel_id: uuid::Uuid,
    pub client_session: Arc<dyn CarrierSession>,
    pub http_hosts: Arc<arc_swap::ArcSwap<crate::types::DedicatedHttpHosts>>,
    pub traffic: Arc<TunnelTrafficAccounting>,
    pub speed_limit_mbps: i64,
    pub max_conns: i32,
    pub guard: Arc<GuardPipeline>,
    pub pool: BufferPool,
    pub rate_clock: RateClock,
    pub filing: Arc<arc_swap::ArcSwap<crate::filing::FilingGate>>,
}

#[cfg(feature = "http")]
struct DedicatedConnGuard(Arc<TunnelTrafficAccounting>);

#[cfg(feature = "http")]
impl Drop for DedicatedConnGuard {
    fn drop(&mut self) {
        self.0.release_conn();
    }
}

/// 独立端口一条访客连接的处理方式（由公网协议设置与实际到达的协议决定）。
#[cfg(feature = "http")]
#[derive(Debug, Clone, Copy)]
pub enum DedicatedMode {
    /// 正常反向代理；参数为访客实际使用的公网协议（写入 X-Forwarded-Proto）。
    Proxy(&'static str),
    /// 隧道启用了 HTTPS，访客却用明文 HTTP 访问：308 跳转到同端口 HTTPS。
    RedirectHttps,
    /// 隧道未启用 HTTPS，访客却用 TLS 访问：返回提示，不代理。
    HttpOnlyNotice,
}

#[cfg(feature = "http")]
pub async fn serve_dedicated_hyper<S>(
    stream: S,
    peer: SocketAddr,
    endpoint: DedicatedEndpoint,
    mode: DedicatedMode,
) -> std::io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let io = TokioIo::new(stream);
    let endpoint = Arc::new(endpoint);
    let conn = Arc::new(InboundConn::default());
    let service = service_fn(move |request| {
        let endpoint = endpoint.clone();
        let conn = conn.clone();
        async move { handle_dedicated_request(request, peer, endpoint, conn, mode).await }
    });
    http1::Builder::new()
        .serve_connection(io, service)
        .with_upgrades()
        .await
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err.to_string()))
}

#[cfg(feature = "http")]
async fn handle_dedicated_request(
    request: Request<Incoming>,
    peer: SocketAddr,
    endpoint: Arc<DedicatedEndpoint>,
    conn: Arc<InboundConn>,
    mode: DedicatedMode,
) -> Result<Response<Body>, Infallible> {
    let Some(host) = request_host(&request) else {
        return Ok(text_response(StatusCode::BAD_REQUEST, "missing host"));
    };
    if endpoint.filing.load().blocks(&host) {
        return Ok(filing_html_response());
    }
    if !endpoint.http_hosts.load().allows(&host) {
        return Ok(text_response(StatusCode::NOT_FOUND, "domain not found"));
    }
    let scheme = match mode {
        DedicatedMode::Proxy(scheme) => scheme,
        DedicatedMode::RedirectHttps => return Ok(https_redirect_response(&host, request.uri())),
        DedicatedMode::HttpOnlyNotice => {
            return Ok(text_response(
                StatusCode::BAD_REQUEST,
                "This tunnel does not enable HTTPS. Please use http:// instead.\n该隧道未启用 HTTPS，请使用 http:// 访问。",
            ));
        }
    };
    let tunnel_id = Some(endpoint.tunnel_id);
    if !conn.admit(&endpoint.guard, peer, endpoint.tunnel_id).await {
        endpoint.traffic.record_reject_guard();
        return Ok(text_response(StatusCode::FORBIDDEN, "denied"));
    }
    let http_ctx = HttpCtx {
        peer: peer.ip(),
        trust: SourceTrust::Tcp,
        header_bytes: estimate_header_bytes(request.headers(), request.uri()),
        header_count: request.headers().len(),
        request_rate_slot: 0,
    };
    let http_verdict = endpoint.guard.check_http(&http_ctx);
    if endpoint
        .guard
        .record_if_denied(&http_verdict, peer.ip(), tunnel_id)
    {
        endpoint.traffic.record_reject_guard();
        return Ok(text_response(StatusCode::TOO_MANY_REQUESTS, "http guard rejected"));
    }
    let traffic = endpoint.traffic.clone();
    let max_conns = endpoint.max_conns;
    let target = ProxyTarget {
        tunnel_id: endpoint.tunnel_id,
        session: endpoint.client_session.clone(),
        speed_limit_mbps: endpoint.speed_limit_mbps,
        traffic: endpoint.traffic.clone(),
        pool: endpoint.pool.clone(),
        rate_clock: endpoint.rate_clock.clone(),
        acquire_slot: Box::new(move || {
            if traffic.try_acquire_conn(max_conns) {
                Some(Box::new(DedicatedConnGuard(traffic.clone())) as Box<dyn Send>)
            } else {
                traffic.record_reject_quota();
                None
            }
        }),
    };
    Ok(proxy_request(request, peer, scheme, target, &conn).await)
}

#[cfg(feature = "http")]
fn https_redirect_response(host: &str, uri: &Uri) -> Response<Body> {
    let path = uri.path_and_query().map(|value| value.as_str()).unwrap_or("/");
    let location = format!("https://{host}{path}");
    match HeaderValue::from_str(&location) {
        Ok(value) => Response::builder()
            .status(StatusCode::PERMANENT_REDIRECT)
            .header(header::LOCATION, value)
            .header("content-type", "text/plain; charset=utf-8")
            .body(full_body(Bytes::from(format!("Redirecting to {location}\n"))))
            .expect("response"),
        Err(_) => text_response(StatusCode::BAD_REQUEST, "invalid host"),
    }
}

/// 源站按自身协议拼出的绝对跳转（如源站是 https，Location 为 `https://<公网 Host>/...`），
/// 若与访客实际使用的公网协议不一致则改正协议，等同 nginx `proxy_redirect` 的默认改写。
#[cfg(feature = "http")]
fn rewrite_public_location(headers: &mut HeaderMap, public_host: Option<&str>, scheme: &str) {
    let Some(public_host) = public_host else {
        return;
    };
    let Some(location) = headers.get(header::LOCATION).and_then(|value| value.to_str().ok()) else {
        return;
    };
    let other = if scheme == "https" { "http://" } else { "https://" };
    let Some(rest) = location
        .get(..other.len())
        .filter(|prefix| prefix.eq_ignore_ascii_case(other))
        .map(|_| &location[other.len()..])
    else {
        return;
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    if !rest[..authority_end].eq_ignore_ascii_case(public_host.trim()) {
        return;
    }
    if let Ok(value) = HeaderValue::from_str(&format!("{scheme}://{rest}")) {
        headers.insert(header::LOCATION, value);
    }
}

#[cfg(all(test, feature = "http"))]
mod tests {
    use super::*;

    fn location_after(location: &str, public_host: &str, scheme: &str) -> String {
        let mut headers = HeaderMap::new();
        headers.insert(header::LOCATION, HeaderValue::from_str(location).unwrap());
        rewrite_public_location(&mut headers, Some(public_host), scheme);
        headers[header::LOCATION].to_str().unwrap().to_owned()
    }

    #[test]
    fn https_origin_redirect_follows_public_http_scheme() {
        assert_eq!(
            location_after("https://1.2.3.4:23584/login?next=/", "1.2.3.4:23584", "http"),
            "http://1.2.3.4:23584/login?next=/"
        );
    }

    #[test]
    fn http_origin_redirect_follows_public_https_scheme() {
        assert_eq!(
            location_after("http://app.example.com:8443/", "APP.example.com:8443", "https"),
            "https://app.example.com:8443/"
        );
    }

    #[test]
    fn foreign_or_matching_locations_untouched() {
        assert_eq!(
            location_after("https://other.example/", "1.2.3.4:23584", "http"),
            "https://other.example/"
        );
        assert_eq!(location_after("/relative", "1.2.3.4:23584", "http"), "/relative");
        assert_eq!(
            location_after("http://1.2.3.4:23584/", "1.2.3.4:23584", "http"),
            "http://1.2.3.4:23584/"
        );
    }
}
