use super::{database_error, response_error, write_audit};
use crate::{
    port_cache::{PortCacheError, PortProtocol, RedisPortCache, reservation_token},
    setup::{AppState, UserRole, unavailable},
    store,
};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get, patch, post, put},
};
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{FromRow, PgPool};
use std::{
    collections::{HashMap, HashSet},
    net::IpAddr,
    str::FromStr,
    sync::Arc,
};
use tz_common::ports::{PortRange, PortRanges};
use url::Url;
use uuid::Uuid;

pub fn admin_router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/v1/admin/tunnels",
            get(list_admin_tunnels).post(admin_create_tunnel),
        )
        .route(
            "/api/v1/admin/tunnels/{tunnel_id}",
            put(admin_update_tunnel).delete(admin_delete_tunnel),
        )
        .route(
            "/api/v1/admin/tunnels/{tunnel_id}/enabled",
            patch(admin_set_tunnel_enabled),
        )
}

pub fn user_router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/v1/tunnels",
            get(list_user_tunnels).post(create_tunnel),
        )
        .route(
            "/api/v1/tunnels/{tunnel_id}",
            get(get_user_tunnel)
                .delete(user_delete_tunnel),
        )
        .route(
            "/api/v1/tunnels/{tunnel_id}/suspend",
            post(user_suspend_tunnel),
        )
        .route(
            "/api/v1/tunnels/{tunnel_id}/resume",
            post(user_resume_tunnel),
        )
        .route("/api/v1/tunnels/{tunnel_id}/close", post(user_close_tunnel))
        .route("/api/v1/tunnels/{tunnel_id}/open", post(user_open_tunnel))
}

#[derive(Debug, Serialize, FromRow)]
struct TunnelRow {
    id: Uuid,
    user_id: Uuid,
    user_email: Option<String>,
    client_id: Uuid,
    client_name: Option<String>,
    node_id: Uuid,
    node_name: Option<String>,
    name: String,
    carrier: String,
    protocol: String,
    remote_port: Option<i32>,
    target_host: Option<String>,
    target_port: Option<i32>,
    target_url: Option<String>,
    status: String,
    enabled: bool,
    speed_limit_mbps: i64,
    max_conns: i32,
    max_new_conns_per_sec: i32,
    last_error: Option<String>,
}

#[derive(Debug, Serialize)]
struct TunnelTrafficMetrics {
    bytes_in: u64,
    bytes_out: u64,
    in_bps: u64,
    out_bps: u64,
}

#[derive(Debug, Serialize)]
struct AdminTunnelListItem {
    #[serde(flatten)]
    tunnel: TunnelRow,
    https_enabled: bool,
    cert_id: Option<Uuid>,
    domains: Vec<String>,
    http_access: Option<String>,
    host_rewrite: Option<String>,
    backend_tls_insecure: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    traffic_metrics: Option<TunnelTrafficMetrics>,
}

#[derive(Debug, FromRow)]
struct TunnelHttpsFields {
    id: Uuid,
    https_enabled: bool,
    cert_id: Option<Uuid>,
    domains: Vec<String>,
    http_access: Option<String>,
    host_rewrite: Option<String>,
    backend_tls_insecure: bool,
}

#[derive(Debug, Serialize)]
struct PaginatedAdminTunnels {
    items: Vec<AdminTunnelListItem>,
    total: i64,
    page: u32,
    page_size: u32,
}

#[derive(Debug, Deserialize)]
struct AdminListTunnelsQuery {
    node_id: Uuid,
    #[serde(default = "default_list_page")]
    page: u32,
    #[serde(default = "default_list_page_size")]
    page_size: u32,
}

fn default_list_page() -> u32 {
    1
}

fn default_list_page_size() -> u32 {
    20
}

const ADMIN_TUNNEL_PAGE_SIZE_MAX: u32 = 100;
/// 限制 OFFSET 深度，避免在超大数据集上扫描过多索引项。
const ADMIN_TUNNEL_MAX_OFFSET: i64 = 2_000_000;

const ADMIN_TUNNEL_LIST_SQL: &str = "SELECT t.id, t.user_id, u.email AS user_email, t.client_id, c.name AS client_name, t.node_id, n.name AS node_name, t.name, t.carrier, t.protocol, t.remote_port, t.target_host, t.target_port, t.target_url, t.status, t.enabled, t.speed_limit_mbps, t.max_conns, t.max_new_conns_per_sec, t.last_error FROM tunnels t JOIN users u ON u.id = t.user_id JOIN clients c ON c.id = t.client_id JOIN nodes n ON n.id = t.node_id WHERE t.node_id = $1 AND t.status <> 'deleted' ORDER BY t.created_at DESC, t.id DESC";

#[derive(Debug, Deserialize)]
struct SetTunnelEnabled {
    enabled: bool,
}

#[derive(Debug, Deserialize)]
struct AdminUpdateTunnel {
    name: Option<String>,
    client_id: Option<Uuid>,
    node_id: Option<Uuid>,
    /// 缺省表示不改；`null` 表示清空并自动分配；数字表示指定端口。
    #[serde(default)]
    remote_port: Option<Option<u16>>,
    target_host: Option<String>,
    target_port: Option<u16>,
    target_url: Option<String>,
    http_access: Option<String>,
    domains: Option<Vec<String>>,
    https_enabled: Option<bool>,
    cert_id: Option<Uuid>,
    host_rewrite: Option<String>,
    backend_tls_insecure: Option<bool>,
    speed_limit_mbps: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct AdminCreateTunnel {
    client_id: Uuid,
    node_id: Uuid,
    name: String,
    carrier: String,
    protocol: String,
    remote_port: Option<u16>,
    target_host: Option<String>,
    target_port: Option<u16>,
    target_url: Option<String>,
    http_access: Option<String>,
    domains: Option<Vec<String>>,
    https_enabled: Option<bool>,
    cert_id: Option<Uuid>,
    host_rewrite: Option<String>,
    backend_tls_insecure: Option<bool>,
}

#[derive(Debug, Serialize, FromRow)]
struct TunnelDetailRow {
    id: Uuid,
    user_id: Uuid,
    client_id: Uuid,
    client_name: Option<String>,
    node_id: Uuid,
    node_name: Option<String>,
    name: String,
    carrier: String,
    protocol: String,
    remote_port: Option<i32>,
    target_host: Option<String>,
    target_port: Option<i32>,
    target_url: Option<String>,
    http_access: Option<String>,
    https_enabled: bool,
    status: String,
    enabled: bool,
    speed_limit_mbps: i64,
    max_conns: i32,
    max_new_conns_per_sec: i32,
    last_error: Option<String>,
}

#[derive(Debug, Serialize)]
struct TunnelDetailResponse {
    #[serde(flatten)]
    tunnel: TunnelDetailRow,
    domains: Vec<String>,
}

#[derive(FromRow)]
struct SubscriptionLimits {
    id: Uuid,
    speed_limit_mbps: i64,
    max_conns_per_tunnel: i32,
    max_new_conns_per_sec: i32,
    max_tunnels: i32,
    allow_custom_port: bool,
    allowed_protocols: Vec<String>,
}

#[derive(FromRow)]
struct TunnelNode {
    region: String,
    capabilities: Value,
    protocols: Vec<String>,
    carrier_ports: Value,
    tcp_port_ranges: Value,
    udp_port_ranges: Value,
    port_exclude: Value,
    http_shared_port: i32,
    https_shared_port: i32,
    guard_policy: Value,
}

#[derive(Deserialize)]
struct CreateTunnel {
    name: String,
    node_id: Uuid,
    client_id: Uuid,
    carrier: String,
    protocol: String,
    remote_port: Option<u16>,
    target_host: Option<String>,
    target_port: Option<u16>,
    target_url: Option<String>,
    http_access: Option<String>,
    domains: Option<Vec<String>>,
    https_enabled: Option<bool>,
    cert_id: Option<Uuid>,
    host_rewrite: Option<String>,
    backend_tls_insecure: Option<bool>,
}

async fn list_admin_tunnels(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<AdminListTunnelsQuery>,
) -> Result<Json<PaginatedAdminTunnels>, (StatusCode, Json<Value>)> {
    let (pg, _) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    let page = query.page.max(1);
    let page_size = query.page_size.clamp(1, ADMIN_TUNNEL_PAGE_SIZE_MAX);
    let offset = i64::from(page - 1) * i64::from(page_size);
    if offset > ADMIN_TUNNEL_MAX_OFFSET {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "页码过大，请提高每页条数或使用更靠前的页",
        )));
    }
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM tunnels t WHERE t.node_id = $1 AND t.status <> 'deleted'",
    )
    .bind(query.node_id)
    .fetch_one(&pg)
    .await
    .map_err(database_error)?;
    let list_sql = format!("{ADMIN_TUNNEL_LIST_SQL} LIMIT $2 OFFSET $3");
    let rows = sqlx::query_as::<_, TunnelRow>(&list_sql)
        .bind(query.node_id)
        .bind(i64::from(page_size))
        .bind(offset)
        .fetch_all(&pg)
        .await
        .map_err(database_error)?;
    let mut items = attach_tunnel_traffic_metrics(&pg, rows).await;
    let ids: Vec<Uuid> = items.iter().map(|item| item.tunnel.id).collect();
    if !ids.is_empty() {
        let extras = sqlx::query_as::<_, TunnelHttpsFields>(
            "SELECT t.id, t.https_enabled, t.cert_id, COALESCE((SELECT array_agg(td.domain ORDER BY td.domain) FROM tunnel_domains td WHERE td.tunnel_id = t.id AND td.status IN ('approved', 'pending_review')), ARRAY[]::text[]) AS domains, t.http_access, t.host_rewrite, t.backend_tls_insecure FROM tunnels t WHERE t.id = ANY($1)",
        )
        .bind(&ids)
        .fetch_all(&pg)
        .await
        .map_err(database_error)?;
        let extras: HashMap<Uuid, TunnelHttpsFields> =
            extras.into_iter().map(|row| (row.id, row)).collect();
        for item in &mut items {
            if let Some(extra) = extras.get(&item.tunnel.id) {
                item.https_enabled = extra.https_enabled;
                item.cert_id = extra.cert_id;
                item.domains.clone_from(&extra.domains);
                item.http_access.clone_from(&extra.http_access);
                item.host_rewrite.clone_from(&extra.host_rewrite);
                item.backend_tls_insecure = extra.backend_tls_insecure;
            }
        }
    }
    Ok(Json(PaginatedAdminTunnels {
        items,
        total,
        page,
        page_size,
    }))
}

