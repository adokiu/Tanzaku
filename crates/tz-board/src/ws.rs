use crate::{pki, setup::AppState};
use axum::{
    extract::{
        connect_info::ConnectInfo,
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::Response,
};
use std::net::SocketAddr;
use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::sync::{Arc, LazyLock};
use tokio::sync::mpsc;
use tracing::{info, warn};
use tz_proto::{
    AuthMessage, CertIssued, ClientConfig, ClientTunnelAssign, ControlOp, Envelope, ErrorPayload,
    HelloMessage, HttpDomainRoute, MessageKind, NodeConfig, NodeTlsCertificate, ProtocolVersion,
    TunnelSpec,
};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlKind {
    Node,
    Agent,
}

#[derive(Clone)]
struct ControlState {
    app: Arc<AppState>,
    kind: ControlKind,
}

#[derive(Clone)]
pub(crate) struct LiveSession {
    pub kind: ControlKind,
    pub tx: mpsc::Sender<Vec<u8>>,
}

pub(crate) static SESSIONS: LazyLock<DashMap<Uuid, LiveSession>> = LazyLock::new(DashMap::new);

const OUTBOUND_QUEUE_CAP: usize = 256;

pub fn node_router(state: Arc<AppState>) -> axum::Router {
    let shared = ControlState {
        app: state,
        kind: ControlKind::Node,
    };
    axum::Router::new()
        .route("/ws", axum::routing::get(ws_upgrade))
        .with_state(shared)
}

pub fn agent_router(state: Arc<AppState>) -> axum::Router {
    let shared = ControlState {
        app: state,
        kind: ControlKind::Agent,
    };
    axum::Router::new()
        .route("/ws", axum::routing::get(ws_upgrade))
        .with_state(shared)
}

async fn ws_upgrade(
    ws: WebSocketUpgrade,
    State(control): State<ControlState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Response {
    let peer_ip = peer.ip().to_string();
    ws.on_upgrade(move |socket| handle_socket(socket, control, peer_ip))
}

async fn handle_socket(socket: WebSocket, control: ControlState, peer_ip: String) {
    let (mut sender, mut receiver) = socket.split();
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(OUTBOUND_QUEUE_CAP);
    let pump = tokio::spawn(async move {
        while let Some(bytes) = rx.recv().await {
            if sender.send(Message::Binary(bytes.into())).await.is_err() {
                break;
            }
        }
    });

    let pg = match ready_pool(&control.app).await {
        Some(pg) => pg,
        None => {
            let _ = send_error(&tx, 1, "board 尚未初始化").await;
            pump.abort();
            return;
        }
    };

    let mut authenticated: Option<Uuid> = None;
    let mut hello_done = false;
    let mut last_touch = std::time::Instant::now();
    let mut message_id = 1_u64;

    while let Some(message) = receiver.next().await {
        let Ok(message) = message else {
            break;
        };
        if let (Some(id), true) = (authenticated, hello_done) {
            if last_touch.elapsed() >= std::time::Duration::from_secs(20) {
                last_touch = std::time::Instant::now();
                let pg_touch = pg.clone();
                let kind = control.kind;
                tokio::spawn(async move {
                    touch_last_seen(&pg_touch, kind, id).await;
                });
            }
        }
        let Message::Binary(bytes) = message else {
            continue;
        };
        let envelope = match rmp_serde::from_slice::<Envelope<ControlOp>>(&bytes) {
            Ok(envelope) => envelope,
            Err(_) => {
                let _ = send_error(&tx, message_id, "控制帧格式无效").await;
                message_id += 1;
                continue;
            }
        };
        if envelope.version.major != ProtocolVersion::CURRENT.major {
            let _ = send_error(&tx, envelope.id, "协议版本不兼容").await;
            break;
        }
        message_id = envelope.id.max(message_id);

        match envelope.payload {
            ControlOp::Auth(AuthMessage { token }) => {
                if authenticated.is_some() {
                    let _ = send_error(&tx, envelope.id, "已经完成认证").await;
                    continue;
                }
                match verify_token(&pg, control.kind, &token).await {
                    Ok(id) => {
                        authenticated = Some(id);
                        info!(?control.kind, %id, "control websocket authenticated");
                    }
                    Err(message) => {
                        let _ = send_error(&tx, envelope.id, message).await;
                        break;
                    }
                }
            }
            ControlOp::Hello(hello) => {
                let Some(id) = authenticated else {
                    let _ = send_error(&tx, envelope.id, "请先发送 Auth").await;
                    continue;
                };
                if let Err(message) =
                    persist_hello(&pg, control.kind, id, &hello, &peer_ip).await
                {
                    let _ = send_error(&tx, envelope.id, &message).await;
                    continue;
                }
                SESSIONS.insert(
                    id,
                    LiveSession {
                        kind: control.kind,
                        tx: tx.clone(),
                    },
                );
                hello_done = true;
                last_touch = std::time::Instant::now();
                let pg_hello = pg.clone();
                let tx_hello = tx.clone();
                let kind = control.kind;
                let hello_id = envelope.id;
                tokio::spawn(async move {
                    crate::stats::ensure_cursor_row(&pg_hello, id).await;
                    if kind == ControlKind::Node {
                        let seq = crate::stats::committed_seq(&pg_hello, id).await;
                        crate::stats::seed_committed_seq(id, seq);
                        crate::stats::reset_ingress_session(id);
                        if seq > 0 {
                            crate::ws::notify_stats_ack(id, seq as u64).await;
                        }
                    } else {
                        // client 重连：轮换最新密钥，先推 node 再回 ClientConfig（client 主动连 server）。
                        rotate_client_carrier_secrets(&pg_hello, id).await;
                    }
                    match build_config(&pg_hello, kind, id).await {
                        Ok(config) => {
                            let op = match kind {
                                ControlKind::Node => {
                                    ControlOp::NodeConfig(config.node.expect("node"))
                                }
                                ControlKind::Agent => {
                                    ControlOp::ClientConfig(config.client.expect("client"))
                                }
                            };
                            let _ = send_response(&tx_hello, hello_id, op).await;
                        }
                        Err(error) => {
                            warn!(?kind, %id, %error, "failed to build control config after Hello");
                            let _ = send_error(&tx_hello, hello_id, &error).await;
                        }
                    }
                });
            }
            ControlOp::CsrRequest(csr) => {
                let Some(id) = authenticated else {
                    let _ = send_error(&tx, envelope.id, "请先发送 Auth").await;
                    continue;
                };
                match pki::sign_csr(&pg, &csr.csr_pem).await {
                    Ok(issued) => {
                        persist_certificate(&pg, control.kind, id, &issued.fingerprint).await;
                        let _ = send_response(
                            &tx,
                            envelope.id,
                            ControlOp::CertIssued(CertIssued {
                                certificate_pem: issued.certificate_pem,
                                fingerprint: issued.fingerprint,
                                not_after: issued.not_after,
                            }),
                        )
                        .await;
                    }
                    Err(_) => {
                        let _ = send_error(&tx, envelope.id, "CSR 签发失败").await;
                    }
                }
            }
            ControlOp::StatsReport(report) => {
                let Some(node_id) = authenticated else {
                    let _ = send_error(&tx, envelope.id, "请先发送 Auth").await;
                    continue;
                };
                if control.kind != ControlKind::Node {
                    let _ = send_error(&tx, envelope.id, "StatsReport 仅适用于节点").await;
                    continue;
                }
                crate::stats::enqueue_stats_report(node_id, report);
            }
            ControlOp::HostMetricsReport(report) => {
                let Some(node_id) = authenticated else {
                    continue;
                };
                if control.kind != ControlKind::Node {
                    continue;
                }
                crate::node_metrics::ingest(node_id, &report);
                if let Some(mut redis) = control.app.redis_connection() {
                    crate::node_metrics::persist_redis(&mut redis, node_id, &report).await;
                }
            }
            ControlOp::GuardEventReport(report) => {
                let Some(node_id) = authenticated else {
                    continue;
                };
                if control.kind != ControlKind::Node {
                    continue;
                }
                crate::guard_events::ingest(
                    &pg,
                    node_id,
                    &report.rule,
                    report.peer.as_deref(),
                    report.tunnel_id,
                    &report.detail,
                    report.hit_count,
                )
                .await;
            }
            ControlOp::ClientTrafficReport(report) => {
                let Some(client_id) = authenticated else {
                    continue;
                };
                if control.kind != ControlKind::Agent {
                    continue;
                }
                // 不可用于计费：client 侧可被篡改，仅更新 Client 管理页展示。
                crate::client_traffic::ingest(client_id, &report);
            }
            ControlOp::TunnelReady {
                tunnel_id,
                revision,
            } => {
                let Some(_) = authenticated else {
                    continue;
                };
                let pg_u = pg.clone();
                tokio::spawn(async move {
                    let _ = sqlx::query(
                        "UPDATE tunnels SET status = 'active', revision = GREATEST(revision, $2), last_error = NULL, updated_at = now() WHERE id = $1 AND status IN ('provisioning', 'ready', 'client_offline', 'node_offline', 'error')",
                    )
                    .bind(tunnel_id)
                    .bind(revision)
                    .execute(&pg_u)
                    .await;
                });
            }
            ControlOp::TunnelFailed {
                tunnel_id,
                revision,
                reason,
            } => {
                let Some(_) = authenticated else {
                    continue;
                };
                let pg_u = pg.clone();
                tokio::spawn(async move {
                    let _ = sqlx::query(
                        "UPDATE tunnels SET status = 'error', last_error = $3, updated_at = now() WHERE id = $1 AND revision <= $2 AND status NOT IN ('deleted', 'suspended', 'pending_review')",
                    )
                    .bind(tunnel_id)
                    .bind(revision)
                    .bind(reason)
                    .execute(&pg_u)
                    .await;
                });
            }
            ControlOp::TunnelState(report) => {
                let Some(client_id) = authenticated else {
                    continue;
                };
                if control.kind != ControlKind::Agent {
                    continue;
                }
                let message = report
                    .message
                    .clone()
                    .unwrap_or_else(|| {
                        if report.backend_tls_error {
                            "backend TLS verification failed".into()
                        } else if !report.reachable {
                            "upstream target unreachable".into()
                        } else {
                            "tunnel recovered".into()
                        }
                    });
                let reconnecting = report
                    .message
                    .as_deref()
                    .is_some_and(|m| m.contains("reconnecting"));
                // carrier 已连通即视为运行中；勿因尚未有首包流量而长期停留在 provisioning。
                let (status, clear_error) = if report.reachable {
                    ("active", true)
                } else if reconnecting {
                    ("provisioning", false)
                } else if report.carrier_connected {
                    ("active", true)
                } else {
                    ("error", false)
                };
                let tunnel_id = report.tunnel_id;
                let pg_u = pg.clone();
                tokio::spawn(async move {
                    let query = if clear_error {
                        "UPDATE tunnels SET status = $3, last_error = NULL, updated_at = now() WHERE id = $1 AND client_id = $2 AND status NOT IN ('deleted', 'suspended', 'pending_review')"
                    } else {
                        "UPDATE tunnels SET status = $3, last_error = $4, updated_at = now() WHERE id = $1 AND client_id = $2 AND status NOT IN ('deleted', 'suspended', 'pending_review')"
                    };
                    let mut q = sqlx::query(query)
                        .bind(tunnel_id)
                        .bind(client_id)
                        .bind(status);
                    if !clear_error {
                        q = q.bind(message);
                    }
                    let _ = q.execute(&pg_u).await;
                });
            }
            ControlOp::PortBindFailed { port, reason, .. } => {
                let Some(node_id) = authenticated else {
                    continue;
                };
                if control.kind != ControlKind::Node {
                    continue;
                };
                let pg_u = pg.clone();
                let reason = reason.clone();
                tokio::spawn(async move {
                    let _ = sqlx::query(
                        "UPDATE tunnels SET status = 'error', last_error = $2, updated_at = now() WHERE node_id = $1 AND remote_port = $3 AND status <> 'deleted'",
                    )
                    .bind(node_id)
                    .bind(&reason)
                    .bind(i32::from(port))
                    .execute(&pg_u)
                    .await;
                });
            }
            _ => {
                let _ = send_error(&tx, envelope.id, "暂不支持该控制操作").await;
            }
        }
    }

    if let Some(id) = authenticated {
        // 同一 node/client 重连后旧连接才结束时，不能把新会话摘掉或标记离线。
        let removed = SESSIONS
            .remove_if(&id, |_, session| session.tx.same_channel(&tx))
            .is_some();
        if removed || !SESSIONS.contains_key(&id) {
            let pg_off = pg.clone();
            let kind = control.kind;
            let app = control.app.clone();
            tokio::spawn(async move {
                mark_offline(&pg_off, kind, id, &app).await;
            });
        }
    }
    pump.abort();
}

async fn touch_last_seen(pg: &PgPool, kind: ControlKind, id: Uuid) {
    let query = match kind {
        ControlKind::Node => {
            "UPDATE nodes SET online = TRUE, last_seen_at = now(), updated_at = now() WHERE id = $1"
        }
        ControlKind::Agent => {
            "UPDATE clients SET online = TRUE, last_seen_at = now(), updated_at = now() WHERE id = $1"
        }
    };
    let _ = sqlx::query(query).bind(id).execute(pg).await;
}

struct BuiltConfig {
    node: Option<NodeConfig>,
    client: Option<ClientConfig>,
}

async fn build_config(pg: &PgPool, kind: ControlKind, id: Uuid) -> Result<BuiltConfig, String> {
    match kind {
        ControlKind::Node => {
            #[derive(sqlx::FromRow)]
            struct NodeRow {
                revision: i64,
                region: String,
                bind_addr: String,
                public_host: String,
                certificate_fingerprint: Option<String>,
                carrier_ports: serde_json::Value,
                tcp_port_ranges: serde_json::Value,
                udp_port_ranges: serde_json::Value,
                port_exclude: serde_json::Value,
                http_shared_port: i32,
                https_shared_port: i32,
                guard_policy: serde_json::Value,
                trusted_proxies: Vec<String>,
            }
            let node = sqlx::query_as::<_, NodeRow>(
                "SELECT config_revision AS revision, region, bind_addr, public_host, certificate_fingerprint, carrier_ports, tcp_port_ranges, udp_port_ranges, port_exclude, http_shared_port, https_shared_port, guard_policy, trusted_proxies::text[] AS trusted_proxies FROM nodes WHERE id = $1 AND enabled = TRUE",
            )
            .bind(id)
            .fetch_optional(pg)
            .await
            .map_err(|_| "读取节点配置失败".to_string())?
            .ok_or_else(|| "节点不存在或已禁用".to_string())?;
            let tunnels = load_node_tunnels(pg, id)
                .await
                .map_err(|_| "加载节点隧道列表失败".to_string())?;
            let http_domain_routes = load_http_domain_routes(pg, id)
                .await
                .map_err(|_| "加载 HTTP 域名路由失败".to_string())?;
            let tls_certificates = load_node_tls_certificates(pg, id)
                .await
                .map_err(|error| format!("加载节点 TLS 证书失败: {error}"))?;
            let board_ca_pem = crate::pki::load_ca_certificate_pem(pg)
                .await
                .map_err(|_| "读取 Board CA 失败".to_string())?;
            let mut authorized_client_fingerprints = load_authorized_client_fingerprints(pg, id)
                .await
                .map_err(|_| "加载 client 授权指纹失败".to_string())?;
            for tunnel in &tunnels {
                if let Some(fingerprint) = tunnel.client_fingerprint.as_ref() {
                    if !fingerprint.is_empty()
                        && !authorized_client_fingerprints.iter().any(|v| v == fingerprint)
                    {
                        authorized_client_fingerprints.push(fingerprint.clone());
                    }
                }
            }
            let host_metrics_interval_secs =
                crate::system_settings::host_metrics_interval_secs(pg).await;
            let guard_policy =
                crate::guard_policy::effective_node_guard_policy(pg, &node.guard_policy).await;
            let cn_http_filing = node.region.eq_ignore_ascii_case("CN")
                && crate::guard_policy::cn_http_filing_enabled(&guard_policy);
            let domain_whitelist = if cn_http_filing {
                sqlx::query_scalar::<_, String>("SELECT domain FROM domain_whitelist ORDER BY domain")
                    .fetch_all(pg)
                    .await
                    .map_err(|_| "加载域名过白名单失败".to_string())?
            } else {
                Vec::new()
            };
            Ok(BuiltConfig {
                node: Some(NodeConfig {
                    node_id: id,
                    revision: node.revision,
                    bind_addr: node.bind_addr,
                    public_host: node.public_host,
                    carrier_ports: node.carrier_ports,
                    tcp_port_ranges: node.tcp_port_ranges,
                    udp_port_ranges: node.udp_port_ranges,
                    port_exclude: node.port_exclude,
                    http_shared_port: node.http_shared_port,
                    https_shared_port: node.https_shared_port,
                    guard_policy,
                    cn_http_filing,
                    domain_whitelist,
                    trusted_proxies: node.trusted_proxies,
                    board_ca_pem,
                    authorized_client_fingerprints,
                    certificate_fingerprint: node.certificate_fingerprint,
                    tunnels,
                    http_domain_routes,
                    tls_certificates,
                    host_metrics_interval_secs,
                }),
                client: None,
            })
        }
        ControlKind::Agent => {
            let tunnels = load_client_tunnels(pg, id)
                .await
                .map_err(|_| "加载 Client 隧道分配失败".to_string())?;
            let board_ca_pem = crate::pki::load_ca_certificate_pem(pg)
                .await
                .map_err(|_| "读取 Board CA 失败".to_string())?;
            let client_fingerprint = sqlx::query_scalar::<_, Option<String>>(
                "SELECT certificate_fingerprint FROM clients WHERE id = $1",
            )
            .bind(id)
            .fetch_optional(pg)
            .await
            .map_err(|_| "读取 client 证书指纹失败".to_string())?
            .flatten();
            Ok(BuiltConfig {
                node: None,
                client: Some(ClientConfig {
                    client_id: id,
                    board_ca_pem,
                    certificate_fingerprint: client_fingerprint,
                    tunnels,
                }),
            })
        }
    }
}

const TUNNEL_DEDICATED_DOMAINS_SQL: &str = "COALESCE((SELECT array_agg(lower(td.domain) ORDER BY td.domain) FROM tunnel_domains td WHERE td.tunnel_id = t.id AND td.status = 'approved' AND t.http_access = 'dedicated'), ARRAY[]::text[]) AS domains";

const ACTIVE_SUBSCRIPTION_LATERAL: &str = "LEFT JOIN LATERAL (SELECT s.speed_limit_mbps, s.max_conns_per_tunnel, s.max_new_conns_per_sec FROM user_subscriptions s WHERE s.user_id = t.user_id AND s.status = 'active' AND s.starts_at <= now() AND (s.expires_at IS NULL OR s.expires_at > now()) AND s.exhausted_period_start IS NULL ORDER BY s.starts_at DESC LIMIT 1) sub ON TRUE";

#[derive(sqlx::FromRow)]
struct ClientTunnelRow {
    tunnel_id: Uuid,
    node_id: Uuid,
    node_public_host: String,
    node_region: String,
    node_guard_policy: serde_json::Value,
    carrier: String,
    carrier_port: i32,
    protocol: String,
    target_host: Option<String>,
    target_port: Option<i32>,
    target_url: Option<String>,
    host_rewrite: Option<String>,
    backend_tls_insecure: bool,
    node_cert_fingerprint: Option<String>,
}

#[derive(sqlx::FromRow)]
struct HttpDomainRow {
    domain: String,
    tunnel_id: Uuid,
    speed_limit_mbps: i64,
    max_conns: i32,
    client_fingerprint: Option<String>,
}

#[derive(sqlx::FromRow)]
struct NodeTlsRow {
    domain: String,
    cert_pem: String,
    private_key_pem: String,
}

async fn load_node_tls_certificates(
    pg: &PgPool,
    node_id: Uuid,
) -> Result<Vec<NodeTlsCertificate>, sqlx::Error> {
    // 共享 443 按访客实际访问的隧道域名挂证书。只按证书 SAN 精确匹配时，
    // 开启 HTTPS 后握手没有证书，浏览器就是 ERR_SSL_PROTOCOL_ERROR。
    let rows = sqlx::query_as::<_, NodeTlsRow>(
        "SELECT DISTINCT lower(cert_name.domain) AS domain, c.cert_pem, c.private_key_pem FROM tunnels t JOIN certificates c ON c.id = t.cert_id CROSS JOIN LATERAL unnest(c.domains || COALESCE((SELECT array_agg(td.domain) FROM tunnel_domains td WHERE td.tunnel_id = t.id AND td.status = 'approved'), ARRAY[]::text[])) AS cert_name(domain) WHERE t.node_id = $1 AND t.https_enabled = TRUE AND t.http_access = 'shared' AND t.enabled = TRUE AND NOT EXISTS (SELECT 1 FROM user_subscriptions qs WHERE qs.user_id = t.user_id AND qs.status = 'active' AND qs.exhausted_period_start IS NOT NULL) AND t.status NOT IN ('deleted', 'suspended', 'pending_review') AND c.not_after > now() AND cert_name.domain <> ''",
    )
    .bind(node_id)
    .fetch_all(pg)
    .await?;
    let mut grouped: Vec<NodeTlsCertificate> = Vec::new();
    for row in rows {
        if let Some(existing) = grouped.iter_mut().find(|item| {
            item.certificate_pem == row.cert_pem && item.private_key_pem == row.private_key_pem
        }) {
            if !existing.domains.iter().any(|domain| domain == &row.domain) {
                existing.domains.push(row.domain);
            }
        } else {
            grouped.push(NodeTlsCertificate {
                domains: vec![row.domain],
                certificate_pem: row.cert_pem,
                private_key_pem: row.private_key_pem,
            });
        }
    }
    Ok(grouped)
}

async fn load_http_domain_routes(
    pg: &PgPool,
    node_id: Uuid,
) -> Result<Vec<HttpDomainRoute>, sqlx::Error> {
    let sql = format!(
        "SELECT lower(td.domain) AS domain, t.id AS tunnel_id, COALESCE(sub.speed_limit_mbps, t.speed_limit_mbps) AS speed_limit_mbps, COALESCE(sub.max_conns_per_tunnel, t.max_conns) AS max_conns, c.certificate_fingerprint AS client_fingerprint FROM tunnel_domains td JOIN tunnels t ON t.id = td.tunnel_id LEFT JOIN clients c ON c.id = t.client_id {ACTIVE_SUBSCRIPTION_LATERAL} WHERE t.node_id = $1 AND t.http_access = 'shared' AND td.status = 'approved' AND t.enabled = TRUE AND NOT EXISTS (SELECT 1 FROM user_subscriptions qs WHERE qs.user_id = t.user_id AND qs.status = 'active' AND qs.exhausted_period_start IS NOT NULL) AND t.status NOT IN ('deleted', 'suspended', 'pending_review')"
    );
    let rows = sqlx::query_as::<_, HttpDomainRow>(&sql)
    .bind(node_id)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| HttpDomainRoute {
            domain: row.domain,
            tunnel_id: row.tunnel_id,
            client_fingerprint: row.client_fingerprint,
            speed_limit_mbps: row.speed_limit_mbps,
            max_conns: row.max_conns,
        })
        .collect())
}

