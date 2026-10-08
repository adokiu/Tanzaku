use arc_swap::ArcSwap;
use dashmap::DashMap;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};
use tz_agent::{
    emit_host_metrics_report, emit_port_bind_failed, emit_tunnel_failed,
    emit_tunnel_ready, ControlClient, HostMetricsSampler, NodeHandle,
};
use crate::stats_outbox::StatsOutbox;
use crate::stats_period::{merge_traffic_samples, wait_closed_period};
use tz_carrier::registry::{find as find_carrier, CarrierSession};
use tz_guard::pipeline::GuardPipeline;
use tz_ingress::{
    limits::new_conn_bucket,
    registry::find as find_ingress,
    shared_http::{build_route_table, serve as serve_shared_http, SharedHttpListener},
    shared_https::{serve as serve_shared_https, SharedHttpsListener, SniCertResolver},
    shutdown::{IngressShutdown, IngressShutdownHandle},
    traffic::{collect_interval_samples, ensure_tunnel_traffic, TunnelTrafficAccounting},
    types::{DedicatedHttpHosts, TunnelEndpoint},
};
use tz_net::rate::{RateClock, RateDriver};
use tz_proto::StatsReport;
use url::Url;
use uuid::Uuid;

/// 每个统计周期最多发出的 StatsReport 条数（断连积压时逐步补发）。
const STATS_SEND_BURST: usize = 32;
/// 已发出但迟迟无 Ack 时，从已提交水位重新补发。
const STATS_RESEND_AFTER: std::time::Duration = std::time::Duration::from_secs(10);

struct ManagedIngress {
    revision: i64,
    /// 每次启动唯一；任务退出时只清理自己的登记，避免误删 revision 更新后的新监听。
    generation: u64,
    stop: IngressShutdownHandle,
    http_hosts: Arc<ArcSwap<DedicatedHttpHosts>>,
}

static INGRESS_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

struct SharedListener {
    stop: IngressShutdownHandle,
    bind_addr: String,
    port: u16,
}

pub struct NodeRuntime {
    handle: NodeHandle,
    /// 每条隧道独立 carrier 会话（client 在 bind 前导里声明 tunnel_id）。
    tunnel_sessions: Arc<DashMap<Uuid, Arc<dyn CarrierSession>>>,
    guard: Arc<GuardPipeline>,
    ingress_tasks: Arc<DashMap<Uuid, ManagedIngress>>,
    shared_http: Arc<Mutex<Option<SharedListener>>>,
    shared_https: Arc<Mutex<Option<SharedListener>>>,
    shared_routes: Arc<ArcSwap<HashMap<String, tz_ingress::shared_http::SharedHttpRoute>>>,
    sni_resolver: Arc<SniCertResolver>,
    filing: Arc<ArcSwap<tz_ingress::filing::FilingGate>>,
    rate_clock: RateClock,
    stats_seq: Arc<AtomicU64>,
    tunnel_traffic: Arc<DashMap<Uuid, Arc<TunnelTrafficAccounting>>>,
    stats_flush_samples: Arc<tokio::sync::Mutex<Vec<tz_proto::TunnelTrafficSample>>>,
}

fn compiled_capabilities() -> tz_proto::AgentCapabilities {
    tz_proto::AgentCapabilities {
        carriers: tz_carrier::registered_kinds()
            .into_iter()
            .map(str::to_owned)
            .collect(),
        ingress: tz_ingress::registered_kinds()
            .into_iter()
            .map(str::to_owned)
            .collect(),
        guards: tz_guard::registered_modules()
            .into_iter()
            .map(str::to_owned)
            .collect(),
    }
}

fn session_owns_tunnel(session: &dyn CarrierSession, tunnel: &tz_proto::TunnelSpec) -> bool {
    match (session.peer().client_id, tunnel.client_id) {
        (Some(peer), Some(owner)) => peer == owner,
        _ => true,
    }
}