async fn attach_tunnel_traffic_metrics(
    pg: &PgPool,
    rows: Vec<TunnelRow>,
) -> Vec<AdminTunnelListItem> {
    let ids: Vec<Uuid> = rows.iter().map(|row| row.id).collect();
    let hourly = load_hourly_traffic_totals(pg, &ids).await;
    rows.into_iter()
        .map(|tunnel| {
            let live = crate::tunnel_traffic::snapshot(tunnel.id);
            let (pending_in, pending_out) = crate::stats::pending_traffic_bytes(tunnel.id);
            let (hourly_in, hourly_out) = hourly
                .get(&tunnel.id)
                .copied()
                .unwrap_or((0, 0));
            let bytes_in = hourly_in.saturating_add(pending_in);
            let bytes_out = hourly_out.saturating_add(pending_out);
            let traffic_metrics = TunnelTrafficMetrics {
                bytes_in: u64::try_from(bytes_in.max(0)).unwrap_or(u64::MAX),
                bytes_out: u64::try_from(bytes_out.max(0)).unwrap_or(u64::MAX),
                in_bps: live.as_ref().map(|sample| sample.in_bps).unwrap_or(0),
                out_bps: live.as_ref().map(|sample| sample.out_bps).unwrap_or(0),
            };
            AdminTunnelListItem {
                tunnel,
                https_enabled: false,
                cert_id: None,
                domains: Vec::new(),
                http_access: None,
                host_rewrite: None,
                backend_tls_insecure: false,
                traffic_metrics: Some(traffic_metrics),
            }
        })
        .collect()
}

async fn load_hourly_traffic_totals(
    pg: &PgPool,
    tunnel_ids: &[Uuid],
) -> HashMap<Uuid, (i64, i64)> {
    if tunnel_ids.is_empty() {
        return HashMap::new();
    }
    let rows = sqlx::query_as::<_, (Uuid, i64, i64)>(
        "SELECT tunnel_id, COALESCE(SUM(bytes_in), 0)::bigint, COALESCE(SUM(bytes_out), 0)::bigint FROM tunnel_traffic_hourly WHERE tunnel_id = ANY($1) GROUP BY tunnel_id",
    )
    .bind(tunnel_ids)
    .fetch_all(pg)
    .await
    .unwrap_or_default();
    rows.into_iter()
        .map(|(tunnel_id, bytes_in, bytes_out)| (tunnel_id, (bytes_in, bytes_out)))
        .collect()
}

async fn list_user_tunnels(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(page): Query<crate::page::PageQuery>,
) -> Result<Json<crate::page::Page<TunnelRow>>, (StatusCode, Json<Value>)> {
    let (pg, user) = state
        .database_for(&headers, UserRole::User)
        .await
        .map_err(response_error)?;
    let (page, page_size, offset) = page.resolve();
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM tunnels WHERE user_id = $1 AND status <> 'deleted'",
    )
    .bind(user.user_id)
    .fetch_one(&pg)
    .await
    .map_err(database_error)?;
    let rows = sqlx::query_as::<_, TunnelRow>(
        "SELECT t.id, t.user_id, NULL::text AS user_email, t.client_id, c.name AS client_name, t.node_id, n.name AS node_name, t.name, t.carrier, t.protocol, t.remote_port, t.target_host, t.target_port, t.target_url, t.status, t.enabled, t.speed_limit_mbps, t.max_conns, t.max_new_conns_per_sec, t.last_error FROM tunnels t JOIN clients c ON c.id = t.client_id JOIN nodes n ON n.id = t.node_id WHERE t.user_id = $1 AND t.status <> 'deleted' ORDER BY t.created_at DESC LIMIT $2 OFFSET $3",
    )
    .bind(user.user_id)
    .bind(page_size)
    .bind(offset)
    .fetch_all(&pg)
    .await
    .map_err(database_error)?;
    Ok(Json(crate::page::Page::new(rows, total, page, page_size)))
}

async fn get_user_tunnel(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(tunnel_id): Path<Uuid>,
) -> Result<Json<TunnelDetailResponse>, (StatusCode, Json<Value>)> {
    let (pg, user) = state
        .database_for(&headers, UserRole::User)
        .await
        .map_err(response_error)?;
    let tunnel = sqlx::query_as::<_, TunnelDetailRow>(
        "SELECT t.id, t.user_id, t.client_id, c.name AS client_name, t.node_id, n.name AS node_name, t.name, t.carrier, t.protocol, t.remote_port, t.target_host, t.target_port, t.target_url, t.http_access, t.https_enabled, t.status, t.enabled, t.speed_limit_mbps, t.max_conns, t.max_new_conns_per_sec, t.last_error FROM tunnels t JOIN clients c ON c.id = t.client_id JOIN nodes n ON n.id = t.node_id WHERE t.id = $1 AND t.user_id = $2 AND t.status <> 'deleted'",
    )
    .bind(tunnel_id)
    .bind(user.user_id)
    .fetch_optional(&pg)
    .await
    .map_err(database_error)?
    .ok_or_else(|| response_error(unavailable(StatusCode::NOT_FOUND, "隧道不存在或已删除")))?;
    let domains = sqlx::query_scalar::<_, String>(
        "SELECT domain FROM tunnel_domains WHERE tunnel_id = $1 AND status IN ('approved', 'pending_review') ORDER BY domain",
    )
    .bind(tunnel_id)
    .fetch_all(&pg)
    .await
    .map_err(database_error)?;
    Ok(Json(TunnelDetailResponse { tunnel, domains }))
}

async fn user_suspend_tunnel(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(tunnel_id): Path<Uuid>,
) -> Result<Json<TunnelDetailRow>, (StatusCode, Json<Value>)> {
    let (pg, user) = state
        .database_for(&headers, UserRole::User)
        .await
        .map_err(response_error)?;
    transition_tunnel_status(
        &pg,
        user.user_id,
        tunnel_id,
        "suspended",
        false,
        &["active", "provisioning", "error"],
        TunnelRuntimeSync::Suspend,
    )
    .await
}

async fn user_resume_tunnel(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(tunnel_id): Path<Uuid>,
) -> Result<Json<TunnelDetailRow>, (StatusCode, Json<Value>)> {
    let (pg, user) = state
        .database_for(&headers, UserRole::User)
        .await
        .map_err(response_error)?;
    transition_tunnel_status(
        &pg,
        user.user_id,
        tunnel_id,
        "active",
        true,
        &["suspended"],
        TunnelRuntimeSync::Online,
    )
    .await
}

async fn user_close_tunnel(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(tunnel_id): Path<Uuid>,
) -> Result<Json<TunnelDetailRow>, (StatusCode, Json<Value>)> {
    let (pg, user) = state
        .database_for(&headers, UserRole::User)
        .await
        .map_err(response_error)?;
    let row = sqlx::query_as::<_, (String, bool)>(
        "SELECT status, enabled FROM tunnels WHERE id = $1 AND user_id = $2 AND status <> 'deleted'",
    )
    .bind(tunnel_id)
    .bind(user.user_id)
    .fetch_optional(&pg)
    .await
    .map_err(database_error)?
    .ok_or_else(|| response_error(unavailable(StatusCode::NOT_FOUND, "隧道不存在或已删除")))?;
    if !row.1 {
        return Err(response_error(unavailable(
            StatusCode::CONFLICT,
            "隧道已处于关闭状态",
        )));
    }
    if row.0 == "pending_review" {
        return Err(response_error(unavailable(
            StatusCode::CONFLICT,
            "审核中的隧道不能关闭",
        )));
    }
    transition_tunnel_status(
        &pg,
        user.user_id,
        tunnel_id,
        "active",
        false,
        &["active", "provisioning", "error"],
        TunnelRuntimeSync::Offline,
    )
    .await
}

async fn user_open_tunnel(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(tunnel_id): Path<Uuid>,
) -> Result<Json<TunnelDetailRow>, (StatusCode, Json<Value>)> {
    let (pg, user) = state
        .database_for(&headers, UserRole::User)
        .await
        .map_err(response_error)?;
    let row = sqlx::query_as::<_, (String, bool)>(
        "SELECT status, enabled FROM tunnels WHERE id = $1 AND user_id = $2 AND status <> 'deleted'",
    )
    .bind(tunnel_id)
    .bind(user.user_id)
    .fetch_optional(&pg)
    .await
    .map_err(database_error)?
    .ok_or_else(|| response_error(unavailable(StatusCode::NOT_FOUND, "隧道不存在或已删除")))?;
    if row.1 {
        return Err(response_error(unavailable(
            StatusCode::CONFLICT,
            "隧道已在运行中",
        )));
    }
    if row.0 == "suspended" {
        return Err(response_error(unavailable(
            StatusCode::CONFLICT,
            "订阅暂停的隧道请使用恢复，而非开启",
        )));
    }
    if row.0 == "pending_review" {
        return Err(response_error(unavailable(
            StatusCode::CONFLICT,
            "审核中的隧道暂不能开启",
        )));
    }
    transition_tunnel_status(
        &pg,
        user.user_id,
        tunnel_id,
        "active",
        true,
        &["active", "error", "provisioning"],
        TunnelRuntimeSync::Online,
    )
    .await
}

