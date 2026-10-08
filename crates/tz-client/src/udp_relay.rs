use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use bytes::Bytes;
use dashmap::DashMap;
use tz_carrier::{
    registry::CarrierSession,
    types::{DatagramDropped, FlowId, OpenError},
};
use tz_agent::AgentHandle;
use tz_net::meter::TrafficMeter;
use tz_proto::ClientTunnelAssign;

/// 双向都无报文超过该时长的 flow 回收 socket 与回包任务。
const FLOW_IDLE: Duration = Duration::from_secs(60);
const FLOW_IDLE_CHECK: Duration = Duration::from_secs(15);
const MAX_UDP_PAYLOAD: usize = 65_507;
const FLOW_SOCKET_BUFFER: usize = 1024 * 1024;

struct Flow {
    socket: Arc<tokio::net::UdpSocket>,
    /// 相对 `Clock::started` 的秒数。
    last_active: AtomicU64,
}

#[derive(Clone, Copy)]
struct Clock {
    started: Instant,
}

impl Clock {
    fn now(self) -> u64 {
        self.started.elapsed().as_secs()
    }
}

type Flows = Arc<DashMap<FlowId, Arc<Flow>>>;

pub async fn run_udp_relay(
    session: Arc<dyn CarrierSession>,
    tunnel: ClientTunnelAssign,
    agent: AgentHandle,
    meter: TrafficMeter,
) -> anyhow::Result<()> {
    let target_host = tunnel
        .target_host
        .clone()
        .unwrap_or_else(|| "127.0.0.1".into());
    let target_port = tunnel
        .target_port
        .and_then(|value| u16::try_from(value).ok())
        .unwrap_or(53);
    let target: SocketAddr = format!("{target_host}:{target_port}")
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid UDP target address"))?;
    let flows: Flows = Arc::new(DashMap::new());
    let clock = Clock {
        started: Instant::now(),
    };
    let mut end_to_end_active = false;
    loop {
        let (flow, payload) = session.recv_datagram().await.map_err(map_carrier_err)?;
        meter.left_to_right.add(payload.len());
        if !end_to_end_active {
            end_to_end_active = true;
            crate::runtime::report_tunnel_state(&agent, &tunnel, true, false, true, None).await;
        }
        if let Err(error) = forward_datagram(flow, payload, target, &session, &flows, clock, &meter).await
        {
            tracing::debug!(?error, tunnel_id = %tunnel.tunnel_id, "udp relay datagram failed");
        }
    }
}

async fn forward_datagram(
    flow_id: FlowId,
    payload: Bytes,
    target: SocketAddr,
    session: &Arc<dyn CarrierSession>,
    flows: &Flows,
    clock: Clock,
    meter: &TrafficMeter,
) -> anyhow::Result<()> {
    let existing = flows.get(&flow_id).map(|entry| entry.clone());
    let flow = match existing {
        Some(flow) => flow,
        None => open_flow(flow_id, target, session, flows, clock, meter).await?,
    };
    flow.last_active.store(clock.now(), Ordering::Relaxed);
    match flow.socket.try_send_to(&payload, target) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
            flow.socket.send_to(&payload, target).await?;
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

async fn open_flow(
    flow_id: FlowId,
    target: SocketAddr,
    session: &Arc<dyn CarrierSession>,
    flows: &Flows,
    clock: Clock,
    meter: &TrafficMeter,
) -> anyhow::Result<Arc<Flow>> {
    // 不 connect：多 IP 的目标（网关 / 路由器上的服务）回包源地址可能不是 target，
    // 已 connect 的 socket 会被内核静默丢弃这类回包。每个 flow 独占临时端口，收到的都属于本 flow。
    let bind_addr = if target.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" };
    let socket = tokio::net::UdpSocket::bind(bind_addr).await?;
    tz_net::sockopt::set_udp_buffers(&socket, FLOW_SOCKET_BUFFER);
    let flow = Arc::new(Flow {
        socket: Arc::new(socket),
        last_active: AtomicU64::new(clock.now()),
    });
    flows.insert(flow_id, flow.clone());

    let session = session.clone();
    let flows = flows.clone();
    let meter = meter.clone();
    let reply = flow.clone();
    tokio::spawn(async move {
        let mut buffer = vec![0_u8; MAX_UDP_PAYLOAD];
        loop {
            let read = match tokio::time::timeout(FLOW_IDLE_CHECK, reply.socket.recv_from(&mut buffer)).await {
                Ok(Ok((read, _source))) => read,
                Ok(Err(_)) => break,
                Err(_) => {
                    let idle = clock.now().saturating_sub(reply.last_active.load(Ordering::Relaxed));
                    if idle >= FLOW_IDLE.as_secs() {
                        break;
                    }
                    continue;
                }
            };
            if read == 0 {
                continue;
            }
            reply.last_active.store(clock.now(), Ordering::Relaxed);
            meter.right_to_left.add(read);
            if let Err(DatagramDropped::Closed) = session
                .send_datagram_wait(flow_id, Bytes::copy_from_slice(&buffer[..read]))
                .await
            {
                break;
            }
        }
        flows.remove_if(&flow_id, |_, current| Arc::ptr_eq(current, &reply));
    });
    Ok(flow)
}

fn map_carrier_err(error: OpenError) -> anyhow::Error {
    anyhow::anyhow!(error.to_string())
}