impl NodeRuntime {
    pub fn bootstrap(board_url: String, token: String) -> Arc<Self> {
        let handle = ControlClient::node(board_url, token)
            .with_capabilities(compiled_capabilities())
            .start_node();
        let (rate_clock, rate_driver) =
            RateDriver::new(std::time::Duration::from_millis(100)).expect("rate clock");
        tokio::spawn(rate_driver.run());
        let guard = Arc::new(GuardPipeline::from_policy(&serde_json::json!({})));
        crate::guard_events::install_guard_event_hook(handle.clone(), &guard);
        let filing = Arc::new(ArcSwap::from_pointee(tz_ingress::filing::FilingGate::default()));
        Arc::new(Self {
            handle,
            tunnel_sessions: Arc::new(DashMap::new()),
            guard,
            ingress_tasks: Arc::new(DashMap::new()),
            shared_http: Arc::new(Mutex::new(None)),
            shared_https: Arc::new(Mutex::new(None)),
            shared_routes: Arc::new(ArcSwap::from_pointee(HashMap::new())),
            sni_resolver: Arc::new(SniCertResolver::new(filing.clone())),
            filing,
            rate_clock,
            stats_seq: Arc::new(AtomicU64::new(0)),
            tunnel_traffic: Arc::new(DashMap::new()),
            stats_flush_samples: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        })
    }

    pub fn handle(&self) -> &NodeHandle {
        &self.handle
    }

    pub fn carrier_listen_config(&self, carrier: &str) -> Option<tz_carrier::types::ListenConfig> {
        let config = self.handle.config.load();
        let entry = config.carrier_ports.get(carrier)?;
        if entry.get("enabled").and_then(|value| value.as_bool()) == Some(false) {
            return None;
        }
        let port = entry
            .get("port")
            .and_then(|value| value.as_u64())
            .and_then(|value| u16::try_from(value).ok())
            .filter(|port| *port > 0)?;
        Some(tz_carrier::types::ListenConfig {
            bind_addr: config.bind_addr.clone(),
            port,
            node_config: self.handle.config.clone(),
        })
    }

    fn resolve_client_session(
        &self,
        tunnel: &tz_proto::TunnelSpec,
    ) -> Option<Arc<dyn CarrierSession>> {
        let session = self.tunnel_sessions.get(&tunnel.tunnel_id)?.clone();
        if session.is_closed() || !session_owns_tunnel(session.as_ref(), tunnel) {
            return None;
        }
        Some(session)
    }