#[derive(Debug, Clone, Copy)]
enum TunnelRuntimeSync {
    Offline,
    Online,
    Suspend,
}

async fn transition_tunnel_status(
    pg: &PgPool,
    user_id: Uuid,
    tunnel_id: Uuid,
    next_status: &str,
    enabled: bool,
    allowed_from: &[&str],
    runtime: TunnelRuntimeSync,
) -> Result<Json<TunnelDetailRow>, (StatusCode, Json<Value>)> {
    let allowed: HashSet<&str> = allowed_from.iter().copied().collect();
    let current = sqlx::query_as::<_, (String, Uuid, Uuid)>(
        "SELECT status, node_id, client_id FROM tunnels WHERE id = $1 AND user_id = $2 AND status <> 'deleted'",
    )
    .bind(tunnel_id)
    .bind(user_id)
    .fetch_optional(pg)
    .await
    .map_err(database_error)?
    .ok_or_else(|| response_error(unavailable(StatusCode::NOT_FOUND, "隧道不存在或已删除")))?;
    if !allowed.contains(current.0.as_str()) {
        return Err(response_error(unavailable(
            StatusCode::CONFLICT,
            "当前状态不允许此操作",
        )));
    }
    if enabled {
        ensure_quota_available(pg, user_id).await?;
    }
    let row = sqlx::query_as::<_, TunnelDetailRow>(
        "UPDATE tunnels SET status = $3, enabled = $4, last_error = CASE WHEN $3 = 'active' THEN NULL ELSE last_error END, guard_pause_until = CASE WHEN $4 THEN NULL ELSE guard_pause_until END, revision = revision + 1, updated_at = now() WHERE id = $1 AND user_id = $2 RETURNING id, user_id, client_id, NULL::text AS client_name, node_id, NULL::text AS node_name, name, carrier, protocol, remote_port, target_host, target_port, target_url, http_access, https_enabled, status, enabled, speed_limit_mbps, max_conns, max_new_conns_per_sec, last_error",
    )
    .bind(tunnel_id)
    .bind(user_id)
    .bind(next_status)
    .bind(enabled)
    .fetch_one(pg)
    .await
    .map_err(database_error)?;
    match runtime {
        TunnelRuntimeSync::Suspend => {
            crate::ws::push_tunnel_suspend(pg, current.1, current.2, tunnel_id).await;
        }
        TunnelRuntimeSync::Offline => {
            crate::ws::sync_tunnel_offline(pg, current.1, current.2, tunnel_id).await;
        }
        TunnelRuntimeSync::Online => {
            crate::ws::sync_tunnel_online(pg, current.1, current.2, tunnel_id).await;
        }
    }
    Ok(Json(row))
}

async fn ensure_quota_available(pg: &PgPool, user_id: Uuid) -> Result<(), (StatusCode, Json<Value>)> {
    if crate::quota::user_quota_exhausted(pg, user_id)
        .await
        .map_err(database_error)?
    {
        return Err(response_error(unavailable(
            StatusCode::FORBIDDEN,
            "当前周期流量已用尽，额度恢复前无法启动隧道",
        )));
    }
    Ok(())
}

