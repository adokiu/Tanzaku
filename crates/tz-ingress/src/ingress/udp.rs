use portable_atomic::AtomicU64;
use std::{
    net::{IpAddr, SocketAddr},
    sync::{
        atomic::Ordering,
        Arc,
    },
};

use bytes::Bytes;
use dashmap::DashMap;
use tz_carrier::types::{flow_id_from_peer, FlowId};
use tz_guard::types::{PktCtx, SourceTrust};
use tz_net::wheel::TimingWheel;

use crate::{
    registry::{BoxFuture, IngressError, IngressFactory},
    types::TunnelEndpoint,
};

inventory::submit! {
    IngressFactory {
        kind: "udp",
        serve: serve,
    }
}

fn serve(endpoint: TunnelEndpoint) -> BoxFuture<'static, Result<(), IngressError>> {
    Box::pin(async move {
        let socket = match crate::bind::udp_socket(endpoint.bind_addr.as_str(), endpoint.port)
            .await
        {
            Ok(socket) => {
                crate::udp_src::enable_local_ip_info(&socket);
                tz_net::sockopt::set_udp_buffers(&socket, 4 * 1024 * 1024);
                Arc::new(socket)
            }
            Err(err) => {
                if let Ok(mut slot) = endpoint.bind_failed.lock() {
                    *slot = Some((endpoint.port, err.to_string()));
                }
                return Err(IngressError::Io(err));
            }
        };
        if let Some(on_bound) = &endpoint.on_ingress_bound {
            on_bound();
        }
        let peers: Arc<DashMap<FlowId, Peer>> = Arc::new(DashMap::new());
        let clock = Arc::new(AtomicU64::new(0));
        let shutdown = endpoint.shutdown.clone();
        let uplink = run_uplink(
            socket.clone(),
            endpoint.clone(),
            peers.clone(),
            clock.clone(),
            shutdown.clone(),
        );
        let downlink = run_downlink(socket, endpoint, peers, clock, shutdown);
        tokio::select! {
            result = uplink => result,
            result = downlink => result,
        }
    })
}

/// 无双向报文超过该秒数的 flow 被回收。
const FLOW_IDLE_SECS: u64 = 60;

struct Peer {
    addr: SocketAddr,
    /// 访客发往的本机 IP，回包以此为源地址。
    local_ip: Option<IpAddr>,
    last_active: AtomicU64,
}

async fn run_uplink(
    socket: Arc<tokio::net::UdpSocket>,
    endpoint: TunnelEndpoint,
    peers: Arc<DashMap<FlowId, Peer>>,
    clock: Arc<AtomicU64>,
    shutdown: crate::shutdown::IngressShutdown,
) -> Result<(), IngressError> {
    let mut buffer = vec![0_u8; 65507];
    let mut wheel = TimingWheel::<FlowId, ()>::new(64, 65_536).map_err(|_| IngressError::Stopped)?;
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(1));
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => return Ok(()),
            _ = ticker.tick() => {
                let tick = clock.fetch_add(1, Ordering::Relaxed) + 1;
                for (flow, _) in wheel.advance(tick) {
                    let idle_until = peers
                        .get(&flow)
                        .map(|peer| peer.last_active.load(Ordering::Relaxed) + FLOW_IDLE_SECS);
                    match idle_until {
                        Some(until) if until > tick => {
                            let _ = wheel.schedule(flow, until, ());
                        }
                        Some(_) => {
                            peers.remove(&flow);
                        }
                        None => {}
                    }
                }
            }
            received = crate::udp_src::recv_with_local_ip(&socket, &mut buffer) => {
                let (size, peer, local_ip) = received?;
                if size == 0 {
                    continue;
                }
                let flow = flow_id_from_peer(peer);
                let tick = clock.load(Ordering::Relaxed);
                let new_flow = match peers.get(&flow) {
                    Some(existing) => {
                        existing.last_active.store(tick, Ordering::Relaxed);
                        false
                    }
                    None => true,
                };
                let ctx = PktCtx {
                    peer: peer.ip(),
                    trust: SourceTrust::Udp,
                    bytes: size,
                    new_flow,
                    tunnel_id: Some(endpoint.tunnel_id),
                };
                let verdict = endpoint.guard.check_udp(&ctx);
                if endpoint
                    .guard
                    .record_if_denied(&verdict, peer.ip(), Some(endpoint.tunnel_id))
                {
                    endpoint.traffic.record_reject_guard();
                    continue;
                }
                if new_flow {
                    if endpoint.max_conns > 0 && peers.len() as i32 >= endpoint.max_conns {
                        endpoint.traffic.record_reject_quota();
                        continue;
                    }
                    if !endpoint.allow_new_conn().await {
                        continue;
                    }
                    peers.insert(
                        flow,
                        Peer {
                            addr: peer,
                            local_ip,
                            last_active: AtomicU64::new(tick),
                        },
                    );
                    let _ = wheel.schedule(flow, tick + FLOW_IDLE_SECS, ());
                }
                let payload = Bytes::copy_from_slice(&buffer[..size]);
                endpoint.traffic.record_datagram_in(size);
                let _ = endpoint
                    .carrier_session()
                    .send_datagram_wait(flow, payload)
                    .await;
            }
        }
    }
}

async fn run_downlink(
    socket: Arc<tokio::net::UdpSocket>,
    endpoint: TunnelEndpoint,
    peers: Arc<DashMap<FlowId, Peer>>,
    clock: Arc<AtomicU64>,
    shutdown: crate::shutdown::IngressShutdown,
) -> Result<(), IngressError> {
    loop {
        let session = endpoint.carrier_session();
        tokio::select! {
            _ = shutdown.cancelled() => return Ok(()),
            received = session.recv_datagram() => {
                let Ok((flow, payload)) = received else {
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    continue;
                };
                let Some((addr, local_ip)) = peers.get(&flow).map(|peer| {
                    peer.last_active.store(clock.load(Ordering::Relaxed), Ordering::Relaxed);
                    (peer.addr, peer.local_ip)
                }) else {
                    continue;
                };
                // 下行也计入该访客 IP 的 PPS（iperf -R 等「对端发送」场景此前完全不限速）。
                let ctx = PktCtx {
                    peer: addr.ip(),
                    trust: SourceTrust::Udp,
                    bytes: payload.len(),
                    new_flow: false,
                    tunnel_id: Some(endpoint.tunnel_id),
                };
                let verdict = endpoint.guard.check_udp(&ctx);
                if endpoint
                    .guard
                    .record_if_denied(&verdict, addr.ip(), Some(endpoint.tunnel_id))
                {
                    endpoint.traffic.record_reject_guard();
                    continue;
                }
                endpoint.traffic.record_datagram_out(payload.len());
                let _ = crate::udp_src::send_from_local_ip(&socket, &payload, addr, local_ip).await;
            }
        }
    }
}