    pub async fn run(self: Arc<Self>) -> ! {
        let host_metrics = self.clone();
        tokio::spawn(async move {
            let mut sampler = HostMetricsSampler::new();
            loop {
                let interval_secs = host_metrics
                    .handle
                    .config
                    .load()
                    .host_metrics_interval_secs
                    .max(1) as u64;
                let _ = wait_closed_period(interval_secs).await;
                let report = sampler.sample();
                emit_host_metrics_report(host_metrics.handle(), report).await;
            }
        });

        let stats = self.clone();
        tokio::spawn(async move {
            let mut outbox = StatsOutbox::new();
            let mut last_closed_period_end: u64 = 0;
            let mut sent_upto: u64 = 0;
            let mut last_committed: u64 = 0;
            let mut last_ack_progress = std::time::Instant::now();
            loop {
                let interval_secs = stats
                    .handle
                    .config
                    .load()
                    .host_metrics_interval_secs
                    .max(1) as u64;
                let (period_start, period_end) = wait_closed_period(interval_secs).await;
                if last_closed_period_end != 0 && period_start != last_closed_period_end {
                    tracing::warn!(
                        expected_period_start = last_closed_period_end,
                        actual_period_start = period_start,
                        "stats period discontinuity (possible node clock skew or missed tick)"
                    );
                }
                last_closed_period_end = period_end;

                let config = stats.handle.config.load();
                let mut tunnel_ids: std::collections::HashSet<Uuid> =
                    config.tunnels.iter().map(|tunnel| tunnel.tunnel_id).collect();
                for route in &config.http_domain_routes {
                    tunnel_ids.insert(route.tunnel_id);
                }
                let mut samples =
                    collect_interval_samples(stats.tunnel_traffic.as_ref(), tunnel_ids);
                let mut flushed = stats.stats_flush_samples.lock().await;
                if !flushed.is_empty() {
                    samples.append(&mut flushed);
                    samples = merge_traffic_samples(samples);
                }
                let committed = stats
                    .handle
                    .stats_committed_seq
                    .load(Ordering::SeqCst);
                if committed > last_committed {
                    last_committed = committed;
                    last_ack_progress = std::time::Instant::now();
                }
                outbox.acknowledge(committed);
                // seq 取窗口结束的 Unix 秒：进程重启后仍单调，Board 的已提交水位不会挡住新包。
                let seq = period_end.max(stats.stats_seq.load(Ordering::SeqCst) + 1);
                stats.stats_seq.store(seq, Ordering::SeqCst);
                outbox.enqueue(StatsReport {
                    seq,
                    seq_from: None,
                    seq_to: None,
                    period_start_unix: period_start,
                    period_end_unix: period_end,
                    interval_secs: interval_secs as u32,
                    tunnels: samples,
                });
                if last_ack_progress.elapsed() >= STATS_RESEND_AFTER {
                    sent_upto = sent_upto.min(committed);
                    last_ack_progress = std::time::Instant::now();
                }
                let pending: Vec<StatsReport> = outbox
                    .pending_reports()
                    .filter(|report| report.seq > sent_upto)
                    .take(STATS_SEND_BURST)
                    .cloned()
                    .collect();
                for report in pending {
                    let seq = report.seq;
                    if stats
                        .handle
                        .try_emit(tz_proto::ControlOp::StatsReport(report))
                        .await
                    {
                        sent_upto = seq;
                    } else {
                        sent_upto = sent_upto.min(committed);
                        break;
                    }
                }
            }
        });

        let mut last_revision = 0_i64;
        let mut last_firewall_key = String::new();
        let mut wait_hint = std::time::Instant::now()
            .checked_sub(std::time::Duration::from_secs(10))
            .unwrap_or_else(std::time::Instant::now);
        loop {
            let config = self.handle.config.load();
            let identity = self.handle.identity.load();
            if identity.is_none() {
                if wait_hint.elapsed() >= std::time::Duration::from_secs(10) {
                    tracing::info!(
                        "waiting for board: node WebSocket / certificate (check --board and --token)"
                    );
                    wait_hint = std::time::Instant::now();
                }
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                continue;
            }
            if config.revision != last_revision {
                tracing::info!(
                    node_id = %config.node_id,
                    revision = config.revision,
                    tunnels = config.tunnels.len(),
                    http_routes = config.http_domain_routes.len(),
                    authorized_clients = config.authorized_client_fingerprints.len(),
                    "node config revision applied"
                );
                last_revision = config.revision;
                self.guard.reload(&config.guard_policy);
                // 策略 revision 变化时强制重同步防火墙（端口集合在下方每轮也会比对）。
                last_firewall_key.clear();
            }
            self.filing.store(Arc::new(tz_ingress::filing::FilingGate {
                enabled: config.cn_http_filing,
                whitelist: config.domain_whitelist.clone(),
            }));

            self.tunnel_sessions.retain(|tunnel_id, session| {
                if session.is_closed() {
                    tracing::info!(%tunnel_id, "carrier session closed; removed from node");
                    return false;
                }
                true
            });

            let route_table = build_route_table(
                &config.http_domain_routes,
                &self.tunnel_sessions,
                &self.tunnel_traffic,
            );
            self.shared_routes.store(Arc::new(route_table));
            self.sni_resolver.reload(&config.tls_certificates);

            if config.http_shared_port > 0 && !config.http_domain_routes.is_empty() {
                self.ensure_shared_http(
                    &config.bind_addr,
                    config.http_shared_port as u16,
                );
            } else if let Some(listener) = self.shared_http.lock().ok().and_then(|mut g| g.take()) {
                listener.stop.stop();
            }
            if config.https_shared_port > 0
                && !config.http_domain_routes.is_empty()
                && !config.tls_certificates.is_empty()
            {
                self.ensure_shared_https(
                    &config.bind_addr,
                    config.https_shared_port as u16,
                );
            } else if let Some(listener) = self.shared_https.lock().ok().and_then(|mut g| g.take())
            {
                listener.stop.stop();
            }

            for tunnel in &config.tunnels {
                let Some(port) = tunnel.remote_port.and_then(|port| u16::try_from(port).ok()) else {
                    if let Some((_, managed)) = self.ingress_tasks.remove(&tunnel.tunnel_id) {
                        tracing::info!(
                            tunnel_id = %tunnel.tunnel_id,
                            "stopping dedicated ingress; tunnel uses shared ports"
                        );
                        managed.stop.stop();
                    }
                    continue;
                };
                let http_hosts = DedicatedHttpHosts::new(&config.public_host, port, &tunnel.domains);
                if let Some(managed) = self.ingress_tasks.get(&tunnel.tunnel_id) {
                    if managed.revision == tunnel.revision {
                        if **managed.http_hosts.load() != http_hosts {
                            tracing::info!(
                                tunnel_id = %tunnel.tunnel_id,
                                ip_host = %http_hosts.ip_host,
                                domains = ?http_hosts.domains,
                                "dedicated http hosts updated"
                            );
                            managed.http_hosts.store(Arc::new(http_hosts));
                        }
                        continue;
                    }
                }
                if let Some((_, managed)) = self.ingress_tasks.remove(&tunnel.tunnel_id) {
                    managed.stop.stop();
                }
                // board 先下发配置、client 再主动连上来；暂时没有 carrier 会话是正常状态，不刷日志。
                let Some(session) = self.resolve_client_session(tunnel) else {
                    continue;
                };
                let ingress_kind = match tunnel.protocol.as_str() {
                    "http" => "http",
                    "udp" => "udp",
                    _ => "tcp",
                };
                let Some(factory) = find_ingress(ingress_kind) else {
                    continue;
                };
                let target_host = tunnel
                    .target_host
                    .clone()
                    .or_else(|| {
                        tunnel.target_url.as_ref().and_then(|target| {
                            Url::parse(target)
                                .ok()
                                .and_then(|url| url.host_str().map(str::to_string))
                        })
                    })
                    .unwrap_or_else(|| "127.0.0.1".into());
                let target_port = tunnel
                    .target_port
                    .and_then(|value| u16::try_from(value).ok())
                    .or_else(|| {
                        tunnel.target_url.as_ref().and_then(|target| {
                            Url::parse(target).ok().and_then(|url| url.port_or_known_default())
                        })
                    })
                    .unwrap_or(if tunnel.protocol == "http" {
                        80
                    } else if tunnel.protocol == "udp" {
                        53
                    } else {
                        80
                    });
                let target_label = format!("{target_host}:{target_port}");
                let (shutdown, stop_handle) = IngressShutdown::new_pair();
                let http_hosts = Arc::new(ArcSwap::from_pointee(http_hosts));
                let generation =
                    INGRESS_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                self.ingress_tasks.insert(
                    tunnel.tunnel_id,
                    ManagedIngress {
                        revision: tunnel.revision,
                        generation,
                        stop: stop_handle,
                        http_hosts: http_hosts.clone(),
                    },
                );
                self.handle
                    .running_tunnels
                    .insert(tunnel.tunnel_id, tunnel.revision);
                let bind_failed = Arc::new(Mutex::new(None));
                let ready_handle = self.handle.clone();
                let ready_tunnel_id = tunnel.tunnel_id;
                let ready_revision = tunnel.revision;
                let on_ingress_bound: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
                    let handle = ready_handle.clone();
                    tokio::spawn(async move {
                        emit_tunnel_ready(&handle, ready_tunnel_id, ready_revision).await;
                    });
                });
                let traffic = ensure_tunnel_traffic(&self.tunnel_traffic, tunnel.tunnel_id);
                let endpoint = TunnelEndpoint {
                    tunnel_id: tunnel.tunnel_id,
                    bind_addr: config.bind_addr.clone(),
                    port,
                    target_host,
                    target_port,
                    live_sessions: self.tunnel_sessions.clone(),
                    client_session: session,
                    guard: self.guard.clone(),
                    speed_limit_mbps: tunnel.speed_limit_mbps,
                    max_conns: tunnel.max_conns,
                    traffic,
                    rate_clock: self.rate_clock.clone(),
                    new_conn_bucket: new_conn_bucket(
                        &self.rate_clock,
                        tunnel.max_new_conns_per_sec,
                    ),
                    shutdown,
                    bind_failed: bind_failed.clone(),
                    on_ingress_bound: Some(on_ingress_bound),
                    http_hosts,
                    filing: self.filing.clone(),
                };
                let handle = self.handle.clone();
                let tunnel_id = tunnel.tunnel_id;
                let revision = tunnel.revision;
                let ingress_tasks = self.ingress_tasks.clone();
                let l4 = ingress_kind.to_string();
                tracing::info!(
                    tunnel_id = %tunnel.tunnel_id,
                    protocol = %tunnel.protocol,
                    bind_port = port,
                    revision = tunnel.revision,
                    target = %target_label,
                    "starting tunnel ingress listener"
                );
                tokio::spawn(async move {
                    let result = (factory.serve)(endpoint).await;
                    let still_current = ingress_tasks
                        .remove_if(&tunnel_id, |_, managed| managed.generation == generation)
                        .is_some();
                    if !still_current {
                        return;
                    }
                    handle.running_tunnels.remove(&tunnel_id);
                    if result.is_err() {
                        if let Some((port, reason)) =
                            bind_failed.lock().ok().and_then(|mut slot| slot.take())
                        {
                            emit_port_bind_failed(&handle, l4, port, reason).await;
                        } else {
                            emit_tunnel_failed(
                                &handle,
                                tunnel_id,
                                revision,
                                "ingress listener stopped",
                            )
                            .await;
                        }
                    }
                });
            }

