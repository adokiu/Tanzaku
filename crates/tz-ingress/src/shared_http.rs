use crate::limits::pump_options;
use arc_swap::ArcSwap;
use dashmap::DashMap;
use std::{collections::HashMap, io, sync::Arc};
use tz_carrier::{
    registry::CarrierSession,
    types::StreamHeader,
};
use tz_guard::{
    pipeline::GuardPipeline,
    types::{ConnCtx, SourceTrust},
};
use tz_net::{budget::Budget, pool::BufferPool, rate::RateClock};

use crate::shutdown::IngressShutdown;
use uuid::Uuid;

use crate::traffic::TunnelTrafficAccounting;

#[derive(Clone)]
pub struct SharedHttpRoute {
    pub tunnel_id: Uuid,
    pub client_session: Arc<dyn CarrierSession>,
    pub speed_limit_mbps: i64,
    pub max_conns: i32,
    pub traffic: Arc<TunnelTrafficAccounting>,
}

#[derive(Clone)]
pub struct SharedHttpListener {
    pub bind_addr: String,
    pub port: u16,
    pub routes: Arc<ArcSwap<HashMap<String, SharedHttpRoute>>>,
    pub guard: Arc<GuardPipeline>,
    pub rate_clock: RateClock,
    pub shutdown: IngressShutdown,
    pub filing: Arc<ArcSwap<crate::filing::FilingGate>>,
}

impl SharedHttpRoute {
    pub fn try_acquire(&self) -> bool {
        if self.traffic.try_acquire_conn(self.max_conns) {
            return true;
        }
        self.traffic.record_reject_quota();
        false
    }

    fn release(&self) {
        self.traffic.release_conn();
    }
}

pub(crate) struct ConnSlot(pub(crate) Arc<SharedHttpRoute>);

impl Drop for ConnSlot {
    fn drop(&mut self) {
        self.0.release();
    }
}

pub async fn serve(listener: SharedHttpListener) -> io::Result<()> {
    let socket = tokio::net::TcpListener::bind((listener.bind_addr.as_str(), listener.port)).await?;
    let budget = Budget::new(64 * 1024 * 1024);
    let pool = BufferPool::new(
        std::num::NonZeroUsize::new(32 * 1024).expect("chunk"),
        std::num::NonZeroUsize::new(256).expect("count"),
        &budget,
    )
    .map_err(|_| io::Error::from(io::ErrorKind::Other))?;
    loop {
        let (mut inbound, peer) = tokio::select! {
            _ = listener.shutdown.cancelled() => return Ok(()),
            accepted = socket.accept() => accepted?,
        };
        let routes_arc = listener.routes.clone();
        let guard = listener.guard.clone();
        let pool = pool.clone();
        let rate_clock = listener.rate_clock.clone();
        let filing = listener.filing.clone();
        tokio::spawn(async move {
            #[cfg(feature = "http")]
            {
                let ctx = Arc::new(crate::http_l7::SharedRequestContext {
                    routes: routes_arc,
                    guard,
                    pool,
                    rate_clock,
                    filing,
                });
                let _ = crate::http_l7::serve_inbound_hyper(inbound, peer, ctx, "http").await;
                return;
            }
            #[cfg(not(feature = "http"))]
            {
                let routes = routes_arc.load();
                let mut buffer = vec![0_u8; 16 * 1024];
                let read = match tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    read_http_prefix(&mut inbound, &mut buffer),
                )
                .await
                {
                    Ok(Ok(read)) => read,
                    _ => return,
                };
                let Some(host) = parse_host(&buffer[..read]) else {
                    return;
                };
                let Some(route) = routes.get(&host).cloned() else {
                    let _ = write_http_error(&mut inbound, 404, "domain not found").await;
                    return;
                };
                let ctx = ConnCtx {
                    peer: peer.ip(),
                    trust: SourceTrust::Tcp,
                    trusted_proxy: false,
                    tunnel_id: Some(route.tunnel_id),
                };
                let verdict = guard.check_conn(&ctx);
                if guard.record_if_denied(&verdict, peer.ip(), Some(route.tunnel_id)) {
                    route.traffic.record_reject_guard();
                    if let Ok(std_stream) = inbound.into_std() {
                        let socket = socket2::Socket::from(std_stream);
                        let _ = socket.set_linger(Some(std::time::Duration::ZERO));
                    }
                    return;
                }
                if !route.try_acquire() {
                    let _ = write_http_error(&mut inbound, 429, "too many connections").await;
                    return;
                }
                let _slot = ConnSlot(Arc::new(route.clone()));
                let header = StreamHeader {
                    tunnel_id: route.tunnel_id,
                    src_addr: peer,
                };
                let Ok(mut carrier) = route.client_session.open_stream(header).await else {
                    return;
                };
                use tokio::io::AsyncWriteExt;
                if carrier.write_all(&buffer[..read]).await.is_err() {
                    return;
                }
                let relay = tz_net::pump::pump(
                    inbound,
                    carrier,
                    pool,
                    pump_options(
                        &rate_clock,
                        route.speed_limit_mbps,
                        route.traffic.meter.clone(),
                    ),
                );
                tokio::select! {
                    _ = relay => {}
                    _ = route.traffic.connections_closed() => {}
                }
            }
        });
    }
}

async fn read_http_prefix(
    stream: &mut tokio::net::TcpStream,
    buffer: &mut [u8],
) -> io::Result<usize> {
    use tokio::io::AsyncReadExt;
    let mut total = 0_usize;
    while total < buffer.len() {
        let read = stream.read(&mut buffer[total..]).await?;
        if read == 0 {
            break;
        }
        total += read;
        if buffer[..total].windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    Ok(total)
}

pub fn parse_host(prefix: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(prefix).ok()?;
    for line in text.lines() {
        let lower = line.to_ascii_lowercase();
        if let Some(value) = lower.strip_prefix("host:") {
            let host = value.trim().split(':').next()?.trim().to_ascii_lowercase();
            if !host.is_empty() {
                return Some(host);
            }
        }
    }
    None
}

async fn write_http_error(
    stream: &mut tokio::net::TcpStream,
    status: u16,
    message: &str,
) -> io::Result<()> {
    use tokio::io::AsyncWriteExt;
    let body = format!("{message}\n");
    let response = format!(
        "HTTP/1.1 {status} \r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await
}

pub fn build_route_table(
    routes: &[tz_proto::HttpDomainRoute],
    sessions_by_tunnel: &DashMap<Uuid, Arc<dyn CarrierSession>>,
    traffic_by_tunnel: &DashMap<Uuid, Arc<TunnelTrafficAccounting>>,
) -> HashMap<String, SharedHttpRoute> {
    let mut table = HashMap::new();
    for route in routes {
        let Some(session) = sessions_by_tunnel
            .get(&route.tunnel_id)
            .map(|entry| entry.clone())
            .filter(|session| !session.is_closed())
        else {
            continue;
        };
        let traffic = crate::traffic::ensure_tunnel_traffic(traffic_by_tunnel, route.tunnel_id);
        table.insert(
            route.domain.to_ascii_lowercase(),
            SharedHttpRoute {
                tunnel_id: route.tunnel_id,
                client_session: session,
                speed_limit_mbps: route.speed_limit_mbps,
                max_conns: route.max_conns,
                traffic,
            },
        );
    }
    table
}
