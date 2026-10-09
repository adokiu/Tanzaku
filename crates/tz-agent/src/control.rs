use portable_atomic::AtomicU64;
use arc_swap::ArcSwap;
use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use std::{
    sync::{
        atomic::Ordering,
        Arc,
    },
    time::Duration,
};
use thiserror::Error;
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tz_pki::IdentityMaterial;
use tracing::{debug, info, warn};
use crate::hello::build_hello_message;
use tz_proto::{
    AgentCapabilities, AuthMessage, CertIssued, ClientConfig, ControlOp, CsrRequest, Envelope,
    MessageKind, NodeConfig, ProtocolVersion, StatsReport, TunnelSpec, TunnelStateReport,
};

/// Ensures agent WebSocket URL includes the board control path (`/ws`).
pub fn normalize_board_url(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return trimmed.to_string();
    }
    let lower = trimmed.to_ascii_lowercase();
    let converted = if lower.starts_with("https://") {
        format!("wss://{}", &trimmed["https://".len()..])
    } else if lower.starts_with("http://") {
        format!("ws://{}", &trimmed["http://".len()..])
    } else if !lower.contains("://") {
        format!("ws://{trimmed}")
    } else {
        trimmed.to_string()
    };
    let trimmed = converted.as_str();
    if trimmed.ends_with("/ws") {
        return trimmed.to_string();
    }
    if let Some(rest) = trimmed.split("://").nth(1) {
        if rest.contains('/') {
            return trimmed.to_string();
        }
    }
    format!("{trimmed}/ws")
}

#[derive(Debug, Error)]
pub enum ControlError {
    #[error("control websocket failed: {0}")]
    WebSocket(String),
    #[error("board rejected control session: {0}")]
    Rejected(String),
}

#[derive(Clone)]
pub struct NodeHandle {
    pub config: Arc<ArcSwap<NodeConfig>>,
    pub identity: Arc<ArcSwap<Option<IdentityMaterial>>>,
    events: Arc<Mutex<Option<mpsc::Sender<ControlOp>>>>,
    pub stats_committed_seq: Arc<portable_atomic::AtomicU64>,
    pub running_tunnels: Arc<DashMap<uuid::Uuid, i64>>,
}

#[derive(Clone)]
pub struct AgentHandle {
    pub config: Arc<ArcSwap<ClientConfig>>,
    pub identity: Arc<ArcSwap<Option<IdentityMaterial>>>,
    events: Arc<Mutex<Option<mpsc::Sender<ControlOp>>>>,
}

impl AgentHandle {
    pub async fn emit(&self, op: ControlOp) {
        let guard = self.events.lock().await;
        if let Some(tx) = guard.as_ref() {
            let _ = tx.send(op).await;
        }
    }
}

impl NodeHandle {
    pub async fn emit(&self, op: ControlOp) {
        let guard = self.events.lock().await;
        if let Some(tx) = guard.as_ref() {
            let _ = tx.send(op).await;
        }
    }

    pub async fn try_emit(&self, op: ControlOp) -> bool {
        let guard = self.events.lock().await;
        if let Some(tx) = guard.as_ref() {
            tx.try_send(op).is_ok()
        } else {
            false
        }
    }
}

pub struct ControlClient {
    board_url: String,
    token: String,
    capabilities: AgentCapabilities,
    role: ControlRole,
}

#[derive(Clone, Copy)]
enum ControlRole {
    Node,
    Agent,
}