async fn load_authorized_client_fingerprints(
    pg: &PgPool,
    node_id: Uuid,
) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT DISTINCT c.certificate_fingerprint FROM tunnels t JOIN clients c ON c.id = t.client_id WHERE t.node_id = $1 AND t.enabled = TRUE AND NOT EXISTS (SELECT 1 FROM user_subscriptions qs WHERE qs.user_id = t.user_id AND qs.status = 'active' AND qs.exhausted_period_start IS NOT NULL) AND t.status NOT IN ('deleted', 'suspended') AND c.enabled = TRUE AND c.certificate_fingerprint IS NOT NULL",
    )
    .bind(node_id)
    .fetch_all(pg)
    .await
}

async fn load_node_tunnels(pg: &PgPool, node_id: Uuid) -> Result<Vec<TunnelSpec>, sqlx::Error> {
    let sql = format!(
        "SELECT t.id AS tunnel_id, t.revision, t.protocol, t.carrier, t.remote_port, t.target_host, t.target_port, t.target_url, t.client_id, COALESCE(sub.speed_limit_mbps, t.speed_limit_mbps) AS speed_limit_mbps, COALESCE(sub.max_conns_per_tunnel, t.max_conns) AS max_conns, COALESCE(sub.max_new_conns_per_sec, t.max_new_conns_per_sec) AS max_new_conns_per_sec, c.certificate_fingerprint AS client_fingerprint, COALESCE(c.region, '') AS client_region, {TUNNEL_DEDICATED_DOMAINS_SQL} FROM tunnels t LEFT JOIN clients c ON c.id = t.client_id {ACTIVE_SUBSCRIPTION_LATERAL} WHERE t.node_id = $1 AND t.status NOT IN ('deleted', 'pending_review') AND t.enabled = TRUE AND NOT EXISTS (SELECT 1 FROM user_subscriptions qs WHERE qs.user_id = t.user_id AND qs.status = 'active' AND qs.exhausted_period_start IS NOT NULL)"
    );
    #[derive(sqlx::FromRow)]
    struct NodeTunnelLoadRow {
        tunnel_id: Uuid,
        revision: i64,
        protocol: String,
        carrier: String,
        remote_port: Option<i32>,
        target_host: Option<String>,
        target_port: Option<i32>,
        target_url: Option<String>,
        client_id: Uuid,
        speed_limit_mbps: i64,
        max_conns: i32,
        max_new_conns_per_sec: i32,
        client_fingerprint: Option<String>,
        client_region: String,
        domains: Vec<String>,
    }
    let node_meta: Option<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT region, COALESCE(guard_policy, '{}'::jsonb) FROM nodes WHERE id = $1",
    )
    .bind(node_id)
    .fetch_optional(pg)
    .await?;
    let Some((node_region, node_policy)) = node_meta else {
        return Ok(Vec::new());
    };
    let effective = crate::guard_policy::effective_node_guard_policy(pg, &node_policy).await;
    let rows = sqlx::query_as::<_, NodeTunnelLoadRow>(&sql)
        .bind(node_id)
        .fetch_all(pg)
        .await?;
    Ok(rows
        .into_iter()
        .filter(|row| {
            !crate::guard_policy::cn_residency_blocks_client(
                &effective,
                &node_region,
                &row.client_region,
            )
        })
        .map(|row| TunnelSpec {
            carrier_secret: crate::carrier_secret::for_carrier(row.tunnel_id, &row.carrier),
            tunnel_id: row.tunnel_id,
            revision: row.revision,
            protocol: row.protocol,
            carrier: row.carrier,
            remote_port: row.remote_port,
            target_host: row.target_host,
            target_port: row.target_port,
            target_url: row.target_url,
            client_id: Some(row.client_id),
            speed_limit_mbps: row.speed_limit_mbps,
            max_conns: row.max_conns,
            max_new_conns_per_sec: row.max_new_conns_per_sec,
            client_fingerprint: row.client_fingerprint,
            domains: row.domains,
        })
        .collect())
}