            let current_ids: std::collections::HashSet<Uuid> =
                config.tunnels.iter().map(|t| t.tunnel_id).collect();
            self.ingress_tasks.retain(|id, managed| {
                if current_ids.contains(id) {
                    return true;
                }
                tracing::info!(tunnel_id = %id, "stopping tunnel ingress (removed from config)");
                managed.stop.stop();
                if let Some((_, session)) = self.tunnel_sessions.remove(id) {
                    session.close("tunnel removed from node config");
                }
                false
            });

            // IP ACL → nftables/iptables：在 TCP 握手前 DROP，UDP 直接丢包。
            {
                let mut tcp_ports = std::collections::BTreeSet::new();
                let mut udp_ports = std::collections::BTreeSet::new();
                for tunnel in &config.tunnels {
                    let Some(port) = tunnel.remote_port.and_then(|port| u16::try_from(port).ok())
                    else {
                        continue;
                    };
                    if tunnel.protocol == "udp" {
                        udp_ports.insert(port);
                    } else {
                        tcp_ports.insert(port);
                    }
                }
                if config.http_shared_port > 0 {
                    tcp_ports.insert(config.http_shared_port as u16);
                }
                if config.https_shared_port > 0 {
                    tcp_ports.insert(config.https_shared_port as u16);
                }
                let acl = crate::firewall::parse_ip_acl(&config.guard_policy);
                let firewall_key = format!(
                    "{:?}|{:?}|{:?}",
                    acl, tcp_ports, udp_ports
                );
                if firewall_key != last_firewall_key {
                    last_firewall_key = firewall_key;
                    let tcp: Vec<u16> = tcp_ports.into_iter().collect();
                    let udp: Vec<u16> = udp_ports.into_iter().collect();
                    crate::firewall::sync(&acl, &tcp, &udp);
                }
            }