impl ControlClient {
    pub fn with_capabilities(mut self, capabilities: AgentCapabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    pub fn node(board_url: impl Into<String>, token: impl Into<String>) -> Self {
        let board_url = normalize_board_url(&board_url.into());
        Self {
            board_url,
            token: token.into(),
            capabilities: AgentCapabilities {
                carriers: vec!["tcp".into(), "quic".into()],
                ingress: vec!["tcp".into(), "udp".into(), "http".into()],
                guards: vec![
                    "ip_acl".into(),
                    "per_ip_limit".into(),
                    "auto_ban".into(),
                    "udp_amplify".into(),
                    "http_guard".into(),
                    "tls_guard".into(),
                    "preauth_guard".into(),
                ],
            },
            role: ControlRole::Node,
        }
    }

    pub fn agent(board_url: impl Into<String>, token: impl Into<String>) -> Self {
        let board_url = normalize_board_url(&board_url.into());
        Self {
            board_url,
            token: token.into(),
            capabilities: AgentCapabilities {
                carriers: vec!["tcp".into(), "quic".into()],
                ingress: vec![],
                guards: vec![],
            },
            role: ControlRole::Agent,
        }
    }

    pub fn start_node(self) -> NodeHandle {
        let config = Arc::new(ArcSwap::from_pointee(empty_node_config()));
        let identity = Arc::new(ArcSwap::from_pointee(None));
        let events = Arc::new(Mutex::new(None));
        let handle = NodeHandle {
            config: config.clone(),
            identity: identity.clone(),
            events: events.clone(),
            stats_committed_seq: Arc::new(AtomicU64::new(0)),
            running_tunnels: Arc::new(DashMap::new()),
        };
        let worker = handle.clone();
        let board_url = self.board_url.clone();
        tokio::spawn(async move {
            let mut backoff = Duration::from_secs(1);
            loop {
                let stats_committed = worker.stats_committed_seq.clone();
                match self
                    .connect_once_node(&config, &identity, &events, stats_committed)
                    .await
                {
                    Ok(()) => {
                        warn!(board = %board_url, role = "node", "board control session ended, reconnecting");
                        backoff = Duration::from_secs(1);
                    }
                    Err(error) => {
                        warn!(
                            board = %board_url,
                            role = "node",
                            error = %error,
                            retry_secs = backoff.as_secs(),
                            "board connection failed, retrying"
                        );
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(Duration::from_secs(60));
                    }
                }
            }
        });
        handle
    }

    pub fn start_agent(self) -> AgentHandle {
        let config = Arc::new(ArcSwap::from_pointee(empty_client_config()));
        let identity = Arc::new(ArcSwap::from_pointee(None));
        let events = Arc::new(Mutex::new(None));
        let handle = AgentHandle {
            config: config.clone(),
            identity: identity.clone(),
            events: events.clone(),
        };
        let board_url = self.board_url.clone();
        tokio::spawn(async move {
            let mut backoff = Duration::from_secs(1);
            loop {
                match self
                    .connect_once_agent(&config, &identity, &events)
                    .await
                {
                    Ok(()) => {
                        warn!(
                            board = %board_url,
                            role = "client",
                            "board control session ended, reconnecting"
                        );
                        backoff = Duration::from_secs(1);
                    }
                    Err(error) => {
                        warn!(
                            board = %board_url,
                            role = "client",
                            error = %error,
                            retry_secs = backoff.as_secs(),
                            "board connection failed, retrying"
                        );
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(Duration::from_secs(60));
                    }
                }
            }
        });
        handle
    }

    async fn connect_once_node(
        &self,
        slot: &Arc<ArcSwap<NodeConfig>>,
        identity_slot: &Arc<ArcSwap<Option<IdentityMaterial>>>,
        events: &Arc<Mutex<Option<mpsc::Sender<ControlOp>>>>,
        stats_committed: Arc<AtomicU64>,
    ) -> Result<(), ControlError> {
        let (mut sender, mut receiver) = self.handshake().await?;
        let identity = bootstrap_identity(
            &mut sender,
            &mut receiver,
            IdentityRole::Node(slot.clone()),
        )
        .await?;
        identity_slot.store(Arc::new(Some(identity)));
        let node_cfg = slot.load();
        info!(
            board = %self.board_url,
            node_id = %node_cfg.node_id,
            revision = node_cfg.revision,
            tunnels = node_cfg.tunnels.len(),
            "node connected to board, control session active"
        );
        log_node_config_summary(&node_cfg, "initial");
        let (event_tx, mut event_rx) = mpsc::channel::<ControlOp>(2048);
        *events.lock().await = Some(event_tx);
        let mut outbound_id = 10_u64;
        let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                message = receiver.next() => {
                    let Some(message) = message else { break; };
                    let message = message.map_err(|err| ControlError::WebSocket(err.to_string()))?;
                    match message {
                        Message::Binary(bytes) => {
                            let envelope: Envelope<ControlOp> =
                                rmp_serde::from_slice(&bytes).map_err(|err| ControlError::WebSocket(err.to_string()))?;
                            match envelope.payload {
                                ControlOp::Error(error) => return Err(ControlError::Rejected(error.message)),
                                ControlOp::NodeConfig(update) => {
                                    slot.store(Arc::new(update.clone()));
                                    debug!(
                                        node_id = %update.node_id,
                                        revision = update.revision,
                                        tunnels = update.tunnels.len(),
                                        "node config updated from board"
                                    );
                                    log_node_config_summary(&update, "push");
                                }
                                ControlOp::TunnelUpsert(spec) => {
                                    debug!(
                                        tunnel_id = %spec.tunnel_id,
                                        protocol = %spec.protocol,
                                        remote_port = ?spec.remote_port,
                                        revision = spec.revision,
                                        "tunnel upsert from board"
                                    );
                                    apply_tunnel_upsert(slot, spec);
                                }
                                ControlOp::TunnelRemove { tunnel_id } => {
                                    info!(%tunnel_id, "tunnel removed by board");
                                    apply_tunnel_remove(slot, tunnel_id);
                                }
                                ControlOp::TunnelSuspend { tunnel_id } => {
                                    info!(%tunnel_id, "tunnel suspended by board");
                                    apply_tunnel_remove(slot, tunnel_id);
                                }
                                ControlOp::TunnelResume { tunnel_id } => {
                                    info!(%tunnel_id, "tunnel resume requested by board");
                                }
                                ControlOp::StatsAck { seq } => {
                                    stats_committed.fetch_max(seq, Ordering::SeqCst);
                                }
                                _ => {}
                            }
                        }
                        Message::Ping(payload) => {
                            if sender.send(Message::Pong(payload)).await.is_err() {
                                break;
                            }
                        }
                        Message::Pong(_) => {}
                        _ => {}
                    }
                }
                Some(op) = event_rx.recv() => {
                    outbound_id += 1;
                    send(&mut sender, outbound_id, op).await?;
                }
                _ = heartbeat.tick() => {
                    if sender.send(Message::Ping(Vec::new().into())).await.is_err() {
                        break;
                    }
                }
            }
        }
        *events.lock().await = None;
        Ok(())
    }

    async fn connect_once_agent(
        &self,
        slot: &Arc<ArcSwap<ClientConfig>>,
        identity_slot: &Arc<ArcSwap<Option<IdentityMaterial>>>,
        events: &Arc<Mutex<Option<mpsc::Sender<ControlOp>>>>,
    ) -> Result<(), ControlError> {
        let (mut sender, mut receiver) = self.handshake().await?;
        let identity = bootstrap_identity(
            &mut sender,
            &mut receiver,
            IdentityRole::Agent(slot.clone()),
        )
        .await?;
        identity_slot.store(Arc::new(Some(identity)));
        let client_cfg = slot.load();
        info!(
            board = %self.board_url,
            client_id = %client_cfg.client_id,
            tunnels = client_cfg.tunnels.len(),
            "client connected to board, control session active"
        );
        log_client_config_summary(&client_cfg);
        let (event_tx, mut event_rx) = mpsc::channel::<ControlOp>(2048);
        *events.lock().await = Some(event_tx);
        let mut outbound_id = 10_u64;
        let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                message = receiver.next() => {
                    let Some(message) = message else { break; };
                    let message = message.map_err(|err| ControlError::WebSocket(err.to_string()))?;
                    match message {
                        Message::Binary(bytes) => {
                            let envelope: Envelope<ControlOp> =
                                rmp_serde::from_slice(&bytes).map_err(|err| ControlError::WebSocket(err.to_string()))?;
                            match envelope.payload {
                                ControlOp::Error(error) => return Err(ControlError::Rejected(error.message)),
                                ControlOp::ClientConfig(update) => {
                                    slot.store(Arc::new(update.clone()));
                                    debug!(
                                        client_id = %update.client_id,
                                        tunnels = update.tunnels.len(),
                                        "client config updated from board"
                                    );
                                    log_client_config_summary(&update);
                                }
                                ControlOp::TunnelRemove { tunnel_id } => {
                                    info!(%tunnel_id, "tunnel assignment removed by board");
                                    apply_client_tunnel_remove(slot, tunnel_id);
                                }
                                ControlOp::TunnelSuspend { tunnel_id } => {
                                    info!(%tunnel_id, "tunnel assignment suspended by board");
                                    apply_client_tunnel_remove(slot, tunnel_id);
                                }
                                ControlOp::TunnelResume { tunnel_id } => {
                                    info!(
                                        %tunnel_id,
                                        "tunnel resume requested by board (await ClientConfig)"
                                    );
                                }
                                _ => {}
                            }
                        }
                        Message::Ping(payload) => {
                            if sender.send(Message::Pong(payload)).await.is_err() {
                                break;
                            }
                        }
                        Message::Pong(_) => {}
                        _ => {}
                    }
                }
                Some(op) = event_rx.recv() => {
                    outbound_id += 1;
                    send(&mut sender, outbound_id, op).await?;
                }
                _ = heartbeat.tick() => {
                    if sender.send(Message::Ping(Vec::new().into())).await.is_err() {
                        break;
                    }
                }
            }
        }
        *events.lock().await = None;
        Ok(())
    }

    async fn handshake(
        &self,
    ) -> Result<
        (
            futures_util::stream::SplitSink<
                tokio_tungstenite::WebSocketStream<
                    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
                >,
                Message,
            >,
            futures_util::stream::SplitStream<
                tokio_tungstenite::WebSocketStream<
                    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
                >,
            >,
        ),
        ControlError,
    > {
        let role_label = match self.role {
            ControlRole::Node => "node",
            ControlRole::Agent => "client",
        };
        info!(board = %self.board_url, role = role_label, "connecting to board WebSocket");
        let (socket, _) = connect_async(&self.board_url)
            .await
            .map_err(|err| ControlError::WebSocket(err.to_string()))?;
        info!(board = %self.board_url, role = role_label, "WebSocket connected, sending Auth/Hello");
        let (mut sender, mut receiver) = socket.split();
        send(
            &mut sender,
            1,
            ControlOp::Auth(AuthMessage {
                token: self.token.clone(),
            }),
        )
        .await?;
        send(
            &mut sender,
            2,
            ControlOp::Hello(build_hello_message(
                0,
                self.capabilities.clone(),
                vec![],
            )),
        )
        .await?;
        Ok((sender, receiver))
    }
}