pub(crate) async fn load_client_tunnels(
    pg: &PgPool,
    client_id: Uuid,
) -> Result<Vec<ClientTunnelAssign>, sqlx::Error> {
    let client_region: String =
        sqlx::query_scalar("SELECT COALESCE(region, '') FROM clients WHERE id = $1")
            .bind(client_id)
            .fetch_one(pg)
            .await
            .unwrap_or_default();
    let rows = sqlx::query_as::<_, ClientTunnelRow>(
        "SELECT t.id AS tunnel_id, n.id AS node_id, n.public_host AS node_public_host, n.region AS node_region, COALESCE(n.guard_policy, '{}'::jsonb) AS node_guard_policy, t.carrier, (n.carrier_ports->t.carrier->>'port')::int AS carrier_port, t.protocol, t.target_host, t.target_port, t.target_url, t.host_rewrite, t.backend_tls_insecure, n.certificate_fingerprint AS node_cert_fingerprint FROM tunnels t JOIN nodes n ON n.id = t.node_id WHERE t.client_id = $1 AND t.enabled = TRUE AND NOT EXISTS (SELECT 1 FROM user_subscriptions qs WHERE qs.user_id = t.user_id AND qs.status = 'active' AND qs.exhausted_period_start IS NOT NULL) AND t.status NOT IN ('deleted', 'suspended', 'pending_review')",
    )
    .bind(client_id)
    .fetch_all(pg)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let effective =
            crate::guard_policy::effective_node_guard_policy(pg, &row.node_guard_policy).await;
        if crate::guard_policy::cn_residency_blocks_client(
            &effective,
            &row.node_region,
            &client_region,
        ) {
            continue;
        }
        out.push(ClientTunnelAssign {
            carrier_secret: crate::carrier_secret::for_carrier(row.tunnel_id, &row.carrier),
            tunnel_id: row.tunnel_id,
            node_id: row.node_id,
            node_public_host: row.node_public_host,
            carrier: row.carrier,
            carrier_port: u16::try_from(row.carrier_port).unwrap_or(7000),
            protocol: row.protocol,
            target_host: row.target_host,
            target_port: row.target_port,
            target_url: row.target_url,
            host_rewrite: row.host_rewrite,
            backend_tls_insecure: row.backend_tls_insecure,
            node_cert_fingerprint: row.node_cert_fingerprint,
        });
    }
    Ok(out)
}