async fn admin_set_tunnel_enabled(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(tunnel_id): Path<Uuid>,
    Json(body): Json<SetTunnelEnabled>,
) -> Result<StatusCode, (StatusCode, Json<Value>)> {
    let (pg, actor) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    if body.enabled {
        admin_open_tunnel(&pg, tunnel_id).await?;
    } else {
        admin_close_tunnel(&pg, tunnel_id).await?;
    }
    let mut transaction = pg.begin().await.map_err(database_error)?;
    write_audit(
        &mut transaction,
        actor.user_id,
        if body.enabled {
            "tunnel.enable"
        } else {
            "tunnel.disable"
        },
        "tunnel",
        tunnel_id,
    )
    .await?;
    transaction.commit().await.map_err(database_error)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn admin_open_tunnel(
    pg: &PgPool,
    tunnel_id: Uuid,
) -> Result<(), (StatusCode, Json<Value>)> {
    let row = sqlx::query_as::<_, (String, bool)>(
        "SELECT status, enabled FROM tunnels WHERE id = $1 AND status <> 'deleted'",
    )
    .bind(tunnel_id)
    .fetch_optional(pg)
    .await
    .map_err(database_error)?
    .ok_or_else(|| response_error(unavailable(StatusCode::NOT_FOUND, "隧道不存在或已删除")))?;
    if row.1 {
        return Ok(());
    }
    if row.0 == "suspended" {
        transition_tunnel_status_admin(
            pg,
            tunnel_id,
            "active",
            true,
            &["suspended"],
            TunnelRuntimeSync::Online,
        )
        .await?;
        return Ok(());
    }
    if row.0 == "pending_review" {
        return Err(response_error(unavailable(
            StatusCode::CONFLICT,
            "审核中的隧道暂不能开启",
        )));
    }
    transition_tunnel_status_admin(
        pg,
        tunnel_id,
        "active",
        true,
        &["active", "error", "provisioning"],
        TunnelRuntimeSync::Online,
    )
    .await?;
    Ok(())
}

async fn admin_close_tunnel(
    pg: &PgPool,
    tunnel_id: Uuid,
) -> Result<(), (StatusCode, Json<Value>)> {
    let row = sqlx::query_as::<_, (String, bool)>(
        "SELECT status, enabled FROM tunnels WHERE id = $1 AND status <> 'deleted'",
    )
    .bind(tunnel_id)
    .fetch_optional(pg)
    .await
    .map_err(database_error)?
    .ok_or_else(|| response_error(unavailable(StatusCode::NOT_FOUND, "隧道不存在或已删除")))?;
    if !row.1 {
        return Ok(());
    }
    if row.0 == "pending_review" {
        return Err(response_error(unavailable(
            StatusCode::CONFLICT,
            "审核中的隧道不能关闭",
        )));
    }
    transition_tunnel_status_admin(
        pg,
        tunnel_id,
        "active",
        false,
        &["active", "provisioning", "error"],
        TunnelRuntimeSync::Offline,
    )
    .await?;
    Ok(())
}

async fn transition_tunnel_status_admin(
    pg: &PgPool,
    tunnel_id: Uuid,
    next_status: &str,
    enabled: bool,
    allowed_from: &[&str],
    runtime: TunnelRuntimeSync,
) -> Result<(), (StatusCode, Json<Value>)> {
    let allowed: HashSet<&str> = allowed_from.iter().copied().collect();
    let current = sqlx::query_as::<_, (String, Uuid, Uuid, Uuid)>(
        "SELECT status, node_id, client_id, user_id FROM tunnels WHERE id = $1 AND status <> 'deleted'",
    )
    .bind(tunnel_id)
    .fetch_optional(pg)
    .await
    .map_err(database_error)?
    .ok_or_else(|| response_error(unavailable(StatusCode::NOT_FOUND, "隧道不存在或已删除")))?;
    if !allowed.contains(current.0.as_str()) {
        return Err(response_error(unavailable(
            StatusCode::CONFLICT,
            "当前状态不允许此操作",
        )));
    }
    if enabled {
        ensure_quota_available(pg, current.3).await?;
    }
    sqlx::query(
        "UPDATE tunnels SET status = $2, enabled = $3, last_error = CASE WHEN $2 = 'active' THEN NULL ELSE last_error END, guard_pause_until = CASE WHEN $3 THEN NULL ELSE guard_pause_until END, revision = revision + 1, updated_at = now() WHERE id = $1",
    )
    .bind(tunnel_id)
    .bind(next_status)
    .bind(enabled)
    .execute(pg)
    .await
    .map_err(database_error)?;
    match runtime {
        TunnelRuntimeSync::Suspend => {
            crate::ws::push_tunnel_suspend(pg, current.1, current.2, tunnel_id).await;
        }
        TunnelRuntimeSync::Offline => {
            crate::ws::sync_tunnel_offline(pg, current.1, current.2, tunnel_id).await;
        }
        TunnelRuntimeSync::Online => {
            crate::ws::sync_tunnel_online(pg, current.1, current.2, tunnel_id).await;
        }
    }
    Ok(())
}

#[derive(Debug, FromRow)]
struct EditableTunnel {
    protocol: String,
    carrier: String,
    user_id: Uuid,
    client_id: Uuid,
    node_id: Uuid,
    name: String,
    remote_port: Option<i32>,
    port_custom: bool,
    target_host: Option<String>,
    target_port: Option<i32>,
    target_url: Option<String>,
    http_access: Option<String>,
    host_rewrite: Option<String>,
    backend_tls_insecure: bool,
    https_enabled: bool,
    cert_id: Option<Uuid>,
    speed_limit_mbps: i64,
}

async fn admin_update_tunnel(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(tunnel_id): Path<Uuid>,
    Json(body): Json<AdminUpdateTunnel>,
) -> Result<StatusCode, (StatusCode, Json<Value>)> {
    let (pg, actor) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    let current = sqlx::query_as::<_, EditableTunnel>(
        "SELECT protocol, carrier, user_id, client_id, node_id, name, remote_port, port_custom, target_host, target_port, target_url, http_access, host_rewrite, backend_tls_insecure, https_enabled, cert_id, speed_limit_mbps FROM tunnels WHERE id = $1 AND status <> 'deleted'",
    )
    .bind(tunnel_id)
    .fetch_optional(&pg)
    .await
    .map_err(database_error)?
    .ok_or_else(|| response_error(unavailable(StatusCode::NOT_FOUND, "隧道不存在或已删除")))?;

    let name = match body.name.as_deref() {
        Some(value) => {
            let name = value.trim();
            if name.is_empty() || name.len() > 100 {
                return Err(response_error(unavailable(
                    StatusCode::BAD_REQUEST,
                    "隧道名称不能为空",
                )));
            }
            name.to_owned()
        }
        None => current.name.clone(),
    };
    let client_id = body.client_id.unwrap_or(current.client_id);
    let node_id = body.node_id.unwrap_or(current.node_id);
    let user_id: Uuid = sqlx::query_scalar(
        "SELECT user_id FROM clients WHERE id = $1 AND enabled = TRUE",
    )
    .bind(client_id)
    .fetch_optional(&pg)
    .await
    .map_err(database_error)?
    .ok_or_else(|| response_error(unavailable(StatusCode::NOT_FOUND, "Client 不存在或已停用")))?;
    let client_capabilities: Value = sqlx::query_scalar("SELECT capabilities FROM clients WHERE id = $1")
        .bind(client_id)
        .fetch_one(&pg)
        .await
        .map_err(database_error)?;
    if !supports_carrier(&client_capabilities, &current.carrier) {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "所选 Client 尚未上报该 Carrier 能力",
        )));
    }
    let node = sqlx::query_as::<_, TunnelNode>(
        "SELECT region, capabilities, protocols, carrier_ports, tcp_port_ranges, udp_port_ranges, port_exclude, http_shared_port, https_shared_port, guard_policy FROM nodes WHERE id = $1 AND enabled = TRUE",
    )
    .bind(node_id)
    .fetch_optional(&pg)
    .await
    .map_err(database_error)?
    .ok_or_else(|| response_error(unavailable(StatusCode::NOT_FOUND, "节点不存在或已停用")))?;
    if !node.protocols.iter().any(|protocol| protocol == &current.protocol)
        || !tz_ingress::registered_kinds().contains(&current.protocol.as_str())
    {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "所选节点不支持该协议",
        )));
    }
    if !carrier_enabled(&node.carrier_ports, &current.carrier)
        || !supports_carrier(&node.capabilities, &current.carrier)
    {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "所选节点不支持该 Carrier",
        )));
    }

    let is_http = current.protocol == "http";
    let (target_host, target_port, target_url) = if is_http {
        let raw = body
            .target_url
            .as_deref()
            .or(current.target_url.as_deref())
            .unwrap_or_default()
            .trim();
        validate_target_url(raw)?;
        (None, None, Some(raw.to_owned()))
    } else {
        let host = body
            .target_host
            .as_deref()
            .or(current.target_host.as_deref())
            .unwrap_or_default()
            .trim();
        let port = body
            .target_port
            .or_else(|| current.target_port.and_then(|port| u16::try_from(port).ok()))
            .filter(|port| *port > 0);
        if host.is_empty() || host.len() > 253 || port.is_none() {
            return Err(response_error(unavailable(
                StatusCode::BAD_REQUEST,
                "转发目标地址或端口无效",
            )));
        }
        (Some(host.to_owned()), port.map(i32::from), None)
    };
    let http_access = if is_http {
        let access = body
            .http_access
            .as_deref()
            .or(current.http_access.as_deref())
            .unwrap_or("shared");
        if access != "shared" && access != "dedicated" {
            return Err(response_error(unavailable(
                StatusCode::BAD_REQUEST,
                "HTTP 访问方式必须选择共享或独立端口",
            )));
        }
        Some(access.to_owned())
    } else {
        None
    };
    let shared_http = http_access.as_deref() == Some("shared");
    let existing_domains: Vec<String> = sqlx::query_scalar(
        "SELECT domain FROM tunnel_domains WHERE tunnel_id = $1 AND status IN ('approved', 'pending_review') ORDER BY domain",
    )
    .bind(tunnel_id)
    .fetch_all(&pg)
    .await
    .map_err(database_error)?;
    let domains = if is_http {
        if let Some(domains) = &body.domains {
            let mut prepared = Vec::new();
            let mut unique = HashSet::new();
            for domain in domains {
                let normalized = normalize_domain(domain, !shared_http)?;
                if !unique.insert(normalized.clone()) {
                    return Err(response_error(unavailable(
                        StatusCode::BAD_REQUEST,
                        "同一隧道不能重复绑定域名",
                    )));
                }
                prepared.push(normalized);
            }
            prepared
        } else {
            existing_domains.clone()
        }
    } else if body.domains.as_ref().is_some_and(|domains| !domains.is_empty()) {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "只有 HTTP 隧道可以绑定域名",
        )));
    } else {
        Vec::new()
    };
    if shared_http && domains.is_empty() {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "共享 HTTP(S) 入口至少需要绑定一个域名",
        )));
    }
    if shared_http && domains.iter().any(|domain| IpAddr::from_str(domain).is_ok()) {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "共享入口不能填写 IP，请使用域名",
        )));
    }
    let requested_port = match body.remote_port {
        Some(Some(port)) if shared_http => {
            let _ = port;
            return Err(response_error(unavailable(
                StatusCode::BAD_REQUEST,
                "共享 HTTP(S) 入口不能指定独立公网端口",
            )));
        }
        Some(value) => value,
        None => current.remote_port.and_then(|port| u16::try_from(port).ok()),
    };
    let keep_port = !shared_http
        && node_id == current.node_id
        && requested_port.is_some_and(|port| Some(i32::from(port)) == current.remote_port);
    let (remote_port, mut reservation) = if shared_http || keep_port {
        (
            if shared_http {
                None
            } else {
                requested_port.or_else(|| current.remote_port.and_then(|port| u16::try_from(port).ok()))
            },
            None,
        )
    } else {
        allocate_port(
            &state,
            &pg,
            &node,
            node_id,
            &current.protocol,
            false,
            requested_port,
            true,
        )
        .await?
    };
    let port_custom = if shared_http {
        false
    } else {
        match body.remote_port {
            Some(Some(port))
                if node_id == current.node_id && Some(i32::from(port)) == current.remote_port =>
            {
                current.port_custom
            }
            Some(Some(_)) => true,
            Some(None) => false,
            None => current.port_custom,
        }
    };
    let backend_tls_insecure = if is_http {
        body.backend_tls_insecure.unwrap_or(current.backend_tls_insecure)
    } else {
        false
    };
    let target_is_https = target_url
        .as_deref()
        .is_some_and(|target| Url::parse(target).is_ok_and(|url| url.scheme() == "https"));
    if backend_tls_insecure && !target_is_https {
        rollback_port_reservation(reservation.take(), node_id, &state, &pg).await;
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "只有 HTTPS 后端可以关闭证书校验",
        )));
    }
    let https_enabled = is_http && body.https_enabled.unwrap_or(current.https_enabled);
    if https_enabled && domains.is_empty() {
        rollback_port_reservation(reservation.take(), node_id, &state, &pg).await;
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "启用 HTTPS 的 HTTP 隧道必须绑定域名",
        )));
    }
    let cert_id = if https_enabled {
        let cert_id = body.cert_id.or(current.cert_id).ok_or_else(|| {
            response_error(unavailable(
                StatusCode::BAD_REQUEST,
                "启用 HTTPS 必须选择证书",
            ))
        })?;
        let usable: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM certificates WHERE id = $1 AND (owner_user_id = $2 OR owner_user_id IS NULL) AND not_after > now())",
        )
        .bind(cert_id)
        .bind(user_id)
        .fetch_one(&pg)
        .await
        .map_err(database_error)?;
        if !usable {
            rollback_port_reservation(reservation.take(), node_id, &state, &pg).await;
            return Err(response_error(unavailable(
                StatusCode::BAD_REQUEST,
                "证书不存在、已过期或无权使用",
            )));
        }
        Some(cert_id)
    } else {
        None
    };
    let host_rewrite = if is_http {
        body.host_rewrite
            .clone()
            .or(current.host_rewrite.clone())
            .unwrap_or_else(|| "$http_host".into())
    } else {
        "$http_host".into()
    };
    if host_rewrite.trim().is_empty()
        || host_rewrite.len() > 253
        || host_rewrite.chars().any(char::is_control)
    {
        rollback_port_reservation(reservation.take(), node_id, &state, &pg).await;
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "Host 改写值无效",
        )));
    }
    let speed_limit_mbps = match body.speed_limit_mbps {
        Some(limit) if limit <= 0 => {
            rollback_port_reservation(reservation.take(), node_id, &state, &pg).await;
            return Err(response_error(unavailable(
                StatusCode::BAD_REQUEST,
                "限速必须大于 0",
            )));
        }
        Some(limit) => limit,
        None => current.speed_limit_mbps,
    };

    let enforce_cn_filing = is_http
        && node.region.eq_ignore_ascii_case("CN")
        && crate::guard_policy::cn_http_filing_enabled(
            &crate::guard_policy::effective_node_guard_policy(&pg, &node.guard_policy).await,
        );
    let whitelist: Vec<String> = if enforce_cn_filing {
        sqlx::query_scalar("SELECT domain FROM domain_whitelist")
            .fetch_all(&pg)
            .await
            .map_err(database_error)?
    } else {
        Vec::new()
    };

    let mut transaction = match pg.begin().await {
        Ok(transaction) => transaction,
        Err(error) => {
            rollback_port_reservation(reservation.take(), node_id, &state, &pg).await;
            return Err(database_error(error));
        }
    };
    let node_lock = format!("tunnel-ports:{node_id}");
    if sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 42))")
        .bind(&node_lock)
        .execute(&mut *transaction)
        .await
        .is_err()
    {
        let _ = transaction.rollback().await;
        rollback_port_reservation(reservation.take(), node_id, &state, &pg).await;
        return Err(response_error(unavailable(StatusCode::SERVICE_UNAVAILABLE, "数据库暂不可用")));
    }
    let updated = sqlx::query(
        "UPDATE tunnels SET name = $2, user_id = $3, client_id = $4, node_id = $5, remote_port = $6, port_custom = $7, target_host = $8, target_port = $9, target_url = $10, http_access = $11, https_enabled = $12, cert_id = $13, host_rewrite = $14, backend_tls_insecure = $15, speed_limit_mbps = $16, revision = revision + 1, updated_at = now() WHERE id = $1 AND status <> 'deleted'",
    )
    .bind(tunnel_id)
    .bind(&name)
    .bind(user_id)
    .bind(client_id)
    .bind(node_id)
    .bind(remote_port.map(i32::from))
    .bind(port_custom)
    .bind(&target_host)
    .bind(target_port)
    .bind(&target_url)
    .bind(&http_access)
    .bind(https_enabled)
    .bind(cert_id)
    .bind(&host_rewrite)
    .bind(backend_tls_insecure)
    .bind(speed_limit_mbps)
    .execute(&mut *transaction)
    .await;
    if updated.map(|result| result.rows_affected()).unwrap_or(0) == 0 {
        let _ = transaction.rollback().await;
        rollback_port_reservation(reservation.take(), node_id, &state, &pg).await;
        return Err(response_error(unavailable(StatusCode::CONFLICT, "隧道更新失败")));
    }
    if is_http && body.domains.is_some() {
        if let Err(error) = replace_tunnel_domains(
            &mut transaction,
            tunnel_id,
            user_id,
            &domains,
            &existing_domains,
            enforce_cn_filing,
            &whitelist,
        )
        .await
        {
            let _ = transaction.rollback().await;
            rollback_port_reservation(reservation.take(), node_id, &state, &pg).await;
            return Err(error);
        }
    } else if user_id != current.user_id {
        if sqlx::query(
            "UPDATE tunnel_domains SET user_id = $2 WHERE tunnel_id = $1 AND status IN ('approved', 'pending_review')",
        )
        .bind(tunnel_id)
        .bind(user_id)
        .execute(&mut *transaction)
        .await
        .is_err()
        {
            let _ = transaction.rollback().await;
            rollback_port_reservation(reservation.take(), node_id, &state, &pg).await;
            return Err(response_error(unavailable(StatusCode::SERVICE_UNAVAILABLE, "数据库暂不可用")));
        }
    }
    if let Err(error) = write_audit(
        &mut transaction,
        actor.user_id,
        "tunnel.update",
        "tunnel",
        tunnel_id,
    )
    .await
    {
        let _ = transaction.rollback().await;
        rollback_port_reservation(reservation.take(), node_id, &state, &pg).await;
        return Err(error);
    }
    if let Err(error) = transaction.commit().await {
        rollback_port_reservation(reservation.take(), node_id, &state, &pg).await;
        return Err(database_error(error));
    }
    if let Some((cache, protocol, port, token)) = reservation.take() {
        if !matches!(
            cache.commit_reservation(node_id, protocol, port, token).await,
            Ok(true)
        ) {
            invalidate_port_cache(&cache, node_id, protocol, &state, &pg).await;
        }
    }
    let old_port = current.remote_port.and_then(|port| u16::try_from(port).ok());
    let port_changed = shared_http || node_id != current.node_id || remote_port != old_port;
    if port_changed {
        if let Some(port) = old_port {
            if let Some(redis) = state.redis_connection() {
                let cache = RedisPortCache::new(redis);
                let l4 = if current.protocol == "udp" {
                    PortProtocol::Udp
                } else {
                    PortProtocol::Tcp
                };
                if cache.release(current.node_id, l4, port).await.is_err() {
                    if cache.invalidate(current.node_id, l4).await.is_err() {
                        state.disable_redis();
                    }
                    queue_node_rebuild(&pg, current.node_id).await;
                }
            }
        }
    }
    let ctl_pg = state.pg_control().unwrap_or_else(|| pg.clone());
    crate::ws::retarget_tunnel(
        &ctl_pg,
        current.node_id,
        current.client_id,
        node_id,
        client_id,
        tunnel_id,
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

async fn replace_tunnel_domains(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tunnel_id: Uuid,
    user_id: Uuid,
    domains: &[String],
    existing: &[String],
    enforce_cn_filing: bool,
    whitelist: &[String],
) -> Result<(), (StatusCode, Json<Value>)> {
    let removed: Vec<&String> = existing
        .iter()
        .filter(|domain| !domains.contains(domain))
        .collect();
    if !removed.is_empty() {
        let removed: Vec<String> = removed.into_iter().cloned().collect();
        sqlx::query(
            "UPDATE tunnel_domains SET status = 'cooldown', cooldown_until = now() + interval '24 hours' WHERE tunnel_id = $1 AND domain = ANY($2) AND status IN ('approved', 'pending_review')",
        )
        .bind(tunnel_id)
        .bind(&removed)
        .execute(&mut **transaction)
        .await
        .map_err(database_error)?;
    }
    if user_id_changed_domains(transaction, tunnel_id, user_id).await.is_err() {
        return Err(response_error(unavailable(
            StatusCode::SERVICE_UNAVAILABLE,
            "数据库暂不可用",
        )));
    }
    for domain in domains {
        if existing.iter().any(|current| current == domain) {
            continue;
        }
        if enforce_cn_filing
            && IpAddr::from_str(domain).is_err()
            && !super::domains::domain_covers(whitelist, domain)
        {
            return Err(response_error(unavailable(
                StatusCode::FORBIDDEN,
                "该域名未过白，不能在中国大陆节点创建 HTTP 隧道",
            )));
        }
        let cooldown_owner = sqlx::query_scalar::<_, Uuid>(
            "SELECT user_id FROM tunnel_domains WHERE domain = $1 AND status IN ('rejected', 'cooldown') AND cooldown_until > now() FOR UPDATE",
        )
        .bind(domain)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(database_error)?;
        if cooldown_owner.is_some_and(|owner| owner != user_id) {
            return Err(response_error(unavailable(
                StatusCode::CONFLICT,
                "该域名仍在冷却期内，只有原用户可以重新绑定",
            )));
        }
        let wrote = if cooldown_owner.is_some() {
            sqlx::query("UPDATE tunnel_domains SET tunnel_id = $1, user_id = $2, status = 'approved', reviewed_by = NULL, reviewed_at = NULL, cooldown_until = NULL, created_at = now() WHERE domain = $3")
                .bind(tunnel_id)
                .bind(user_id)
                .bind(domain)
                .execute(&mut **transaction)
                .await
                .map(|_| ())
        } else {
            match sqlx::query("DELETE FROM tunnel_domains WHERE domain = $1 AND status IN ('rejected', 'cooldown') AND cooldown_until <= now()")
                .bind(domain)
                .execute(&mut **transaction)
                .await
            {
                Ok(_) => sqlx::query("INSERT INTO tunnel_domains (id, tunnel_id, user_id, domain, status) VALUES ($1, $2, $3, $4, 'approved')")
                    .bind(Uuid::new_v4())
                    .bind(tunnel_id)
                    .bind(user_id)
                    .bind(domain)
                    .execute(&mut **transaction)
                    .await
                    .map(|_| ()),
                Err(error) => Err(error),
            }
        };
        if wrote.is_err() {
            return Err(response_error(unavailable(
                StatusCode::CONFLICT,
                "域名已被占用或审核记录创建失败",
            )));
        }
    }
    Ok(())
}

async fn user_id_changed_domains(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tunnel_id: Uuid,
    user_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE tunnel_domains SET user_id = $2 WHERE tunnel_id = $1 AND status IN ('approved', 'pending_review')",
    )
    .bind(tunnel_id)
    .bind(user_id)
    .execute(&mut **transaction)
    .await
    .map(|_| ())
}