            let mut tracked_ids = current_ids;
            for route in &config.http_domain_routes {
                tracked_ids.insert(route.tunnel_id);
            }
            {
                let mut flushed = self.stats_flush_samples.lock().await;
                self.tunnel_traffic.retain(|id, entry| {
                    if tracked_ids.contains(id) {
                        return true;
                    }
                    entry.close_connections();
                    let sample = tz_ingress::traffic::drain_traffic_accounting(entry, *id);
                    if tz_ingress::traffic::sample_has_bytes(&sample) {
                        flushed.push(sample);
                    }
                    false
                });
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }

    fn ensure_shared_https(&self, bind_addr: &str, port: u16) {
        let mut slot = self
            .shared_https
            .lock()
            .expect("shared https mutex poisoned");
        if slot
            .as_ref()
            .is_some_and(|listener| listener.bind_addr == bind_addr && listener.port == port)
        {
            return;
        }
        if let Some(listener) = slot.take() {
            listener.stop.stop();
        }
        let (shutdown, stop) = IngressShutdown::new_pair();
        let listener = SharedHttpsListener {
            bind_addr: bind_addr.to_string(),
            port,
            routes: self.shared_routes.clone(),
            resolver: self.sni_resolver.clone(),
            guard: self.guard.clone(),
            rate_clock: self.rate_clock.clone(),
            shutdown,
            filing: self.filing.clone(),
        };
        tracing::info!(bind = %bind_addr, port, "shared https ingress starting");
        tokio::spawn(async move {
            if let Err(error) = serve_shared_https(listener).await {
                tracing::warn!(?error, "shared https ingress stopped");
            }
        });
        *slot = Some(SharedListener {
            stop,
            bind_addr: bind_addr.to_string(),
            port,
        });
    }