async fn verify_token(
    pg: &PgPool,
    kind: ControlKind,
    token: &str,
) -> Result<Uuid, &'static str> {
    if token.len() < 16 || token.len() > 256 {
        return Err("token 无效");
    }
    let digest = Sha256::digest(token.as_bytes()).to_vec();
    match kind {
        ControlKind::Node => {
            let id: Option<Uuid> = sqlx::query_scalar(
                "SELECT id FROM nodes WHERE token_hash = $1 AND enabled = TRUE",
            )
            .bind(digest)
            .fetch_optional(pg)
            .await
            .map_err(|_| "数据库暂不可用")?;
            id.ok_or("节点 token 无效")
        }
        ControlKind::Agent => {
            let id: Option<Uuid> = sqlx::query_scalar(
                "SELECT id FROM clients WHERE token_hash = $1 AND enabled = TRUE",
            )
            .bind(digest)
            .fetch_optional(pg)
            .await
            .map_err(|_| "数据库暂不可用")?;
            id.ok_or("Client token 无效")
        }
    }
}

fn resolve_client_public_ip(hello: &HelloMessage, peer_ip: &str) -> String {
    hello
        .public_ipv4
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            hello
                .public_ipv6
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| peer_ip.trim().to_owned())
}

async fn persist_hello(
    pg: &PgPool,
    kind: ControlKind,
    id: Uuid,
    hello: &HelloMessage,
    peer_ip: &str,
) -> Result<(), String> {
    let capabilities = serde_json::to_value(&hello.capabilities).map_err(|_| "capabilities 无效")?;
    match kind {
        ControlKind::Node => {
            sqlx::query(
                "UPDATE nodes SET online = TRUE, capabilities = $2, version = $3, os = $4, last_seen_at = now(), updated_at = now() WHERE id = $1",
            )
            .bind(id)
            .bind(capabilities)
            .bind(&hello.version)
            .bind(&hello.os)
            .execute(pg)
            .await
            .map_err(|_| "无法更新节点状态".to_string())?;
            mark_tunnels_peer_online(pg, ControlKind::Node, id).await;
        }
        ControlKind::Agent => {
            let public_ip = resolve_client_public_ip(hello, peer_ip);
            sqlx::query(
                "UPDATE clients SET online = TRUE, capabilities = $2, version = $3, os = $4, arch = $5, public_ip = $6, last_seen_at = now(), updated_at = now() WHERE id = $1",
            )
            .bind(id)
            .bind(capabilities)
            .bind(&hello.version)
            .bind(&hello.os)
            .bind(&hello.arch)
            .bind(&public_ip)
            .execute(pg)
            .await
            .map_err(|_| "无法更新 Client 状态".to_string())?;
            mark_tunnels_peer_online(pg, ControlKind::Agent, id).await;
            let pg = pg.clone();
            let ip = public_ip.clone();
            tokio::spawn(async move {
                crate::geoip::refresh_client_region(&pg, id, &ip).await;
            });
        }
    }
    Ok(())
}

