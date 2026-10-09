use dashmap::DashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tz_agent::{emit_client_traffic_report, emit_tunnel_state, AgentHandle, ControlClient};
use tz_proto::ClientTrafficReport;
use tz_carrier::registry::find as find_carrier;
use tz_proto::{ClientTunnelAssign, TunnelStateReport};

use crate::relay::relay_connection;
use crate::traffic::ClientTrafficRegistry;
use crate::udp_relay::run_udp_relay;
use crate::upstream::connect_upstream;

struct TunnelWorker {
    stop: Arc<AtomicBool>,
    session: Arc<tokio::sync::Mutex<Option<Arc<dyn tz_carrier::registry::CarrierSession>>>>,
    /// node 换证 / carrier 变更时需重启 worker
    assign_key: String,
}

fn tunnel_assign_key(tunnel: &ClientTunnelAssign) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        tunnel.carrier,
        tunnel.node_public_host,
        tunnel.carrier_port,
        tunnel.node_cert_fingerprint.as_deref().unwrap_or(""),
        tunnel.carrier_secret,
    )
}

pub struct AgentRuntime {
    handle: AgentHandle,
    workers: Arc<DashMap<uuid::Uuid, TunnelWorker>>,
    traffic: Arc<ClientTrafficRegistry>,
}

impl AgentRuntime {
    pub fn bootstrap(board_url: String, token: String) -> Self {
        Self {
            handle: ControlClient::agent(board_url, token)
                .with_capabilities(tz_proto::AgentCapabilities {
                    carriers: tz_carrier::registered_kinds()
                        .into_iter()
                        .map(str::to_owned)
                        .collect(),
                    ingress: Vec::new(),
                    guards: Vec::new(),
                })
                .start_agent(),
            workers: Arc::new(DashMap::new()),
            traffic: Arc::new(ClientTrafficRegistry::default()),
        }
    }