async fn admin_delete_tunnel(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(tunnel_id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, Json<Value>)> {
    let (pg, actor) = state.database_for(&headers, UserRole::Admin).await.map_err(response_error)?;
    delete_tunnel_record(&state, &pg, actor.user_id, tunnel_id, None).await
}

async fn user_delete_tunnel(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(tunnel_id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, Json<Value>)> {
    let (pg, user) = state.database_for(&headers, UserRole::User).await.map_err(response_error)?;
    delete_tunnel_record(&state, &pg, user.user_id, tunnel_id, Some(user.user_id)).await
}

async fn delete_tunnel_record(
    state: &AppState,
    pg: &PgPool,
    actor_id: Uuid,
    tunnel_id: Uuid,
    owner_id: Option<Uuid>,
) -> Result<StatusCode, (StatusCode, Json<Value>)> {
    let mut transaction = pg.begin().await.map_err(database_error)?;
    let query = if owner_id.is_some() {
        "SELECT node_id, client_id, protocol, remote_port FROM tunnels WHERE id = $1 AND user_id = $2 AND status <> 'deleted'"
    } else {
        "SELECT node_id, client_id, protocol, remote_port FROM tunnels WHERE id = $1 AND status <> 'deleted'"
    };
    let mut lookup = sqlx::query_as::<_, (Uuid, Uuid, String, Option<i32>)>(query).bind(tunnel_id);
    if let Some(owner_id) = owner_id {
        lookup = lookup.bind(owner_id);
    }
    let deleted = lookup
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| response_error(unavailable(StatusCode::NOT_FOUND, "隧道不存在或已删除")))?;
    let node_lock = format!("tunnel-ports:{}", deleted.0);
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 42))")
        .bind(node_lock)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
    let update = if let Some(owner_id) = owner_id {
        sqlx::query("UPDATE tunnels SET status = 'deleted', enabled = FALSE, revision = revision + 1, updated_at = now() WHERE id = $1 AND user_id = $2 AND status <> 'deleted'")
            .bind(tunnel_id)
            .bind(owner_id)
            .execute(&mut *transaction)
            .await
    } else {
        sqlx::query("UPDATE tunnels SET status = 'deleted', enabled = FALSE, revision = revision + 1, updated_at = now() WHERE id = $1 AND status <> 'deleted'")
            .bind(tunnel_id)
            .execute(&mut *transaction)
            .await
    }
    .map_err(database_error)?;
    if update.rows_affected() == 0 {
        return Err(response_error(unavailable(StatusCode::NOT_FOUND, "隧道不存在或已删除")));
    }
    sqlx::query(
        "UPDATE tunnel_domains SET status = 'cooldown', cooldown_until = now() + interval '24 hours' WHERE tunnel_id = $1 AND status IN ('approved', 'pending_review')",
    )
    .bind(tunnel_id)
    .execute(&mut *transaction)
    .await
    .map_err(database_error)?;
    write_audit(&mut transaction, actor_id, "tunnel.delete", "tunnel", tunnel_id).await?;
    transaction.commit().await.map_err(database_error)?;

    crate::ws::on_tunnel_removed(pg, deleted.0, deleted.1, tunnel_id).await;

    if let Some(port) = deleted.3.and_then(|port| u16::try_from(port).ok()) {
        if let Some(redis) = state.redis_connection() {
            let cache = RedisPortCache::new(redis);
            let l4 = if deleted.2 == "udp" { PortProtocol::Udp } else { PortProtocol::Tcp };
            if cache.release(deleted.0, l4, port).await.is_err() {
                if cache.invalidate(deleted.0, l4).await.is_err() {
                    state.disable_redis();
                }
                queue_node_rebuild(pg, deleted.0).await;
            }
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn queue_node_rebuild(pg: &PgPool, node_id: Uuid) {
    let _ = sqlx::query("INSERT INTO redis_outbox (operation, payload) VALUES ('rebuild_node_ports', $1)")
        .bind(serde_json::json!({ "node_id": node_id }))
        .execute(pg)
        .await;
}

async fn create_tunnel(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<CreateTunnel>,
) -> Result<(StatusCode, Json<TunnelRow>), (StatusCode, Json<Value>)> {
    let (pg, user) = state
        .database_for(&headers, UserRole::User)
        .await
        .map_err(response_error)?;
    create_tunnel_inner(&state, &pg, user.user_id, user.user_id, request).await
}

async fn admin_create_tunnel(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<AdminCreateTunnel>,
) -> Result<(StatusCode, Json<TunnelRow>), (StatusCode, Json<Value>)> {
    let (pg, actor) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    let user_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT user_id FROM clients WHERE id = $1 AND enabled = TRUE",
    )
    .bind(body.client_id)
    .fetch_optional(&pg)
    .await
    .map_err(database_error)?;
    let owner_user_id = user_id.ok_or_else(|| {
        response_error(unavailable(StatusCode::NOT_FOUND, "Client 不存在或已停用"))
    })?;
    let request = CreateTunnel {
        name: body.name,
        node_id: body.node_id,
        client_id: body.client_id,
        carrier: body.carrier,
        protocol: body.protocol,
        remote_port: body.remote_port,
        target_host: body.target_host,
        target_port: body.target_port,
        target_url: body.target_url,
        http_access: body.http_access,
        domains: body.domains,
        https_enabled: body.https_enabled,
        cert_id: body.cert_id,
        host_rewrite: body.host_rewrite,
        backend_tls_insecure: body.backend_tls_insecure,
    };
    create_tunnel_inner(&state, &pg, actor.user_id, owner_user_id, request).await
}

async fn create_tunnel_inner(
    state: &Arc<AppState>,
    pg: &PgPool,
    actor_id: Uuid,
    owner_user_id: Uuid,
    request: CreateTunnel,
) -> Result<(StatusCode, Json<TunnelRow>), (StatusCode, Json<Value>)> {
    let name = request.name.trim();
    if name.is_empty() || name.len() > 100 {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "隧道名称长度必须为 1 到 100 个字符",
        )));
    }
    const ACTIVE_SUBSCRIPTION_SQL: &str = "SELECT id, speed_limit_mbps, max_conns_per_tunnel, max_new_conns_per_sec, max_tunnels, allow_custom_port, allowed_protocols FROM user_subscriptions WHERE user_id = $1 AND status = 'active' AND starts_at <= now() AND (expires_at IS NULL OR expires_at > now()) AND exhausted_period_start IS NULL ORDER BY starts_at DESC LIMIT 1";
    let subscription = sqlx::query_as::<_, SubscriptionLimits>(ACTIVE_SUBSCRIPTION_SQL)
        .bind(owner_user_id)
        .fetch_optional(pg)
        .await
        .map_err(database_error)?
        .ok_or_else(|| {
            response_error(unavailable(
                StatusCode::FORBIDDEN,
                "用户没有有效订阅或当前周期流量已用尽",
            ))
        })?;
    if !subscription
        .allowed_protocols
        .iter()
        .any(|protocol| protocol == &request.protocol)
    {
        return Err(response_error(unavailable(
            StatusCode::FORBIDDEN,
            "当前订阅不允许所选上层协议",
        )));
    }
    let tunnel_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM tunnels WHERE user_id = $1 AND status <> 'deleted'",
    )
    .bind(owner_user_id)
    .fetch_one(pg)
    .await
    .map_err(database_error)?;
    if tunnel_count >= i64::from(subscription.max_tunnels) {
        return Err(response_error(unavailable(
            StatusCode::FORBIDDEN,
            "已达到订阅允许的隧道数量上限",
        )));
    }
    let client_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM clients WHERE id = $1 AND user_id = $2 AND enabled = TRUE)",
    )
    .bind(request.client_id)
    .bind(owner_user_id)
    .fetch_one(pg)
    .await
    .map_err(database_error)?;
    if !client_exists {
        return Err(response_error(unavailable(
            StatusCode::NOT_FOUND,
            "Client 不存在或已停用",
        )));
    }
    let node = sqlx::query_as::<_, TunnelNode>(
        "SELECT region, capabilities, protocols, carrier_ports, tcp_port_ranges, udp_port_ranges, port_exclude, http_shared_port, https_shared_port, guard_policy FROM nodes n WHERE n.id = $1 AND n.enabled = TRUE AND n.online = TRUE AND EXISTS (SELECT 1 FROM node_group_members ngm JOIN subscription_node_groups sng ON sng.node_group_id = ngm.node_group_id WHERE ngm.node_id = n.id AND sng.subscription_id = $2)",
    )
    .bind(request.node_id)
    .bind(subscription.id)
    .fetch_optional(pg)
    .await
    .map_err(database_error)?
    .ok_or_else(|| {
        response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "所选节点不在线或不在订阅允许的节点组中",
        ))
    })?;
    {
        let effective = crate::guard_policy::effective_node_guard_policy(pg, &node.guard_policy).await;
        if let Some(max_tunnels) = crate::guard_policy::max_tunnels_from_policy(&effective) {
            let node_tunnel_count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM tunnels WHERE node_id = $1 AND status <> 'deleted'",
            )
            .bind(request.node_id)
            .fetch_one(pg)
            .await
            .map_err(database_error)?;
            if node_tunnel_count >= i64::from(max_tunnels) {
                return Err(response_error(unavailable(
                    StatusCode::FORBIDDEN,
                    "已达到该节点允许的隧道数量上限",
                )));
            }
        }
        let client_region: String =
            sqlx::query_scalar("SELECT COALESCE(region, '') FROM clients WHERE id = $1")
                .bind(request.client_id)
                .fetch_one(pg)
                .await
                .map_err(database_error)?;
        if crate::guard_policy::cn_residency_blocks_client(
            &effective,
            &node.region,
            &client_region,
        ) {
            return Err(response_error(unavailable(
                StatusCode::FORBIDDEN,
                "中国大陆节点仅允许中国大陆地区的 Client 连接，以降低跨境数据传输风险",
            )));
        }
    }
    if !tz_ingress::registered_kinds().contains(&request.protocol.as_str())
        || !node.protocols.iter().any(|protocol| protocol == &request.protocol)
    {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "所选节点不支持该协议",
        )));
    }
    if !tz_carrier::registered_kinds().contains(&request.carrier.as_str())
        || !carrier_enabled(&node.carrier_ports, &request.carrier)
        || !supports_carrier(&node.capabilities, &request.carrier)
    {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "所选节点不支持该 Carrier",
        )));
    }
    let client_capabilities: Value =
        sqlx::query_scalar("SELECT capabilities FROM clients WHERE id = $1")
            .bind(request.client_id)
            .fetch_one(pg)
            .await
            .map_err(database_error)?;
    if !supports_carrier(&client_capabilities, &request.carrier) {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "所选 Client 尚未上报该 Carrier 能力",
        )));
    }

    let is_http = request.protocol == "http";
    let (target_host, target_port, target_url) = match request.protocol.as_str() {
        "http" => {
            if request.target_host.is_some() || request.target_port.is_some() {
                return Err(response_error(unavailable(
                    StatusCode::BAD_REQUEST,
                    "HTTP 转发目标必须填写 URL",
                )));
            }
            let raw = request.target_url.as_deref().unwrap_or_default().trim();
            validate_target_url(raw)?;
            (None, None, Some(raw.to_owned()))
        }
        _ => {
            if request.target_url.is_some() {
                return Err(response_error(unavailable(
                    StatusCode::BAD_REQUEST,
                    "TCP/UDP 转发目标不能填写 URL",
                )));
            }
            let host = request.target_host.as_deref().unwrap_or_default().trim();
            let port = request.target_port.filter(|port| *port > 0);
            if host.is_empty() || host.len() > 253 || port.is_none() {
                return Err(response_error(unavailable(
                    StatusCode::BAD_REQUEST,
                    "转发目标地址或端口无效",
                )));
            }
            (Some(host.to_owned()), port.map(i32::from), None)
        }
    };
    let http_access = if is_http {
        match request.http_access.as_deref() {
            Some("shared") | Some("dedicated") => request.http_access.clone(),
            _ => {
                return Err(response_error(unavailable(
                    StatusCode::BAD_REQUEST,
                    "HTTP 访问方式必须选择共享或独立端口",
                )));
            }
        }
    } else {
        if request.http_access.is_some() {
            return Err(response_error(unavailable(
                StatusCode::BAD_REQUEST,
                "非 HTTP 隧道不能设置 HTTP 访问方式",
            )));
        }
        None
    };
    let domains = request.domains.unwrap_or_default();
    let shared_http = is_http && http_access.as_deref() == Some("shared");
    if !is_http && !domains.is_empty() {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "只有 HTTP 隧道可以绑定域名",
        )));
    }
    if shared_http && domains.is_empty() {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "共享 HTTP(S) 入口至少需要绑定一个域名",
        )));
    }
    if shared_http && request.remote_port.is_some() {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "共享 HTTP(S) 入口不能指定独立公网端口",
        )));
    }
    let enforce_cn_filing = is_http
        && node.region.eq_ignore_ascii_case("CN")
        && crate::guard_policy::cn_http_filing_enabled(
            &crate::guard_policy::effective_node_guard_policy(pg, &node.guard_policy).await,
        );
    let whitelist: Vec<String> = if enforce_cn_filing {
        sqlx::query_scalar("SELECT domain FROM domain_whitelist")
            .fetch_all(pg)
            .await
            .map_err(database_error)?
    } else {
        Vec::new()
    };
    let mut prepared_domains = Vec::with_capacity(domains.len());
    let mut unique_domains = HashSet::with_capacity(domains.len());
    let final_status = "provisioning";
    for domain in &domains {
        let normalized = normalize_domain(domain, !shared_http)?;
        if !unique_domains.insert(normalized.clone()) {
            return Err(response_error(unavailable(
                StatusCode::BAD_REQUEST,
                "同一隧道不能重复绑定域名",
            )));
        }
        if enforce_cn_filing
            && IpAddr::from_str(&normalized).is_err()
            && !super::domains::domain_covers(&whitelist, &normalized)
        {
            return Err(response_error(unavailable(
                StatusCode::FORBIDDEN,
                "该域名未过白，不能在中国大陆节点创建 HTTP 隧道",
            )));
        }
        prepared_domains.push((normalized, true));
    }
    let backend_tls_insecure = request.backend_tls_insecure.unwrap_or(false);
    let target_is_https = target_url
        .as_deref()
        .is_some_and(|target| Url::parse(target).is_ok_and(|url| url.scheme() == "https"));
    if backend_tls_insecure && !target_is_https {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "只有 HTTPS 后端可以关闭证书校验",
        )));
    }
    let https_enabled = request.https_enabled.unwrap_or(false);
    if https_enabled && (!is_http || domains.is_empty()) {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "启用 HTTPS 的 HTTP 隧道必须绑定域名",
        )));
    }
    let cert_id = if https_enabled {
        let cert_id = request.cert_id.ok_or_else(|| {
            response_error(unavailable(
                StatusCode::BAD_REQUEST,
                "启用 HTTPS 必须选择证书",
            ))
        })?;
        let cert_domains: Option<Vec<String>> = sqlx::query_scalar(
            "SELECT domains FROM certificates WHERE id = $1 AND (owner_user_id = $2 OR owner_user_id IS NULL) AND not_after > now()",
        )
        .bind(cert_id)
        .bind(owner_user_id)
        .fetch_optional(pg)
        .await
        .map_err(database_error)?;
        if cert_domains.is_none() {
            return Err(response_error(unavailable(
                StatusCode::BAD_REQUEST,
                "证书不存在、已过期或无权使用",
            )));
        }
        Some(cert_id)
    } else {
        if request.cert_id.is_some() {
            return Err(response_error(unavailable(
                StatusCode::BAD_REQUEST,
                "未启用 HTTPS 时不能选择 HTTPS 证书",
            )));
        }
        None
    };
    let host_rewrite = request.host_rewrite.unwrap_or_else(|| "$http_host".into());
    if host_rewrite.trim().is_empty()
        || host_rewrite.len() > 253
        || host_rewrite.chars().any(char::is_control)
    {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "Host 改写值无效",
        )));
    }

    let id = Uuid::new_v4();
    let mut transaction = pg.begin().await.map_err(database_error)?;
    let user_lock = format!("tunnel-limit:{}", owner_user_id);
    let node_lock = format!("tunnel-ports:{}", request.node_id);
    for lock in [&user_lock, &node_lock] {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 42))")
            .bind(lock)
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
    }
    let current_tunnel_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM tunnels WHERE user_id = $1 AND status <> 'deleted'",
    )
    .bind(owner_user_id)
    .fetch_one(&mut *transaction)
    .await
    .map_err(database_error)?;
    if current_tunnel_count >= i64::from(subscription.max_tunnels) {
        return Err(response_error(unavailable(
            StatusCode::FORBIDDEN,
            "已达到订阅允许的隧道数量上限",
        )));
    }
    let (remote_port, reservation) = allocate_port(
        &state,
        pg,
        &node,
        request.node_id,
        &request.protocol,
        shared_http,
        request.remote_port,
        subscription.allow_custom_port,
    )
    .await?;
    let mut cache_reservation = reservation;
    let tunnel = match sqlx::query_as::<_, TunnelRow>(
        "INSERT INTO tunnels (id, user_id, client_id, node_id, name, carrier, protocol, remote_port, port_custom, target_host, target_port, target_url, http_access, https_enabled, cert_id, host_rewrite, backend_tls_insecure, speed_limit_mbps, max_conns, max_new_conns_per_sec, status) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21) RETURNING id, user_id, NULL::text AS user_email, client_id, NULL::text AS client_name, node_id, NULL::text AS node_name, name, carrier, protocol, remote_port, target_host, target_port, target_url, status, enabled, speed_limit_mbps, max_conns, max_new_conns_per_sec, last_error",
    )
    .bind(id)
    .bind(owner_user_id)
    .bind(request.client_id)
    .bind(request.node_id)
    .bind(name)
    .bind(&request.carrier)
    .bind(&request.protocol)
    .bind(remote_port.map(i32::from))
    .bind(request.remote_port.is_some())
    .bind(target_host)
    .bind(target_port)
    .bind(target_url)
    .bind(http_access)
    .bind(https_enabled)
    .bind(cert_id)
    .bind(host_rewrite)
    .bind(backend_tls_insecure)
    .bind(subscription.speed_limit_mbps)
    .bind(subscription.max_conns_per_tunnel)
    .bind(subscription.max_new_conns_per_sec)
    .bind(final_status)
    .fetch_one(&mut *transaction)
    .await {
        Ok(tunnel) => tunnel,
        Err(_) => {
            let _ = transaction.rollback().await;
            rollback_port_reservation(cache_reservation.take(), request.node_id, &state, pg).await;
            return Err(response_error(unavailable(StatusCode::CONFLICT, "隧道创建失败；请检查名称、端口和域名是否冲突")));
        }
    };
    for (domain, approved) in prepared_domains {
        let cooldown_owner = match sqlx::query_scalar::<_, Uuid>(
            "SELECT user_id FROM tunnel_domains WHERE domain = $1 AND status IN ('rejected', 'cooldown') AND cooldown_until > now() FOR UPDATE",
        )
        .bind(&domain)
        .fetch_optional(&mut *transaction)
        .await
        {
            Ok(owner) => owner,
            Err(_) => {
                let _ = transaction.rollback().await;
                rollback_port_reservation(cache_reservation.take(), request.node_id, &state, pg).await;
                return Err(response_error(unavailable(StatusCode::SERVICE_UNAVAILABLE, "数据库暂不可用")));
            }
        };
        if cooldown_owner.is_some_and(|owner| owner != owner_user_id) {
            let _ = transaction.rollback().await;
            rollback_port_reservation(cache_reservation.take(), request.node_id, &state, pg).await;
            return Err(response_error(unavailable(StatusCode::CONFLICT, "该域名仍在冷却期内，只有原用户可以重新绑定")));
        }
        let domain_status = if approved { "approved" } else { "pending_review" };
        let domain_write: Result<(), sqlx::Error> = if cooldown_owner.is_some() {
            sqlx::query("UPDATE tunnel_domains SET tunnel_id = $1, status = $2, reviewed_by = NULL, reviewed_at = NULL, cooldown_until = NULL, created_at = now() WHERE domain = $3")
                .bind(id)
                .bind(domain_status)
                .bind(&domain)
                .execute(&mut *transaction)
                .await
                .map(|_| ())
        } else {
            match sqlx::query("DELETE FROM tunnel_domains WHERE domain = $1 AND status IN ('rejected', 'cooldown') AND cooldown_until <= now()")
                .bind(&domain)
                .execute(&mut *transaction)
                .await
            {
                Ok(_) => sqlx::query("INSERT INTO tunnel_domains (id, tunnel_id, user_id, domain, status) VALUES ($1, $2, $3, $4, $5)")
                    .bind(Uuid::new_v4())
                    .bind(id)
                    .bind(owner_user_id)
                    .bind(&domain)
                    .bind(domain_status)
                    .execute(&mut *transaction)
                    .await
                    .map(|_| ()),
                Err(error) => Err(error),
            }
        };
        if domain_write.is_err() {
            let _ = transaction.rollback().await;
            rollback_port_reservation(cache_reservation.take(), request.node_id, &state, pg).await;
            return Err(response_error(unavailable(StatusCode::CONFLICT, "域名已被占用或审核记录创建失败")));
        }
    }
    if let Err(error) = write_audit(
        &mut transaction,
        actor_id,
        "tunnel.create",
        "tunnel",
        id,
    )
    .await
    {
        let _ = transaction.rollback().await;
        rollback_port_reservation(cache_reservation.take(), request.node_id, &state, pg).await;
        return Err(error);
    }
    if let Err(error) = transaction.commit().await {
        if let Some((cache, protocol, _, _)) = cache_reservation.take() {
            invalidate_port_cache(&cache, request.node_id, protocol, &state, pg).await;
        }
        return Err(database_error(error));
    }
    if let Some((cache, protocol, port, token)) = cache_reservation.take() {
        if !matches!(
            cache
                .commit_reservation(request.node_id, protocol, port, token)
                .await,
            Ok(true)
        ) {
            invalidate_port_cache(&cache, request.node_id, protocol, &state, pg).await;
        }
    }
    let ctl_pg = state.pg_control().unwrap_or_else(|| pg.clone());
    crate::ws::defer_on_tunnel_changed(
        ctl_pg,
        tunnel.node_id,
        tunnel.client_id,
        tunnel.id,
    );
    Ok((StatusCode::CREATED, Json(tunnel)))
}