async fn persist_certificate(
    pg: &PgPool,
    kind: ControlKind,
    id: Uuid,
    fingerprint: &str,
) {
    let query = match kind {
        ControlKind::Node => {
            "UPDATE nodes SET certificate_fingerprint = $2, updated_at = now() WHERE id = $1"
        }
        ControlKind::Agent => {
            "UPDATE clients SET certificate_fingerprint = $2, updated_at = now() WHERE id = $1"
        }
    };
    if let Err(error) = sqlx::query(query)
        .bind(id)
        .bind(fingerprint)
        .execute(pg)
        .await
    {
        warn!(?kind, %id, %fingerprint, %error, "persist_certificate update failed");
        return;
    }
    match kind {
        ControlKind::Agent => {
            let Ok(node_ids) = sqlx::query_scalar::<_, Uuid>(
                "SELECT DISTINCT node_id FROM tunnels WHERE client_id = $1 AND status NOT IN ('deleted') AND enabled = TRUE",
            )
            .bind(id)
            .fetch_all(pg)
            .await
            else {
                return;
            };
            for node_id in node_ids {
                push_full_node_config(pg, node_id).await;
            }
            push_client_config(pg, id).await;
        }
        ControlKind::Node => {
            let Ok(client_ids) = sqlx::query_scalar::<_, Uuid>(
                "SELECT DISTINCT client_id FROM tunnels WHERE node_id = $1 AND client_id IS NOT NULL AND status NOT IN ('deleted') AND enabled = TRUE",
            )
            .bind(id)
            .fetch_all(pg)
            .await
            else {
                return;
            };
            for client_id in client_ids {
                push_client_config(pg, client_id).await;
            }
        }
    }
}