fn log_node_config_summary(cfg: &NodeConfig, source: &str) {
    for tunnel in &cfg.tunnels {
        debug!(
            source,
            tunnel_id = %tunnel.tunnel_id,
            protocol = %tunnel.protocol,
            remote_port = ?tunnel.remote_port,
            revision = tunnel.revision,
            client_fingerprint = tunnel.client_fingerprint.as_deref(),
            "node tunnel in config"
        );
    }
}

fn log_client_config_summary(cfg: &ClientConfig) {
    for tunnel in &cfg.tunnels {
        debug!(
            tunnel_id = %tunnel.tunnel_id,
            protocol = %tunnel.protocol,
            carrier = %tunnel.carrier,
            node_id = %tunnel.node_id,
            node_public_host = %tunnel.node_public_host,
            carrier_port = tunnel.carrier_port,
            target_host = tunnel.target_host.as_deref(),
            target_port = tunnel.target_port,
            "client tunnel assignment"
        );
    }
}

fn apply_tunnel_upsert(slot: &Arc<ArcSwap<NodeConfig>>, spec: TunnelSpec) {
    let spec = spec.clone();
    slot.rcu(|current| {
        let mut next = (**current).clone();
        if let Some(existing) = next
            .tunnels
            .iter_mut()
            .find(|tunnel| tunnel.tunnel_id == spec.tunnel_id)
        {
            *existing = spec.clone();
        } else {
            next.tunnels.push(spec.clone());
        }
        if let Some(fingerprint) = spec.client_fingerprint.as_ref() {
            if !fingerprint.is_empty()
                && !next
                    .authorized_client_fingerprints
                    .iter()
                    .any(|value| value == fingerprint)
            {
                next.authorized_client_fingerprints.push(fingerprint.clone());
            }
        }
        next.revision = next.revision.saturating_add(1);
        Arc::new(next)
    });
}