async fn allocate_port(
    state: &AppState,
    pg: &PgPool,
    node: &TunnelNode,
    node_id: Uuid,
    protocol: &str,
    shared_http: bool,
    requested: Option<u16>,
    allow_custom: bool,
) -> Result<
    (
        Option<u16>,
        Option<(RedisPortCache, PortProtocol, u16, Uuid)>,
    ),
    (StatusCode, Json<Value>),
> {
    if shared_http {
        return Ok((None, None));
    }
    if requested.is_some() && !allow_custom {
        return Err(response_error(unavailable(
            StatusCode::FORBIDDEN,
            "当前订阅不允许自定义公网端口",
        )));
    }
    let l4 = if protocol == "udp" {
        PortProtocol::Udp
    } else {
        PortProtocol::Tcp
    };
    let ranges = if l4 == PortProtocol::Udp {
        store::decode_port_ranges(node.udp_port_ranges.clone())
    } else {
        store::decode_port_ranges(node.tcp_port_ranges.clone())
    }
    .map_err(super::config_data_error)?;
    let exclusions =
        store::decode_port_ranges(node.port_exclude.clone()).map_err(super::config_data_error)?;
    let mut fixed = if l4 == PortProtocol::Udp {
        store::reserved_ports_for_socket(&node.carrier_ports, "udp")
    } else {
        let mut ports = store::reserved_ports_for_socket(&node.carrier_ports, "tcp");
        if node.protocols.iter().any(|item| item == "http") {
            ports.push(node.http_shared_port as u16);
            ports.push(node.https_shared_port as u16);
        }
        ports
    };
    let fixed = PortRanges::new(
        fixed
            .drain(..)
            .map(|port| PortRange {
                start: port,
                end: port,
            })
            .collect(),
    )
    .unwrap_or_else(|_| PortRanges::empty());
    let available = ranges.excludes(&exclusions).excludes(&fixed);
    if available.port_count() == 0 {
        return Err(response_error(unavailable(
            StatusCode::CONFLICT,
            "所选节点没有可用公网端口",
        )));
    }
    if let Some(port) = requested {
        if !available.contains(port) {
            return Err(response_error(unavailable(
                StatusCode::BAD_REQUEST,
                "公网端口不在该节点可分配区间内，或属于保留端口",
            )));
        }
    }
    if let Some(redis) = state.redis_connection() {
        let cache = RedisPortCache::new(redis);
        let token = reservation_token();
        let reserved = match requested {
            Some(port) => cache
                .reserve_custom(node_id, l4, port, token)
                .await
                .map(|accepted| accepted.then_some(port)),
            None => cache.reserve_random(node_id, l4, &available, token).await,
        };
        match reserved {
            Ok(Some(port)) => return Ok((Some(port), Some((cache, l4, port, token)))),
            Ok(None) if requested.is_some() => {
                return Err(response_error(unavailable(
                    StatusCode::CONFLICT,
                    "公网端口已被占用",
                )));
            }
            Ok(None) => {
                return Err(response_error(unavailable(
                    StatusCode::CONFLICT,
                    "所选节点端口区间已用尽",
                )));
            }
            Err(PortCacheError::NotReady) => {}
            Err(PortCacheError::Redis(_)) => {
                invalidate_port_cache(&cache, node_id, l4, state, pg).await;
            }
            Err(PortCacheError::Randomness(_)) => {
                return Err(response_error(unavailable(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "无法选择公网端口",
                )));
            }
        }
    }
    allocate_port_from_pg(pg, node_id, l4, &available, requested)
        .await
        .map(|port| (Some(port), None))
}