/// 控制面重连后，把因对端离线而挂起的隧道拉回 provisioning，便于 client/node 继续建 carrier。
async fn mark_tunnels_peer_online(pg: &PgPool, kind: ControlKind, id: Uuid) {
    match kind {
        ControlKind::Node => {
            let _ = sqlx::query(
                "UPDATE tunnels SET status = 'provisioning', last_error = NULL, updated_at = now() WHERE node_id = $1 AND enabled = TRUE AND status = 'node_offline'",
            )
            .bind(id)
            .execute(pg)
            .await;
        }
        ControlKind::Agent => {
            let _ = sqlx::query(
                "UPDATE tunnels SET status = 'provisioning', last_error = NULL, updated_at = now() WHERE client_id = $1 AND enabled = TRUE AND status = 'client_offline'",
            )
            .bind(id)
            .execute(pg)
            .await;
        }
    }
}

async fn mark_offline(pg: &PgPool, kind: ControlKind, id: Uuid, app: &AppState) {
    let query = match kind {
        ControlKind::Node => "UPDATE nodes SET online = FALSE, updated_at = now() WHERE id = $1",
        ControlKind::Agent => "UPDATE clients SET online = FALSE, updated_at = now() WHERE id = $1",
    };
    let _ = sqlx::query(query).bind(id).execute(pg).await;
    match kind {
        ControlKind::Node => {
            crate::node_metrics::clear(id);
            if let Some(mut redis) = app.redis_connection() {
                crate::node_metrics::clear_redis(&mut redis, id).await;
            }
            let _ = sqlx::query(
                "UPDATE tunnels SET status = 'node_offline', last_error = 'node disconnected', updated_at = now() WHERE node_id = $1 AND enabled = TRUE AND status NOT IN ('deleted', 'suspended', 'pending_review')",
            )
            .bind(id)
            .execute(pg)
            .await;
            let Ok(client_ids) = sqlx::query_scalar::<_, Uuid>(
                "SELECT DISTINCT client_id FROM tunnels WHERE node_id = $1 AND client_id IS NOT NULL AND enabled = TRUE AND status NOT IN ('deleted', 'suspended', 'pending_review')",
            )
            .bind(id)
            .fetch_all(pg)
            .await
            else {
                return;
            };
            for client_id in client_ids {
                push_client_config(pg, client_id).await;
            }
        }
        ControlKind::Agent => {
            crate::client_traffic::clear(id);
            let _ = sqlx::query(
                "UPDATE tunnels SET status = 'client_offline', last_error = 'client disconnected', updated_at = now() WHERE client_id = $1 AND enabled = TRUE AND status NOT IN ('deleted', 'suspended', 'pending_review')",
            )
            .bind(id)
            .execute(pg)
            .await;
            let Ok(node_ids) = sqlx::query_scalar::<_, Uuid>(
                "SELECT DISTINCT node_id FROM tunnels WHERE client_id = $1 AND enabled = TRUE AND status NOT IN ('deleted', 'suspended', 'pending_review')",
            )
            .bind(id)
            .fetch_all(pg)
            .await
            else {
                return;
            };
            for node_id in node_ids {
                push_full_node_config(pg, node_id).await;
            }
        }
    }
}

async fn ready_pool(app: &AppState) -> Option<PgPool> {
    app.pg_control()
}

async fn send_response(tx: &mpsc::Sender<Vec<u8>>, id: u64, payload: ControlOp) -> bool {
    let envelope = Envelope {
        version: ProtocolVersion::CURRENT,
        id,
        kind: MessageKind::Response,
        payload,
    };
    send_envelope(tx, envelope).await
}

async fn send_error(tx: &mpsc::Sender<Vec<u8>>, id: u64, message: &str) -> bool {
    send_response(
        tx,
        id,
        ControlOp::Error(ErrorPayload {
            code: "control_error".into(),
            message: message.into(),
            retryable: false,
        }),
    )
    .await
}

async fn send_envelope(tx: &mpsc::Sender<Vec<u8>>, envelope: Envelope<ControlOp>) -> bool {
    let bytes = match rmp_serde::to_vec_named(&envelope) {
        Ok(bytes) => bytes,
        Err(_) => return false,
    };
    match tx.try_send(bytes) {
        Ok(()) => true,
        Err(mpsc::error::TrySendError::Full(_)) => false,
        Err(mpsc::error::TrySendError::Closed(_)) => false,
    }
}