fn apply_tunnel_remove(slot: &Arc<ArcSwap<NodeConfig>>, tunnel_id: uuid::Uuid) {
    slot.rcu(|current| {
        let mut next = (**current).clone();
        next.tunnels.retain(|tunnel| tunnel.tunnel_id != tunnel_id);
        next.revision = next.revision.saturating_add(1);
        Arc::new(next)
    });
}

fn apply_client_tunnel_remove(slot: &Arc<ArcSwap<ClientConfig>>, tunnel_id: uuid::Uuid) {
    slot.rcu(|current| {
        let mut next = (**current).clone();
        next.tunnels.retain(|tunnel| tunnel.tunnel_id != tunnel_id);
        Arc::new(next)
    });
}

enum IdentityRole {
    Node(Arc<ArcSwap<NodeConfig>>),
    Agent(Arc<ArcSwap<ClientConfig>>),
}

async fn bootstrap_identity(
    sender: &mut (impl SinkExt<Message> + Unpin),
    receiver: &mut (impl StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
          + Unpin),
    role: IdentityRole,
) -> Result<IdentityMaterial, ControlError> {
    let san = wait_initial_config(receiver, &role).await?;
    let key_pair = rcgen::KeyPair::generate()
        .map_err(|err| ControlError::WebSocket(err.to_string()))?;
    let csr = rcgen::CertificateParams::new([san.clone()])
        .map_err(|err| ControlError::WebSocket(err.to_string()))?
        .serialize_request(&key_pair)
        .map_err(|err| ControlError::WebSocket(err.to_string()))?;
    send(
        sender,
        3,
        ControlOp::CsrRequest(CsrRequest {
            csr_pem: csr
                .pem()
                .map_err(|err| ControlError::WebSocket(err.to_string()))?,
        }),
    )
    .await?;
    loop {
        let Some(message) = receiver.next().await else {
            return Err(ControlError::WebSocket("board closed before CertIssued".into()));
        };
        let message = message.map_err(|err| ControlError::WebSocket(err.to_string()))?;
        let Message::Binary(bytes) = message else {
            continue;
        };
        let envelope: Envelope<ControlOp> =
            rmp_serde::from_slice(&bytes).map_err(|err| ControlError::WebSocket(err.to_string()))?;
        match envelope.payload {
            ControlOp::Error(error) => return Err(ControlError::Rejected(error.message)),
            ControlOp::CertIssued(CertIssued {
                certificate_pem,
                not_after,
                ..
            }) => {
                info!(%not_after, "board issued agent certificate (30d validity, renew before expiry per plan)");
                return Ok(IdentityMaterial {
                    certificate_chain_pem: vec![certificate_pem],
                    private_key_pem: key_pair.serialize_pem(),
                    not_after: Some(not_after),
                });
            }
            ControlOp::NodeConfig(update) => {
                if let IdentityRole::Node(slot) = &role {
                    slot.store(Arc::new(update));
                }
            }
            ControlOp::ClientConfig(update) => {
                if let IdentityRole::Agent(slot) = &role {
                    slot.store(Arc::new(update));
                }
            }
            _ => {}
        }
    }
}