async fn rollback_port_reservation(
    reservation: Option<(RedisPortCache, PortProtocol, u16, Uuid)>,
    node_id: Uuid,
    state: &AppState,
    pg: &PgPool,
) {
    if let Some((cache, protocol, port, token)) = reservation {
        if !matches!(
            cache
                .rollback_reservation(node_id, protocol, port, token)
                .await,
            Ok(true)
        ) {
            invalidate_port_cache(&cache, node_id, protocol, state, pg).await;
        }
    }
}

async fn invalidate_port_cache(
    cache: &RedisPortCache,
    node_id: Uuid,
    protocol: PortProtocol,
    state: &AppState,
    pg: &PgPool,
) {
    if cache.invalidate(node_id, protocol).await.is_err() {
        state.disable_redis();
    }
    let _ = sqlx::query(
        "INSERT INTO redis_outbox (operation, payload) VALUES ('rebuild_node_ports', $1)",
    )
    .bind(serde_json::json!({ "node_id": node_id }))
    .execute(pg)
    .await;
}

async fn allocate_port_from_pg(
    pg: &PgPool,
    node_id: Uuid,
    l4: PortProtocol,
    ranges: &PortRanges,
    requested: Option<u16>,
) -> Result<u16, (StatusCode, Json<Value>)> {
    let query = if l4 == PortProtocol::Udp {
        "SELECT remote_port FROM tunnels WHERE node_id = $1 AND protocol = 'udp' AND remote_port IS NOT NULL AND status <> 'deleted'"
    } else {
        "SELECT remote_port FROM tunnels WHERE node_id = $1 AND protocol <> 'udp' AND remote_port IS NOT NULL AND status <> 'deleted'"
    };
    let occupied = sqlx::query_scalar::<_, i32>(query)
        .bind(node_id)
        .fetch_all(pg)
        .await
        .map_err(database_error)?
        .into_iter()
        .filter_map(|port| u16::try_from(port).ok())
        .collect::<HashSet<_>>();
    let port = match requested {
        Some(port) if !occupied.contains(&port) => port,
        Some(_) => {
            return Err(response_error(unavailable(
                StatusCode::CONFLICT,
                "公网端口已被占用",
            )));
        }
        None => choose_unoccupied_port(ranges, &occupied).ok_or_else(|| {
            response_error(unavailable(StatusCode::CONFLICT, "所选节点端口区间已用尽"))
        })?,
    };
    Ok(port)
}