pub async fn push_node_configs_to_online_nodes(pg: &PgPool) {
    let node_ids: Vec<Uuid> = SESSIONS
        .iter()
        .filter(|session| session.kind == ControlKind::Node)
        .map(|session| *session.key())
        .collect();
    for node_id in node_ids {
        push_full_node_config(pg, node_id).await;
    }
}

pub async fn push_full_node_config(pg: &PgPool, node_id: Uuid) {
    match build_config(pg, ControlKind::Node, node_id).await {
        Ok(BuiltConfig {
            node: Some(config), ..
        }) => {
            push_node_op(node_id, ControlOp::NodeConfig(config)).await;
        }
        Ok(_) => {}
        Err(error) => {
            warn!(%node_id, %error, "push_full_node_config failed");
        }
    }
}

#[derive(Debug, sqlx::FromRow)]
struct TunnelPushTarget {
    id: Uuid,
    node_id: Uuid,
    client_id: Uuid,
}

/// 将当前有效订阅的限速/并发快照写入该用户全部隧道，并推送到在线 node/client。
pub async fn sync_user_tunnel_limits_from_active_subscription(pg: &PgPool, user_id: Uuid) {
    #[derive(sqlx::FromRow)]
    struct SubLimits {
        speed_limit_mbps: i64,
        max_conns_per_tunnel: i32,
        max_new_conns_per_sec: i32,
    }
    let limits = match sqlx::query_as::<_, SubLimits>(
        "SELECT speed_limit_mbps, max_conns_per_tunnel, max_new_conns_per_sec FROM user_subscriptions WHERE user_id = $1 AND status = 'active' AND starts_at <= now() AND (expires_at IS NULL OR expires_at > now()) AND exhausted_period_start IS NULL ORDER BY starts_at DESC LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(pg)
    .await
    {
        Ok(Some(limits)) => limits,
        Ok(None) => return,
        Err(error) => {
            warn!(%user_id, %error, "load active subscription limits failed");
            return;
        }
    };

    let targets = match sqlx::query_as::<_, TunnelPushTarget>(
        "UPDATE tunnels SET speed_limit_mbps = $2, max_conns = $3, max_new_conns_per_sec = $4, revision = revision + 1, updated_at = now() WHERE user_id = $1 AND status <> 'deleted' RETURNING id, node_id, client_id",
    )
    .bind(user_id)
    .bind(limits.speed_limit_mbps)
    .bind(limits.max_conns_per_tunnel)
    .bind(limits.max_new_conns_per_sec)
    .fetch_all(pg)
    .await
    {
        Ok(rows) => rows,
        Err(error) => {
            warn!(%user_id, %error, "apply subscription limits to tunnels failed");
            return;
        }
    };

    for row in targets {
        on_tunnel_changed(pg, row.node_id, row.client_id, row.id).await;
    }
}

/// 在控制面连接池上异步下发，避免阻塞 Web API 请求。
pub async fn retarget_tunnel(
    pg: &PgPool,
    old_node_id: Uuid,
    old_client_id: Uuid,
    new_node_id: Uuid,
    new_client_id: Uuid,
    tunnel_id: Uuid,
) {
    if old_node_id != new_node_id {
        let _ = push_node_op(old_node_id, ControlOp::TunnelRemove { tunnel_id }).await;
        push_full_node_config(pg, old_node_id).await;
    }
    if old_client_id != new_client_id {
        push_client_config(pg, old_client_id).await;
    }
    on_tunnel_changed(pg, new_node_id, new_client_id, tunnel_id).await;
}

pub fn defer_on_tunnel_changed(pg: PgPool, node_id: Uuid, client_id: Uuid, tunnel_id: Uuid) {
    tokio::spawn(async move {
        on_tunnel_changed(&pg, node_id, client_id, tunnel_id).await;
    });
}

pub async fn on_tunnel_changed(pg: &PgPool, node_id: Uuid, client_id: Uuid, tunnel_id: Uuid) {
    // 创建/修改/恢复：统一轮换最新密钥，先下发 node，再下发 client（client 主动连）。
    crate::carrier_secret::rotate(tunnel_id);
    let _ = sqlx::query(
        "UPDATE nodes SET config_revision = config_revision + 1, updated_at = now() WHERE id = $1",
    )
    .bind(node_id)
    .execute(pg)
    .await;
    if let Ok(Some(spec)) = load_tunnel_spec(pg, node_id, tunnel_id).await {
        push_node_op(node_id, ControlOp::TunnelUpsert(spec)).await;
    }
    push_full_node_config(pg, node_id).await;
    push_client_config(pg, client_id).await;
}

pub async fn push_tunnel_suspend(pg: &PgPool, node_id: Uuid, client_id: Uuid, tunnel_id: Uuid) {
    if !push_node_op(node_id, ControlOp::TunnelSuspend { tunnel_id }).await {
        warn!(%node_id, %tunnel_id, "node outbound queue full; session dropped");
    }
    let _ = push_agent_op(client_id, ControlOp::TunnelSuspend { tunnel_id }).await;
    push_full_node_config(pg, node_id).await;
    push_client_config(pg, client_id).await;
}

/// client 重连：轮换其全部 TCP 隧道密钥，先推给 node，再由调用方把新配置回给 client。
async fn rotate_client_carrier_secrets(pg: &PgPool, client_id: Uuid) {
    let Ok(rows) = sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT id, node_id FROM tunnels WHERE client_id = $1 AND carrier = 'tcp' AND enabled = TRUE AND status NOT IN ('deleted')",
    )
    .bind(client_id)
    .fetch_all(pg)
    .await
    else {
        return;
    };
    let mut node_ids = Vec::new();
    for (tunnel_id, node_id) in rows {
        crate::carrier_secret::rotate(tunnel_id);
        if !node_ids.contains(&node_id) {
            node_ids.push(node_id);
        }
    }
    for node_id in node_ids {
        push_full_node_config(pg, node_id).await;
    }
}

pub async fn push_tunnel_resume(pg: &PgPool, node_id: Uuid, client_id: Uuid, tunnel_id: Uuid) {
    let _ = push_node_op(node_id, ControlOp::TunnelResume { tunnel_id }).await;
    on_tunnel_changed(pg, node_id, client_id, tunnel_id).await;
}