    pub async fn run(self) -> ! {
        let traffic_handle = self.handle.clone();
        let traffic_registry = self.traffic.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(1));
            loop {
                ticker.tick().await;
                let cfg = traffic_handle.config.load();
                if cfg.client_id.is_nil() {
                    continue;
                }
                let (bytes_in_delta, bytes_out_delta) = traffic_registry.take_interval_delta();
                let (bytes_in, bytes_out) = traffic_registry.totals();
                emit_client_traffic_report(
                    &traffic_handle,
                    ClientTrafficReport {
                        bytes_in,
                        bytes_out,
                        bytes_in_delta,
                        bytes_out_delta,
                    },
                )
                .await;
            }
        });

        let mut wait_hint = std::time::Instant::now()
            .checked_sub(std::time::Duration::from_secs(10))
            .unwrap_or_else(std::time::Instant::now);
        loop {
            let config = self.handle.config.load();
            if config.client_id.is_nil() {
                if wait_hint.elapsed() >= std::time::Duration::from_secs(10) {
                    tracing::info!(
                        "waiting for board ClientConfig (check --board and --token)"
                    );
                    wait_hint = std::time::Instant::now();
                }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                continue;
            }
            wait_hint = std::time::Instant::now()
                .checked_sub(std::time::Duration::from_secs(10))
                .unwrap_or_else(std::time::Instant::now);
            let active: std::collections::HashSet<_> =
                config.tunnels.iter().map(|tunnel| tunnel.tunnel_id).collect();
            self.traffic.retain_active(&active);
            for tunnel in &config.tunnels {
                self.traffic.ensure(tunnel.tunnel_id);
                let key = tunnel_assign_key(tunnel);
                if let Some(existing) = self.workers.get(&tunnel.tunnel_id) {
                    if existing.assign_key == key && !existing.stop.load(Ordering::Acquire) {
                        continue;
                    }
                    if existing.assign_key == key {
                        tracing::info!(
                            tunnel_id = %tunnel.tunnel_id,
                            "restarting tunnel worker after session ended"
                        );
                    } else {
                        tracing::info!(
                            tunnel_id = %tunnel.tunnel_id,
                            "tunnel assignment changed (node cert/carrier/host); restarting worker"
                        );
                    }
                    drop(existing);
                    if let Some((_, worker)) = self.workers.remove(&tunnel.tunnel_id) {
                        worker.stop.store(true, Ordering::Release);
                        let session = worker.session.clone();
                        tokio::spawn(async move {
                            if let Some(session) = session.lock().await.take() {
                                session.close("tunnel assignment updated");
                            }
                        });
                    }
                }
                let stop = Arc::new(AtomicBool::new(false));
                let session_slot = Arc::new(tokio::sync::Mutex::new(None));
                tracing::info!(
                    tunnel_id = %tunnel.tunnel_id,
                    protocol = %tunnel.protocol,
                    carrier = %tunnel.carrier,
                    node = %tunnel.node_public_host,
                    carrier_port = tunnel.carrier_port,
                    "starting client tunnel worker"
                );
                self.workers.insert(
                    tunnel.tunnel_id,
                    TunnelWorker {
                        stop: stop.clone(),
                        session: session_slot.clone(),
                        assign_key: key,
                    },
                );
                let tunnel = tunnel.clone();
                let tunnel_id = tunnel.tunnel_id;
                let workers = self.workers.clone();
                let agent = self.handle.clone();
                let traffic = self.traffic.clone();
                tokio::spawn(async move {
                    let own_stop = stop.clone();
                    if let Err(error) = run_tunnel_worker(
                        tunnel,
                        agent,
                        stop,
                        session_slot,
                        traffic,
                    )
                    .await
                    {
                        tracing::warn!(?error, tunnel_id = %tunnel_id, "client tunnel worker exited");
                    }
                    // 只摘掉自己的条目；旧 worker 晚退出时不能删掉已接替的新 worker。
                    workers.remove_if(&tunnel_id, |_, worker| Arc::ptr_eq(&worker.stop, &own_stop));
                });
            }
            let stale: Vec<uuid::Uuid> = self
                .workers
                .iter()
                .filter(|entry| !active.contains(entry.key()))
                .map(|entry| *entry.key())
                .collect();
            for tunnel_id in stale {
                if let Some((_, worker)) = self.workers.remove(&tunnel_id) {
                    tracing::info!(%tunnel_id, "stopping client tunnel worker (removed from config)");
                    worker.stop.store(true, Ordering::Release);
                    if let Some(session) = worker.session.lock().await.take() {
                        session.close("tunnel disabled by control plane");
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    }
}

fn latest_tunnel_assign(agent: &AgentHandle, tunnel_id: uuid::Uuid, fallback: &ClientTunnelAssign) -> ClientTunnelAssign {
    agent
        .config
        .load()
        .tunnels
        .iter()
        .find(|entry| entry.tunnel_id == tunnel_id)
        .cloned()
        .unwrap_or_else(|| fallback.clone())
}

async fn run_tunnel_worker(
    tunnel: ClientTunnelAssign,
    agent: AgentHandle,
    stop: Arc<AtomicBool>,
    session_slot: Arc<tokio::sync::Mutex<Option<Arc<dyn tz_carrier::registry::CarrierSession>>>>,
    traffic: Arc<ClientTrafficRegistry>,
) -> anyhow::Result<()> {
    let tunnel_id = tunnel.tunnel_id;
    let tunnel_meter = traffic.ensure(tunnel_id).meter.clone();
    let mut reconnect_backoff = std::time::Duration::from_secs(1);

    while !stop.load(Ordering::Acquire) {
        if !agent
            .config
            .load()
            .tunnels
            .iter()
            .any(|entry| entry.tunnel_id == tunnel_id)
        {
            return Ok(());
        }

        let session = match connect_carrier_session(&agent, tunnel_id, &tunnel, &stop).await {
            Ok(session) => session,
            Err(ConnectCarrierOutcome::Stopped) => return Ok(()),
            Err(ConnectCarrierOutcome::Unavailable(err)) => return Err(err),
        };

        reconnect_backoff = std::time::Duration::from_secs(1);
        let tunnel = latest_tunnel_assign(&agent, tunnel_id, &tunnel);
        let server_addr = format!("{}:{}", tunnel.node_public_host, tunnel.carrier_port);
        *session_slot.lock().await = Some(session.clone());
        tracing::info!(
            tunnel_id = %tunnel.tunnel_id,
            server = %server_addr,
            carrier = %tunnel.carrier,
            "carrier session established to node"
        );
        report_tunnel_state(&agent, &tunnel, false, false, true, None).await;

        let session_ended = if tunnel.protocol == "udp" {
            if stop.load(Ordering::Acquire) {
                false
            } else {
                run_udp_relay_cancellable(
                    session.clone(),
                    tunnel.clone(),
                    agent.clone(),
                    stop.clone(),
                    tunnel_meter.clone(),
                )
                .await
                .is_ok() && !stop.load(Ordering::Acquire)
            }
        } else {
            run_stream_relay_loop(
                session.clone(),
                &agent,
                tunnel_id,
                &tunnel,
                &stop,
                tunnel_meter.clone(),
            )
            .await
        };

        *session_slot.lock().await = None;
        session.close("carrier session cycle ended");

        if stop.load(Ordering::Acquire) {
            break;
        }
        // 无论正常结束还是异常断开，都必须退避后再连，避免 Endpoint/会话抖动造成重连风暴。
        report_tunnel_state(
            &agent,
            &tunnel,
            false,
            false,
            true,
            Some(
                if session_ended {
                    "carrier session ended; reconnecting"
                } else {
                    "carrier session interrupted; reconnecting"
                }
                .into(),
            ),
        )
        .await;
        tracing::info!(
            tunnel_id = %tunnel.tunnel_id,
            retry_secs = reconnect_backoff.as_secs(),
            session_ended,
            "carrier session lost; will reconnect"
        );
        tokio::select! {
            _ = wait_for_stop(&stop) => break,
            _ = tokio::time::sleep(reconnect_backoff) => {}
        }
        reconnect_backoff = (reconnect_backoff * 2).min(std::time::Duration::from_secs(30));
    }
    Ok(())
}

enum ConnectCarrierOutcome {
    Stopped,
    Unavailable(anyhow::Error),
}

async fn connect_carrier_session(
    agent: &AgentHandle,
    tunnel_id: uuid::Uuid,
    fallback: &ClientTunnelAssign,
    stop: &Arc<AtomicBool>,
) -> Result<Arc<dyn tz_carrier::registry::CarrierSession>, ConnectCarrierOutcome> {
    let mut backoff = std::time::Duration::from_secs(1);
    loop {
        if stop.load(Ordering::Acquire) {
            return Err(ConnectCarrierOutcome::Stopped);
        }
        let tunnel = latest_tunnel_assign(agent, tunnel_id, fallback);
        let Some(factory) = find_carrier(&tunnel.carrier) else {
            return Err(ConnectCarrierOutcome::Unavailable(anyhow::anyhow!(
                "carrier {} is unavailable",
                tunnel.carrier
            )));
        };
        let server_addr = format!("{}:{}", tunnel.node_public_host, tunnel.carrier_port)
            .parse()
            .map_err(|_| ConnectCarrierOutcome::Unavailable(anyhow::anyhow!("invalid server address")))?;
        let agent_config = agent.config.load();
        let client_id = agent_config.client_id;
        if tunnel.carrier == "tcp" {
            let connect_config = tz_carrier::types::ConnectConfig {
                server_addr,
                tunnel_id,
                client_id,
                carrier_secret: tunnel.carrier_secret.clone(),
            };
            match (factory.connect)(connect_config).await {
                Ok(session) => return Ok(session),
                Err(err) => {
                    let message = err.to_string();
                    report_tunnel_state(
                        agent,
                        &tunnel,
                        false,
                        false,
                        false,
                        Some(message.clone()),
                    )
                    .await;
                    tracing::warn!(
                        tunnel_id = %tunnel.tunnel_id,
                        server = %server_addr,
                        retry_secs = backoff.as_secs(),
                        error = %message,
                        "tcp carrier connect failed; retrying"
                    );
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(std::time::Duration::from_secs(30));
                    continue;
                }
            }
        }
        let connect_config = tz_carrier::types::ConnectConfig {
            server_addr,
            tunnel_id,
            client_id,
            carrier_secret: String::new(),
        };
        match (factory.connect)(connect_config).await {
            Ok(session) => return Ok(session),
            Err(err) => {
                let message = err.to_string();
                report_tunnel_state(
                    agent,
                    &tunnel,
                    false,
                    false,
                    false,
                    Some(message.clone()),
                )
                .await;
                tracing::warn!(
                    tunnel_id = %tunnel.tunnel_id,
                    server = %server_addr,
                    retry_secs = backoff.as_secs(),
                    error = %message,
                    "carrier connect failed; retrying"
                );
                tokio::select! {
                    _ = wait_for_stop(stop) => return Err(ConnectCarrierOutcome::Stopped),
                    _ = tokio::time::sleep(backoff) => {}
                }
                backoff = (backoff * 2).min(std::time::Duration::from_secs(30));
            }
        }
    }
}

async fn run_stream_relay_loop(
    session: Arc<dyn tz_carrier::registry::CarrierSession>,
    agent: &AgentHandle,
    tunnel_id: uuid::Uuid,
    tunnel: &ClientTunnelAssign,
    stop: &Arc<AtomicBool>,
    meter: tz_net::meter::TrafficMeter,
) -> bool {
    let mut end_to_end_active = false;
    loop {
        if stop.load(Ordering::Acquire) {
            session.close("tunnel disabled by control plane");
            return false;
        }
        let accepted = tokio::select! {
            biased;
            _ = wait_for_stop(stop) => return false,
            result = session.accept_stream() => result,
        };
        let Ok((header, carrier)) = accepted else {
            return true;
        };
        if !end_to_end_active {
            end_to_end_active = true;
            report_tunnel_state(agent, tunnel, true, false, true, None).await;
        }
        let tunnel = latest_tunnel_assign(agent, tunnel_id, tunnel);
        let agent = agent.clone();
        let meter = meter.clone();
        tokio::spawn(async move {
            let upstream = match connect_upstream(&tunnel).await {
                Ok(upstream) => upstream,
                Err(error) => {
                    let backend_tls_error = error.chain().any(|cause| cause.is::<rustls::Error>());
                    tracing::warn!(
                        %tunnel_id,
                        target = %crate::upstream::upstream_target(&tunnel),
                        error = %format!("{error:#}"),
                        backend_tls_error,
                        "upstream connection failed"
                    );
                    report_tunnel_state(
                        &agent,
                        &tunnel,
                        false,
                        backend_tls_error,
                        true,
                        Some(format!("{error:#}")),
                    )
                    .await;
                    let _ = header;
                    return;
                }
            };
            if let Err(error) = relay_connection(carrier, &tunnel, upstream, meter).await {
                tracing::warn!(%tunnel_id, error = %format!("{error:#}"), "upstream relay failed");
            }
            let _ = header;
        });
    }
}

async fn wait_for_stop(stop: &AtomicBool) {
    while !stop.load(Ordering::Acquire) {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

async fn run_udp_relay_cancellable(
    session: Arc<dyn tz_carrier::registry::CarrierSession>,
    tunnel: ClientTunnelAssign,
    agent: AgentHandle,
    stop: Arc<AtomicBool>,
    meter: tz_net::meter::TrafficMeter,
) -> anyhow::Result<()> {
    let session_for_stop = session.clone();
    let agent_for_stop = agent.clone();
    let tunnel_for_stop = tunnel.clone();
    let result = tokio::select! {
        _ = wait_for_stop(&stop) => {
            session_for_stop.close("tunnel disabled by control plane");
            Ok(())
        }
        result = run_udp_relay(session, tunnel, agent, meter) => result,
    };
    if !stop.load(Ordering::Acquire) {
        report_tunnel_state(
            &agent_for_stop,
            &tunnel_for_stop,
            false,
            false,
            false,
            Some("UDP carrier session ended".into()),
        )
        .await;
    }
    result
}

pub(crate) async fn report_tunnel_state(
    agent: &AgentHandle,
    tunnel: &ClientTunnelAssign,
    reachable: bool,
    backend_tls_error: bool,
    carrier_connected: bool,
    message: Option<String>,
) {
    emit_tunnel_state(
        agent,
        TunnelStateReport {
            tunnel_id: tunnel.tunnel_id,
            reachable,
            backend_tls_error,
            carrier_connected,
            message,
        },
    )
    .await;
}
