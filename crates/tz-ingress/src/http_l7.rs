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
    let service = service_fn(move |request| {
        let ctx = ctx.clone();
        handle_shared_request(request, peer, ctx, scheme)
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
    let conn_ctx = ConnCtx {
        peer: peer.ip(),
        trust: SourceTrust::Tcp,
        trusted_proxy: false,
        tunnel_id: None,
    };
    let verdict = ctx.guard.check_conn(&conn_ctx);
    if ctx.guard.record_if_denied(&verdict, peer.ip(), None) {
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
    if ctx.guard.record_if_denied(&http_verdict, peer.ip(), None) {
        route.traffic.record_reject_guard();
        return Ok(text_response(StatusCode::TOO_MANY_REQUESTS, "http guard rejected"));
    }
    if !route.try_acquire() {
        return Ok(text_response(StatusCode::TOO_MANY_REQUESTS, "too many connections"));
    }
    let slot = ConnSlot(Arc::new(route.clone()));
    let target = ProxyTarget {
        tunnel_id: route.tunnel_id,
        session: route.client_session.clone(),
        speed_limit_mbps: route.speed_limit_mbps,
        traffic: route.traffic.clone(),
        pool: ctx.pool.clone(),
        rate_clock: ctx.rate_clock.clone(),
    };
    Ok(proxy_request(request, peer, scheme, target, slot).await)
}

#[cfg(feature = "http")]
struct ProxyTarget {
    tunnel_id: uuid::Uuid,
    session: Arc<dyn CarrierSession>,
    speed_limit_mbps: i64,
    traffic: Arc<TunnelTrafficAccounting>,
    pool: BufferPool,
    rate_clock: RateClock,
}

/// 每个请求独占一条 carrier 流：hyper 客户端负责 HTTP/1 报文边界（Content-Length / chunked / 升级），
/// 字节经 pump 进出 carrier，从而沿用限速与流量统计。`conn_guard` 在该流结束时释放并发名额。
#[cfg(feature = "http")]
async fn proxy_request<G: Send + 'static>(
    mut request: Request<Incoming>,
    peer: SocketAddr,
    scheme: &'static str,
    target: ProxyTarget,
    conn_guard: G,
) -> Response<Body> {
    let header = StreamHeader {
        tunnel_id: target.tunnel_id,
        src_addr: peer,
    };
    let carrier = match target.session.open_stream(header).await {
        Ok(carrier) => carrier,
        Err(error) => {
            tracing::warn!(tunnel_id = %target.tunnel_id, ?error, "http proxy: carrier stream open failed");
            return text_response(StatusCode::BAD_GATEWAY, "carrier unavailable");
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
        let _conn_guard = conn_guard;
        tokio::select! {
            _ = tz_net::pump::pump(carrier_io, carrier, pool, options) => {}
            _ = traffic.connections_closed() => {}
        }
    });

    let (mut sender, connection) =
        match hyper::client::conn::http1::Builder::new().handshake(TokioIo::new(proxy_io)).await {
            Ok(parts) => parts,
            Err(error) => {
                tracing::warn!(tunnel_id = %target.tunnel_id, ?error, "http proxy: upstream handshake failed");
                return text_response(StatusCode::BAD_GATEWAY, "upstream unavailable");
            }
        };
    let tunnel_id = target.tunnel_id;
    tokio::spawn(async move {
        if let Err(error) = connection.with_upgrades().await {
            tracing::debug!(%tunnel_id, ?error, "http proxy: upstream connection ended");
        }
    });

    let wants_upgrade = request_wants_upgrade(&request);
    let client_upgrade = wants_upgrade.then(|| hyper::upgrade::on(&mut request));
    prepare_upstream_request(&mut request, peer, scheme, wants_upgrade);

    let mut response = match sender.send_request(request).await {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(%tunnel_id, ?error, "http proxy: upstream request failed");
            return text_response(StatusCode::BAD_GATEWAY, "upstream request failed");
        }
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
    }
    strip_hop_by_hop(response.headers_mut(), switching);
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
    } else {
        headers.insert(header::CONNECTION, HeaderValue::from_static("close"));
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
    if !headers.contains_key("x-forwarded-proto") {
        headers.insert(
            HeaderName::from_static("x-forwarded-proto"),
            HeaderValue::from_static(scheme),
        );
    }
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

#[cfg(feature = "http")]
pub async fn serve_dedicated_hyper<S>(
    stream: S,
    peer: SocketAddr,
    endpoint: DedicatedEndpoint,
) -> std::io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let io = TokioIo::new(stream);
    let endpoint = Arc::new(endpoint);
    let service = service_fn(move |request| {
        let endpoint = endpoint.clone();
        handle_dedicated_request(request, peer, endpoint)
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
    let ctx = ConnCtx {
        peer: peer.ip(),
        trust: SourceTrust::Tcp,
        trusted_proxy: false,
        tunnel_id: Some(endpoint.tunnel_id),
    };
    let verdict = endpoint.guard.check_conn(&ctx);
    if endpoint
        .guard
        .record_if_denied(&verdict, peer.ip(), Some(endpoint.tunnel_id))
    {
        endpoint.traffic.record_reject_guard();
        return Ok(text_response(StatusCode::FORBIDDEN, "denied"));
    }
    if !endpoint.traffic.try_acquire_conn(endpoint.max_conns) {
        endpoint.traffic.record_reject_quota();
        return Ok(text_response(StatusCode::TOO_MANY_REQUESTS, "too many connections"));
    }
    let conn_guard = DedicatedConnGuard(endpoint.traffic.clone());
    let target = ProxyTarget {
        tunnel_id: endpoint.tunnel_id,
        session: endpoint.client_session.clone(),
        speed_limit_mbps: endpoint.speed_limit_mbps,
        traffic: endpoint.traffic.clone(),
        pool: endpoint.pool.clone(),
        rate_clock: endpoint.rate_clock.clone(),
    };
    Ok(proxy_request(request, peer, "http", target, conn_guard).await)
}