/// 从 node/client 运行时摘掉隧道（关闭、暂停、删除等），无需重启进程。
pub async fn sync_tunnel_offline(pg: &PgPool, node_id: Uuid, client_id: Uuid, tunnel_id: Uuid) {
    crate::carrier_secret::forget(tunnel_id);
    if !push_node_op(node_id, ControlOp::TunnelRemove { tunnel_id }).await {
        warn!(%node_id, %tunnel_id, "node outbound queue full; session dropped");
    }
    let _ = push_agent_op(client_id, ControlOp::TunnelRemove { tunnel_id }).await;
    push_full_node_config(pg, node_id).await;
    push_client_config(pg, client_id).await;
}

/// 重新下发隧道到 node/client（开启、恢复等）。
pub async fn sync_tunnel_online(pg: &PgPool, node_id: Uuid, client_id: Uuid, tunnel_id: Uuid) {
    on_tunnel_changed(pg, node_id, client_id, tunnel_id).await;
}

pub async fn on_tunnel_removed(pg: &PgPool, node_id: Uuid, client_id: Uuid, tunnel_id: Uuid) {
    let _ = sqlx::query(
        "UPDATE nodes SET config_revision = config_revision + 1, updated_at = now() WHERE id = $1",
    )
    .bind(node_id)
    .execute(pg)
    .await;
    sync_tunnel_offline(pg, node_id, client_id, tunnel_id).await;
}

pub(crate) async fn notify_stats_ack(node_id: Uuid, seq: u64) {
    push_node_op(node_id, ControlOp::StatsAck { seq }).await;
}

pub async fn push_client_config(pg: &PgPool, client_id: Uuid) {
    let Ok(tunnels) = load_client_tunnels(pg, client_id).await else {
        return;
    };
    let Ok(board_ca_pem) = crate::pki::load_ca_certificate_pem(pg).await else {
        return;
    };
    let client_fingerprint = sqlx::query_scalar::<_, Option<String>>(
        "SELECT certificate_fingerprint FROM clients WHERE id = $1",
    )
    .bind(client_id)
    .fetch_optional(pg)
    .await
    .ok()
    .flatten()
    .flatten();
    push_agent_op(
        client_id,
        ControlOp::ClientConfig(ClientConfig {
            client_id,
            board_ca_pem,
            certificate_fingerprint: client_fingerprint,
            tunnels,
        }),
    )
    .await;
}

async fn push_node_op(node_id: Uuid, op: ControlOp) -> bool {
    push_op(node_id, ControlKind::Node, op).await
}

async fn push_agent_op(client_id: Uuid, op: ControlOp) -> bool {
    push_op(client_id, ControlKind::Agent, op).await
}

async fn push_op(id: Uuid, kind: ControlKind, op: ControlOp) -> bool {
    let Some(tx) = SESSIONS
        .get(&id)
        .filter(|session| session.kind == kind)
        .map(|session| session.tx.clone())
    else {
        return false;
    };
    let ok = send_event(tx.clone(), op).await;
    if !ok && tx.is_closed() {
        SESSIONS.remove_if(&id, |_, session| session.tx.same_channel(&tx));
    }
    ok
}

async fn send_event(tx: mpsc::Sender<Vec<u8>>, op: ControlOp) -> bool {
    let envelope = Envelope {
        version: ProtocolVersion::CURRENT,
        id: 0,
        kind: MessageKind::Event,
        payload: op,
    };
    send_envelope(&tx, envelope).await
}

async fn load_tunnel_spec(
    pg: &PgPool,
    node_id: Uuid,
    tunnel_id: Uuid,
) -> Result<Option<TunnelSpec>, sqlx::Error> {
    let sql = format!(
        "SELECT t.id AS tunnel_id, t.revision, t.protocol, t.carrier, t.remote_port, t.target_host, t.target_port, t.target_url, t.client_id, COALESCE(sub.speed_limit_mbps, t.speed_limit_mbps) AS speed_limit_mbps, COALESCE(sub.max_conns_per_tunnel, t.max_conns) AS max_conns, COALESCE(sub.max_new_conns_per_sec, t.max_new_conns_per_sec) AS max_new_conns_per_sec, c.certificate_fingerprint AS client_fingerprint, COALESCE(c.region, '') AS client_region, {TUNNEL_DEDICATED_DOMAINS_SQL} FROM tunnels t LEFT JOIN clients c ON c.id = t.client_id {ACTIVE_SUBSCRIPTION_LATERAL} WHERE t.node_id = $1 AND t.id = $2 AND t.status NOT IN ('deleted', 'pending_review') AND t.enabled = TRUE AND NOT EXISTS (SELECT 1 FROM user_subscriptions qs WHERE qs.user_id = t.user_id AND qs.status = 'active' AND qs.exhausted_period_start IS NOT NULL)"
    );
    #[derive(sqlx::FromRow)]
    struct SpecRow {
        tunnel_id: Uuid,
        revision: i64,
        protocol: String,
        carrier: String,
        remote_port: Option<i32>,
        target_host: Option<String>,
        target_port: Option<i32>,
        target_url: Option<String>,
        client_id: Uuid,
        speed_limit_mbps: i64,
        max_conns: i32,
        max_new_conns_per_sec: i32,
        client_fingerprint: Option<String>,
        client_region: String,
        domains: Vec<String>,
    }
    let row = sqlx::query_as::<_, SpecRow>(&sql)
        .bind(node_id)
        .bind(tunnel_id)
        .fetch_optional(pg)
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let node_meta: Option<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT region, COALESCE(guard_policy, '{}'::jsonb) FROM nodes WHERE id = $1",
    )
    .bind(node_id)
    .fetch_optional(pg)
    .await?;
    if let Some((node_region, node_policy)) = node_meta {
        let effective = crate::guard_policy::effective_node_guard_policy(pg, &node_policy).await;
        if crate::guard_policy::cn_residency_blocks_client(
            &effective,
            &node_region,
            &row.client_region,
        ) {
            return Ok(None);
        }
    }
    Ok(Some(TunnelSpec {
        carrier_secret: crate::carrier_secret::for_carrier(row.tunnel_id, &row.carrier),
        tunnel_id: row.tunnel_id,
        revision: row.revision,
        protocol: row.protocol,
        carrier: row.carrier,
        remote_port: row.remote_port,
        target_host: row.target_host,
        target_port: row.target_port,
        target_url: row.target_url,
        client_id: Some(row.client_id),
        speed_limit_mbps: row.speed_limit_mbps,
        max_conns: row.max_conns,
        max_new_conns_per_sec: row.max_new_conns_per_sec,
        client_fingerprint: row.client_fingerprint,
        domains: row.domains,
    }))
}