async fn wait_initial_config(
    receiver: &mut (impl StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
          + Unpin),
    role: &IdentityRole,
) -> Result<String, ControlError> {
    for _ in 0..32 {
        let Some(message) = receiver.next().await else {
            break;
        };
        let message = message.map_err(|err| ControlError::WebSocket(err.to_string()))?;
        let Message::Binary(bytes) = message else {
            continue;
        };
        let envelope: Envelope<ControlOp> =
            rmp_serde::from_slice(&bytes).map_err(|err| ControlError::WebSocket(err.to_string()))?;
        match envelope.payload {
            ControlOp::Error(error) => return Err(ControlError::Rejected(error.message)),
            ControlOp::NodeConfig(update) => {
                if let IdentityRole::Node(slot) = role {
                    slot.store(Arc::new(update.clone()));
                    return Ok(format!("node-{}", update.node_id));
                }
            }
            ControlOp::ClientConfig(update) => {
                if let IdentityRole::Agent(slot) = role {
                    slot.store(Arc::new(update.clone()));
                    return Ok(format!("client-{}", update.client_id));
                }
            }
            _ => {}
        }
    }
    Err(ControlError::WebSocket(
        "board did not send NodeConfig/ClientConfig after Hello (check board logs)".into(),
    ))
}

async fn send(
    sender: &mut (impl SinkExt<Message> + Unpin),
    id: u64,
    payload: ControlOp,
) -> Result<(), ControlError> {
    let bytes = rmp_serde::to_vec_named(&Envelope {
        version: ProtocolVersion::CURRENT,
        id,
        kind: MessageKind::Request,
        payload,
    })
    .map_err(|err| ControlError::WebSocket(err.to_string()))?;
    if sender.send(Message::Binary(bytes.into())).await.is_err() {
        return Err(ControlError::WebSocket(
            "failed to send control frame".into(),
        ));
    }
    Ok(())
}