    fn ensure_shared_http(&self, bind_addr: &str, port: u16) {
        let mut slot = self
            .shared_http
            .lock()
            .expect("shared http mutex poisoned");
        if slot
            .as_ref()
            .is_some_and(|listener| listener.bind_addr == bind_addr && listener.port == port)
        {
            return;
        }
        if let Some(listener) = slot.take() {
            listener.stop.stop();
        }
        let (shutdown, stop) = IngressShutdown::new_pair();
        let listener = SharedHttpListener {
            bind_addr: bind_addr.to_string(),
            port,
            routes: self.shared_routes.clone(),
            guard: self.guard.clone(),
            rate_clock: self.rate_clock.clone(),
            shutdown,
            filing: self.filing.clone(),
        };
        tokio::spawn(async move {
            if let Err(error) = serve_shared_http(listener).await {
                tracing::warn!(?error, "shared http ingress stopped");
            }
        });
        *slot = Some(SharedListener {
            stop,
            bind_addr: bind_addr.to_string(),
            port,
        });
    }

    pub fn register_client_session(&self, kind: &str, session: Arc<dyn CarrierSession>) {
        let peer = session.peer().clone();
        let Some(tunnel_id) = peer.tunnel_id else {
            tracing::warn!(carrier = kind, "carrier session without tunnel bind; closing");
            session.close("missing tunnel bind");
            return;
        };
        let config = self.handle.config.load();
        if let Some(tunnel) = config.tunnels.iter().find(|t| t.tunnel_id == tunnel_id) {
            if !session_owns_tunnel(session.as_ref(), tunnel) {
                tracing::warn!(
                    carrier = kind,
                    %tunnel_id,
                    peer_client_id = ?peer.client_id,
                    owner_client_id = ?tunnel.client_id,
                    "carrier session claims a tunnel owned by another client; closing"
                );
                session.close("tunnel not owned by this client");
                return;
            }
        }
        if kind == "tcp" {
            if let Some(existing) = self.tunnel_sessions.get(&tunnel_id) {
                if !existing.is_closed() && existing.peer().client_id == peer.client_id {
                    session.close("duplicate tcp carrier pool registration ignored");
                    return;
                }
            }
        }
        if let Some(previous) = self.tunnel_sessions.insert(tunnel_id, session.clone()) {
            if !Arc::ptr_eq(&previous, &session) {
                previous.close("replaced by newer carrier session for same tunnel");
            }
        }
        tracing::info!(
            carrier = kind,
            %tunnel_id,
            client_id = ?peer.client_id,
            carrier_sessions = self.tunnel_sessions.len(),
            "client carrier session registered for tunnel"
        );
    }

    pub async fn serve_carrier(
        &self,
        kind: &str,
        config: tz_carrier::types::ListenConfig,
    ) -> anyhow::Result<()> {
        let Some(factory) = find_carrier(kind) else {
            anyhow::bail!("{kind} carrier is unavailable");
        };
        tracing::info!(
            carrier = kind,
            bind = %config.bind_addr,
            port = config.port,
            "carrier listener started"
        );
        let mut listener = (factory.listen)(config)
            .await
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
        loop {
            let session = match listener.accept().await {
                Ok(session) => session,
                Err(err) => {
                    tracing::warn!(carrier = kind, ?err, "carrier accept failed, continuing");
                    continue;
                }
            };
            self.register_client_session(kind, session);
        }
    }
}