fn choose_unoccupied_port(ranges: &PortRanges, occupied: &HashSet<u16>) -> Option<u16> {
    let total = ranges.port_count();
    if total == 0 {
        return None;
    }
    let mut seed = [0u8; 8];
    getrandom::fill(&mut seed).ok()?;
    let start = u64::from_ne_bytes(seed) % u64::from(total);
    for offset in 0..u64::from(total) {
        let mut ordinal = ((start + offset) % u64::from(total)) as u32;
        for range in ranges.ranges() {
            let length = u32::from(range.end) - u32::from(range.start) + 1;
            if ordinal < length {
                let candidate = u16::try_from(u32::from(range.start) + ordinal).ok()?;
                if !occupied.contains(&candidate) {
                    return Some(candidate);
                }
                break;
            }
            ordinal -= length;
        }
    }
    None
}

fn normalize_domain(domain: &str, allow_ip: bool) -> Result<String, (StatusCode, Json<Value>)> {
    let domain = domain
        .trim()
        .trim_end_matches('.')
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    if let Ok(ip) = IpAddr::from_str(&domain) {
        if allow_ip {
            return Ok(ip.to_string());
        }
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "共享入口不能填写 IP，请使用域名",
        )));
    }
    let parsed = Url::parse(&format!("http://{domain}"))
        .map_err(|_| response_error(unavailable(StatusCode::BAD_REQUEST, "域名格式无效")))?;
    if domain.is_empty()
        || domain.len() > 253
        || parsed.host_str() != Some(domain.as_str())
        || parsed.port().is_some()
    {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "域名格式无效",
        )));
    }
    Ok(domain)
}

fn carrier_enabled(carrier_ports: &Value, carrier: &str) -> bool {
    carrier_ports
        .get(carrier)
        .and_then(|config| config.get("enabled"))
        .and_then(Value::as_bool)
        == Some(true)
}

fn supports_carrier(capabilities: &Value, carrier: &str) -> bool {
    capabilities
        .get("carriers")
        .and_then(Value::as_array)
        .is_some_and(|carriers| carriers.iter().any(|value| value.as_str() == Some(carrier)))
}

fn validate_target_url(value: &str) -> Result<(), (StatusCode, Json<Value>)> {
    if value.is_empty() || value.len() > 2048 {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "HTTP 转发目标 URL 长度无效",
        )));
    }
    let url = Url::parse(value).map_err(|_| {
        response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "HTTP 转发目标 URL 格式无效",
        ))
    })?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "HTTP 转发 URL 仅支持 http/https，不允许内嵌账号密码或 fragment",
        )));
    }
    Ok(())
}