fn empty_node_config() -> NodeConfig {
    NodeConfig {
        node_id: uuid::Uuid::nil(),
        revision: 0,
        bind_addr: "0.0.0.0".into(),
        public_host: "127.0.0.1".into(),
        carrier_ports: serde_json::json!({}),
        tcp_port_ranges: serde_json::json!([]),
        udp_port_ranges: serde_json::json!([]),
        port_exclude: serde_json::json!([]),
        http_shared_port: 80,
        https_shared_port: 443,
        guard_policy: serde_json::json!({}),
        cn_http_filing: false,
        cn_residency: false,
        domain_whitelist: vec![],
        trusted_proxies: vec![],
        board_ca_pem: String::new(),
        authorized_client_fingerprints: vec![],
        certificate_fingerprint: None,
        tunnels: vec![],
        http_domain_routes: vec![],
        tls_certificates: vec![],
        host_metrics_interval_secs: 1,
    }
}

fn empty_client_config() -> ClientConfig {
    ClientConfig {
        client_id: uuid::Uuid::nil(),
        board_ca_pem: String::new(),
        certificate_fingerprint: None,
        tunnels: vec![],
    }
}

pub async fn emit_stats_report(handle: &NodeHandle, report: StatsReport) {
    if !handle
        .try_emit(ControlOp::StatsReport(report))
        .await
    {
        tracing::warn!("dropped StatsReport: node control outbound queue full");
    }
}

pub async fn emit_host_metrics_report(handle: &NodeHandle, report: tz_proto::HostMetricsReport) {
    handle.emit(ControlOp::HostMetricsReport(report)).await;
}

pub async fn emit_host_metrics_report_agent(
    handle: &AgentHandle,
    report: tz_proto::HostMetricsReport,
) {
    handle.emit(ControlOp::HostMetricsReport(report)).await;
}

pub async fn emit_client_traffic_report(
    handle: &AgentHandle,
    report: tz_proto::ClientTrafficReport,
) {
    handle.emit(ControlOp::ClientTrafficReport(report)).await;
}

pub async fn emit_tunnel_ready(handle: &NodeHandle, tunnel_id: uuid::Uuid, revision: i64) {
    handle
        .emit(ControlOp::TunnelReady {
            tunnel_id,
            revision,
        })
        .await;
}

pub async fn emit_tunnel_failed(
    handle: &NodeHandle,
    tunnel_id: uuid::Uuid,
    revision: i64,
    reason: impl Into<String>,
) {
    handle
        .emit(ControlOp::TunnelFailed {
            tunnel_id,
            revision,
            reason: reason.into(),
        })
        .await;
}

pub async fn emit_tunnel_state(handle: &AgentHandle, report: TunnelStateReport) {
    handle.emit(ControlOp::TunnelState(report)).await;
}

pub async fn emit_port_bind_failed(
    handle: &NodeHandle,
    l4: impl Into<String>,
    port: u16,
    reason: impl Into<String>,
) {
    handle
        .emit(ControlOp::PortBindFailed {
            l4: l4.into(),
            port,
            reason: reason.into(),
        })
        .await;
}

pub async fn emit_guard_event(handle: &NodeHandle, report: tz_proto::GuardEventReport) {
    let _ = handle
        .try_emit(ControlOp::GuardEventReport(report))
        .await;
}
