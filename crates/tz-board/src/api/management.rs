use super::{database_error, response_error};
use crate::setup::{AppState, UserRole, unavailable};
use axum::{Json, Router, extract::{Path, Query, State}, http::{HeaderMap, StatusCode}, routing::{get, post, put}};
use chrono::{Datelike, Duration, FixedOffset, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::FromRow;
use std::sync::Arc;
use uuid::Uuid;

pub fn admin_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/admin/dashboard", get(dashboard))
        .route("/api/v1/admin/settings", get(list_settings).post(set_setting))
        .route("/api/v1/admin/settings/mail/test", post(test_mail))
        .route(
            "/api/v1/admin/security/global",
            get(get_global_security).put(update_global_security),
        )
        .route("/api/v1/admin/security/events", get(list_guard_events))
        .route("/api/v1/admin/security", get(list_node_security))
        .route("/api/v1/admin/security/{node_id}", put(update_node_security))
        .route("/api/v1/admin/audit-logs", get(list_audit_logs))
}

#[derive(Debug, Serialize, FromRow)]
struct DashboardCounts {
    users: i64,
    active_subscriptions: i64,
    nodes: i64,
    nodes_online: i64,
    clients: i64,
    clients_online: i64,
    tunnels: i64,
    tunnels_active: i64,
}

#[derive(Debug, Serialize)]
struct Dashboard {
    users: i64,
    active_subscriptions: i64,
    nodes: i64,
    nodes_online: i64,
    clients: i64,
    clients_online: i64,
    tunnels: i64,
    tunnels_active: i64,
    connections_tcp: u64,
    connections_udp: u64,
    traffic_today_bytes: i64,
    traffic_24h_bytes: i64,
    traffic_month_bytes: i64,
    bandwidth_in_bps: u64,
    bandwidth_out_bps: u64,
    hourly: Vec<TrafficPoint>,
    daily: Vec<TrafficPoint>,
    protocols: Vec<ProtocolCount>,
    nodes_load: Vec<NodeLoad>,
    events: Vec<DashboardEvent>,
}

#[derive(Debug, Serialize)]
struct TrafficPoint {
    at: i64,
    bytes_in: i64,
    bytes_out: i64,
}

#[derive(Debug, Serialize, FromRow)]
struct ProtocolCount {
    protocol: String,
    count: i64,
}

#[derive(Debug, Serialize)]
struct NodeLoad {
    name: String,
    connections_tcp: u32,
    connections_udp: u32,
}

#[derive(Debug, Serialize, FromRow)]
struct DashboardEvent {
    id: i64,
    node_name: String,
    rule: String,
    tunnel_id: Option<Uuid>,
    intensity: i32,
    duration_secs: i64,
    last_seen_at: String,
}

#[derive(Debug, FromRow)]
struct TrafficWindows {
    today: i64,
    last_24h: i64,
    month: i64,
}

#[derive(Debug, FromRow)]
struct BucketRow {
    bucket_unix: i64,
    bytes_in: i64,
    bytes_out: i64,
}

#[derive(Debug, FromRow)]
struct OnlineNode {
    id: Uuid,
    name: String,
}

#[derive(Debug, Serialize, FromRow)]
struct SettingRow {
    key: String,
    value: Value,
    updated_at: String,
}

#[derive(Deserialize)]
struct SetSetting {
    key: String,
    value: Value,
}

#[derive(Deserialize)]
struct MailTestRequest {
    to: String,
}

#[derive(Debug, Serialize, FromRow)]
struct NodeSecurityRow {
    id: Uuid,
    name: String,
    guard_policy: Value,
    trusted_proxies: Vec<String>,
}

#[derive(Debug, Serialize)]
struct NodeSecurityItem {
    id: Uuid,
    name: String,
    /// 节点覆盖层（未合并）。
    guard_policy: Value,
    /// 全局 + 节点 deep-merge 后的生效策略（覆盖禁用时等于全局）。
    effective_policy: Value,
    /// 是否已配置独立 Server 策略。
    has_overlay: bool,
    /// 独立策略是否启用；禁用时继承全局。
    overlay_enabled: bool,
    trusted_proxies: Vec<String>,
}

#[derive(Debug, Serialize)]
struct GlobalSecurity {
    guard_policy: Value,
}

#[derive(Debug, Deserialize)]
struct SetGlobalSecurity {
    guard_policy: Value,
}

#[derive(Debug, Deserialize)]
struct GuardEventsQuery {
    node_id: Option<Uuid>,
    #[serde(flatten)]
    page: crate::page::PageQuery,
}

#[derive(Debug, Serialize, FromRow)]
struct GuardEventRow {
    id: i64,
    node_id: Uuid,
    node_name: String,
    rule: String,
    tunnel_id: Option<Uuid>,
    intensity: i32,
    duration_secs: i64,
    first_seen_at: String,
    last_seen_at: String,
}

#[derive(Debug, Deserialize)]
struct AuditQuery {
    #[serde(flatten)]
    page: crate::page::PageQuery,
}

#[derive(Debug, Serialize, FromRow)]
struct AuditRow {
    id: i64,
    actor_id: Option<Uuid>,
    actor_email: Option<String>,
    action: String,
    target_type: String,
    target_id: Option<String>,
    details: Value,
    remote_ip: Option<String>,
    created_at: String,
}

fn shanghai() -> FixedOffset {
    FixedOffset::east_opt(8 * 3600).expect("shanghai offset")
}

fn shanghai_day_start(now: chrono::DateTime<FixedOffset>) -> chrono::DateTime<Utc> {
    now.date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|naive| naive.and_local_timezone(shanghai()).single())
        .map(|local| local.with_timezone(&Utc))
        .unwrap_or_else(|| Utc::now())
}

fn shanghai_month_start(now: chrono::DateTime<FixedOffset>) -> chrono::DateTime<Utc> {
    shanghai()
        .with_ymd_and_hms(now.year(), now.month(), 1, 0, 0, 0)
        .single()
        .map(|local| local.with_timezone(&Utc))
        .unwrap_or_else(|| Utc::now())
}

fn fill_buckets(rows: Vec<BucketRow>, start: i64, step: i64, count: usize) -> Vec<TrafficPoint> {
    let mut indexed = std::collections::HashMap::with_capacity(rows.len());
    for row in rows {
        indexed.insert(row.bucket_unix, (row.bytes_in, row.bytes_out));
    }
    (0..count)
        .map(|index| {
            let at = start + step * index as i64;
            let (bytes_in, bytes_out) = indexed.get(&at).copied().unwrap_or((0, 0));
            TrafficPoint {
                at,
                bytes_in,
                bytes_out,
            }
        })
        .collect()
}

fn add_pending(points: &mut [TrafficPoint], bucket: i64, bytes_in: i64, bytes_out: i64) {
    let Some(point) = points.iter_mut().find(|point| point.at == bucket) else {
        return;
    };
    point.bytes_in = point.bytes_in.saturating_add(bytes_in);
    point.bytes_out = point.bytes_out.saturating_add(bytes_out);
}

async fn dashboard(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Dashboard>, (StatusCode, Json<Value>)> {
    let (pg, _) = state.database_for(&headers, UserRole::Admin).await.map_err(response_error)?;
    let counts = sqlx::query_as::<_, DashboardCounts>(
        "SELECT (SELECT COUNT(*) FROM users WHERE role = 'user') AS users, (SELECT COUNT(*) FROM user_subscriptions WHERE status = 'active' AND starts_at <= now() AND (expires_at IS NULL OR expires_at > now())) AS active_subscriptions, (SELECT COUNT(*) FROM nodes WHERE enabled = TRUE) AS nodes, (SELECT COUNT(*) FROM nodes WHERE enabled = TRUE AND online = TRUE) AS nodes_online, (SELECT COUNT(*) FROM clients WHERE enabled = TRUE) AS clients, (SELECT COUNT(*) FROM clients WHERE enabled = TRUE AND online = TRUE) AS clients_online, (SELECT COUNT(*) FROM tunnels WHERE status <> 'deleted') AS tunnels, (SELECT COUNT(*) FROM tunnels WHERE status = 'active') AS tunnels_active",
    )
    .fetch_one(&pg)
    .await
    .map_err(database_error)?;

    let now_sh = Utc::now().with_timezone(&shanghai());
    let today_from = shanghai_day_start(now_sh);
    let month_from = shanghai_month_start(now_sh);
    let last_24h_from = Utc::now() - Duration::hours(24);
    let window_from = month_from.min(last_24h_from);
    let windows = sqlx::query_as::<_, TrafficWindows>(
        "SELECT
            COALESCE(SUM(bytes_in + bytes_out) FILTER (WHERE hour_start >= $1), 0)::bigint AS today,
            COALESCE(SUM(bytes_in + bytes_out) FILTER (WHERE hour_start >= $2), 0)::bigint AS last_24h,
            COALESCE(SUM(bytes_in + bytes_out) FILTER (WHERE hour_start >= $3), 0)::bigint AS month
         FROM tunnel_traffic_hourly
         WHERE hour_start >= $4",
    )
    .bind(today_from)
    .bind(last_24h_from)
    .bind(month_from)
    .bind(window_from)
    .fetch_one(&pg)
    .await
    .map_err(database_error)?;

    let now_ts = Utc::now().timestamp();
    let hour_end = now_ts - now_ts.rem_euclid(3600);
    let hour_start = hour_end - 23 * 3600;
    let hour_from = Utc.timestamp_opt(hour_start, 0).single().unwrap_or_else(Utc::now);
    let hourly_rows = sqlx::query_as::<_, BucketRow>(
        "SELECT EXTRACT(EPOCH FROM (date_trunc('hour', hour_start AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'))::bigint AS bucket_unix,
                COALESCE(SUM(bytes_in), 0)::bigint AS bytes_in,
                COALESCE(SUM(bytes_out), 0)::bigint AS bytes_out
         FROM tunnel_traffic_hourly
         WHERE hour_start >= $1
         GROUP BY 1
         ORDER BY 1",
    )
    .bind(hour_from)
    .fetch_all(&pg)
    .await
    .map_err(database_error)?;

    let day_start = today_from.timestamp() - 13 * 86_400;
    let day_from = Utc.timestamp_opt(day_start, 0).single().unwrap_or(today_from);
    let daily_rows = sqlx::query_as::<_, BucketRow>(
        "SELECT EXTRACT(EPOCH FROM (date_trunc('day', hour_start AT TIME ZONE 'Asia/Shanghai') AT TIME ZONE 'Asia/Shanghai'))::bigint AS bucket_unix,
                COALESCE(SUM(bytes_in), 0)::bigint AS bytes_in,
                COALESCE(SUM(bytes_out), 0)::bigint AS bytes_out
         FROM tunnel_traffic_hourly
         WHERE hour_start >= $1
         GROUP BY 1
         ORDER BY 1",
    )
    .bind(day_from)
    .fetch_all(&pg)
    .await
    .map_err(database_error)?;

    let protocols = sqlx::query_as::<_, ProtocolCount>(
        "SELECT protocol, COUNT(*)::bigint AS count FROM tunnels WHERE status <> 'deleted' GROUP BY protocol ORDER BY count DESC, protocol",
    )
    .fetch_all(&pg)
    .await
    .map_err(database_error)?;

    let events = sqlx::query_as::<_, DashboardEvent>(
        "SELECT e.id, n.name AS node_name, e.rule, e.tunnel_id,
                e.hit_count AS intensity,
                GREATEST(0, EXTRACT(EPOCH FROM (e.last_seen_at - e.first_seen_at))::bigint) AS duration_secs,
                e.last_seen_at::text AS last_seen_at
         FROM guard_events e JOIN nodes n ON n.id = e.node_id
         ORDER BY e.last_seen_at DESC
         LIMIT 6",
    )
    .fetch_all(&pg)
    .await
    .map_err(database_error)?;

    let online_nodes = sqlx::query_as::<_, OnlineNode>(
        "SELECT id, name FROM nodes WHERE enabled = TRUE AND online = TRUE ORDER BY name",
    )
    .fetch_all(&pg)
    .await
    .map_err(database_error)?;

    let (pending_in, pending_out) = crate::stats::pending_traffic_total();
    let pending_total = pending_in.saturating_add(pending_out);
    let mut hourly = fill_buckets(hourly_rows, hour_start, 3600, 24);
    let mut daily = fill_buckets(daily_rows, day_start, 86_400, 14);
    add_pending(&mut hourly, hour_end, pending_in, pending_out);
    add_pending(&mut daily, today_from.timestamp(), pending_in, pending_out);

    let mut connections_tcp = 0u64;
    let mut connections_udp = 0u64;
    let mut nodes_load: Vec<NodeLoad> = online_nodes
        .into_iter()
        .map(|node| {
            let metrics = crate::node_metrics::snapshot(node.id);
            let tcp = metrics.as_ref().map(|item| item.connections_tcp).unwrap_or(0);
            let udp = metrics.as_ref().map(|item| item.connections_udp).unwrap_or(0);
            connections_tcp = connections_tcp.saturating_add(u64::from(tcp));
            connections_udp = connections_udp.saturating_add(u64::from(udp));
            NodeLoad {
                name: node.name,
                connections_tcp: tcp,
                connections_udp: udp,
            }
        })
        .collect();
    nodes_load.sort_by(|left, right| {
        let left_total = u64::from(left.connections_tcp) + u64::from(left.connections_udp);
        let right_total = u64::from(right.connections_tcp) + u64::from(right.connections_udp);
        right_total.cmp(&left_total).then_with(|| left.name.cmp(&right.name))
    });
    nodes_load.truncate(8);

    let (bandwidth_in_bps, bandwidth_out_bps) = crate::tunnel_traffic::live_totals();
    Ok(Json(Dashboard {
        users: counts.users,
        active_subscriptions: counts.active_subscriptions,
        nodes: counts.nodes,
        nodes_online: counts.nodes_online,
        clients: counts.clients,
        clients_online: counts.clients_online,
        tunnels: counts.tunnels,
        tunnels_active: counts.tunnels_active,
        connections_tcp,
        connections_udp,
        traffic_today_bytes: windows.today.saturating_add(pending_total),
        traffic_24h_bytes: windows.last_24h.saturating_add(pending_total),
        traffic_month_bytes: windows.month.saturating_add(pending_total),
        bandwidth_in_bps,
        bandwidth_out_bps,
        hourly,
        daily,
        protocols,
        nodes_load,
        events,
    }))
}

async fn list_settings(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Vec<SettingRow>>, (StatusCode, Json<Value>)> {
    let (pg, _) = state.database_for(&headers, UserRole::Admin).await.map_err(response_error)?;
    let rows = sqlx::query_as::<_, SettingRow>(
        "SELECT key, value, updated_at::text AS updated_at FROM system_settings ORDER BY key",
    )
    .fetch_all(&pg)
    .await
    .map_err(database_error)?;
    let rows = rows
        .into_iter()
        .filter(|row| !crate::system_settings::is_hidden_setting_key(&row.key))
        .map(|row| SettingRow {
            value: crate::system_settings::redact_setting_value(&row.key, row.value),
            ..row
        })
        .collect();
    Ok(Json(rows))
}

async fn set_setting(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    client_ip: crate::client_ip::ClientIp,
    Json(setting): Json<SetSetting>,
) -> Result<Json<SettingRow>, (StatusCode, Json<Value>)> {
    let (pg, actor) = state.database_for(&headers, UserRole::Admin).await.map_err(response_error)?;
    if crate::system_settings::is_hidden_setting_key(&setting.key)
        || !crate::system_settings::validate_writable_setting(&setting.key, &setting.value)
    {
        return Err(response_error(unavailable(StatusCode::BAD_REQUEST, "不允许修改该设置或值无效")));
    }
    // SMTP 密码传空字符串：视为不修改，直接返回当前脱敏值
    if setting.key == "mail_smtp_password" && setting.value.as_str().is_some_and(str::is_empty) {
        let current = sqlx::query_as::<_, SettingRow>(
            "SELECT key, value, updated_at::text AS updated_at FROM system_settings WHERE key = $1",
        )
        .bind(&setting.key)
        .fetch_optional(&pg)
        .await
        .map_err(database_error)?;
        let row = current.unwrap_or(SettingRow {
            key: setting.key.clone(),
            value: serde_json::json!(""),
            updated_at: String::new(),
        });
        return Ok(Json(SettingRow {
            value: crate::system_settings::redact_setting_value(&row.key, row.value),
            ..row
        }));
    }
    let mut transaction = pg.begin().await.map_err(database_error)?;
    let row = sqlx::query_as::<_, SettingRow>(
        "INSERT INTO system_settings (key, value) VALUES ($1, $2) ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now() RETURNING key, value, updated_at::text AS updated_at",
    )
    .bind(&setting.key)
    .bind(&setting.value)
    .fetch_one(&mut *transaction)
    .await
    .map_err(database_error)?;
    let audit_value = if setting.key == "mail_smtp_password" {
        serde_json::json!({ "set": true })
    } else {
        setting.value.clone()
    };
    sqlx::query("INSERT INTO audit_logs (actor_id, action, target_type, target_id, details, remote_ip) VALUES ($1, 'setting.update', 'setting', $2, $3, $4::inet)")
        .bind(actor.user_id)
        .bind(&setting.key)
        .bind(&audit_value)
        .bind(client_ip.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
    transaction.commit().await.map_err(database_error)?;
    if crate::system_settings::is_host_metrics_interval_key(&setting.key) {
        crate::ws::push_node_configs_to_online_nodes(&pg).await;
    }
    Ok(Json(SettingRow {
        value: crate::system_settings::redact_setting_value(&row.key, row.value),
        ..row
    }))
}

async fn test_mail(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<MailTestRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (pg, _) = state.database_for(&headers, UserRole::Admin).await.map_err(response_error)?;
    crate::mail::send_test_mail(&pg, &body.to)
        .await
        .map_err(|error| response_error(unavailable(StatusCode::BAD_REQUEST, error.to_string())))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn get_global_security(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<GlobalSecurity>, (StatusCode, Json<Value>)> {
    let (pg, _) = state.database_for(&headers, UserRole::Admin).await.map_err(response_error)?;
    Ok(Json(GlobalSecurity {
        guard_policy: crate::guard_policy::load_global_guard_policy(&pg).await,
    }))
}

async fn update_global_security(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    client_ip: crate::client_ip::ClientIp,
    Json(update): Json<SetGlobalSecurity>,
) -> Result<Json<GlobalSecurity>, (StatusCode, Json<Value>)> {
    let (pg, actor) = state.database_for(&headers, UserRole::Admin).await.map_err(response_error)?;
    validate_guard_policy(&update.guard_policy)?;
    let mut transaction = pg.begin().await.map_err(database_error)?;
    crate::guard_policy::save_global_guard_policy(&mut *transaction, &update.guard_policy)
        .await
        .map_err(database_error)?;
    sqlx::query(
        "INSERT INTO audit_logs (actor_id, action, target_type, target_id, details, remote_ip)
         VALUES ($1, 'security.global.update', 'global_guard_policy', '1', $2, $3::inet)",
    )
    .bind(actor.user_id)
    .bind(&update.guard_policy)
    .bind(client_ip.as_str())
    .execute(&mut *transaction)
    .await
    .map_err(database_error)?;
    // 全局策略变更必须抬升各节点 revision，否则 server 因 revision 未变会跳过 guard.reload。
    sqlx::query("UPDATE nodes SET config_revision = config_revision + 1, updated_at = now()")
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
    transaction.commit().await.map_err(database_error)?;
    crate::ws::push_node_configs_to_online_nodes(&pg).await;
    Ok(Json(GlobalSecurity {
        guard_policy: update.guard_policy,
    }))
}

#[derive(Debug, Deserialize)]
struct NodeSecurityQuery {
    overlay: Option<String>,
    #[serde(flatten)]
    page: crate::page::PageQuery,
}

async fn list_node_security(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<NodeSecurityQuery>,
) -> Result<Json<crate::page::Page<NodeSecurityItem>>, (StatusCode, Json<Value>)> {
    let (pg, _) = state.database_for(&headers, UserRole::Admin).await.map_err(response_error)?;
    let (page, page_size, offset) = query.page.resolve();
    let global = crate::guard_policy::load_global_guard_policy(&pg).await;
    let rows = sqlx::query_as::<_, NodeSecurityRow>(
        "SELECT id, name, guard_policy, trusted_proxies::text[] AS trusted_proxies FROM nodes ORDER BY name",
    )
    .fetch_all(&pg)
    .await
    .map_err(database_error)?;
    let items: Vec<NodeSecurityItem> = rows
        .into_iter()
        .map(|row| {
            let has_overlay = crate::guard_policy::has_node_overlay(&row.guard_policy);
            let overlay_enabled = crate::guard_policy::node_overlay_enabled(&row.guard_policy);
            let effective_policy = if has_overlay && overlay_enabled {
                crate::guard_policy::merge_guard_policy(
                    &global,
                    &crate::guard_policy::strip_policy_meta(&row.guard_policy),
                )
            } else {
                global.clone()
            };
            NodeSecurityItem {
                effective_policy,
                has_overlay,
                overlay_enabled,
                id: row.id,
                name: row.name,
                guard_policy: row.guard_policy,
                trusted_proxies: row.trusted_proxies,
            }
        })
        .collect();
    let items = match query.overlay.as_deref() {
        Some("only") => items.into_iter().filter(|item| item.has_overlay).collect(),
        Some("none") => items.into_iter().filter(|item| !item.has_overlay).collect(),
        _ => items,
    };
    let total = items.len() as i64;
    let start = offset as usize;
    let page_items = items.into_iter().skip(start).take(page_size as usize).collect();
    Ok(Json(crate::page::Page::new(page_items, total, page, page_size)))
}

#[derive(Deserialize)]
struct SetNodeSecurity {
    guard_policy: Value,
    trusted_proxies: Vec<String>,
}

async fn update_node_security(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    client_ip: crate::client_ip::ClientIp,
    Path(node_id): Path<Uuid>,
    Json(update): Json<SetNodeSecurity>,
) -> Result<Json<NodeSecurityItem>, (StatusCode, Json<Value>)> {
    let (pg, actor) = state.database_for(&headers, UserRole::Admin).await.map_err(response_error)?;
    validate_guard_policy(&update.guard_policy)?;
    if update.trusted_proxies.len() > 4096 || update.trusted_proxies.iter().any(|cidr| !valid_cidr(cidr)) {
        return Err(response_error(unavailable(StatusCode::BAD_REQUEST, "可信代理 CIDR 列表无效")));
    }
    let mut transaction = pg.begin().await.map_err(database_error)?;
    let node = sqlx::query_as::<_, NodeSecurityRow>(
        "UPDATE nodes SET guard_policy = $2, trusted_proxies = string_to_array($3, ',')::cidr[], config_revision = config_revision + 1, updated_at = now() WHERE id = $1 RETURNING id, name, guard_policy, trusted_proxies::text[] AS trusted_proxies",
    )
    .bind(node_id)
    .bind(&update.guard_policy)
    .bind(update.trusted_proxies.join(","))
    .fetch_optional(&mut *transaction)
    .await
    .map_err(database_error)?
    .ok_or_else(|| response_error(unavailable(StatusCode::NOT_FOUND, "节点不存在")))?;
    sqlx::query("INSERT INTO audit_logs (actor_id, action, target_type, target_id, details, remote_ip) VALUES ($1, 'node.security.update', 'node', $2, $3, $4::inet)")
        .bind(actor.user_id)
        .bind(node_id.to_string())
        .bind(&update.guard_policy)
        .bind(client_ip.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
    transaction.commit().await.map_err(database_error)?;
    crate::ws::push_full_node_config(&pg, node_id).await;
    let global = crate::guard_policy::load_global_guard_policy(&pg).await;
    let has_overlay = crate::guard_policy::has_node_overlay(&node.guard_policy);
    let overlay_enabled = crate::guard_policy::node_overlay_enabled(&node.guard_policy);
    let effective_policy = if has_overlay && overlay_enabled {
        crate::guard_policy::merge_guard_policy(
            &global,
            &crate::guard_policy::strip_policy_meta(&node.guard_policy),
        )
    } else {
        global
    };
    Ok(Json(NodeSecurityItem {
        effective_policy,
        has_overlay,
        overlay_enabled,
        id: node.id,
        name: node.name,
        guard_policy: node.guard_policy,
        trusted_proxies: node.trusted_proxies,
    }))
}

async fn list_guard_events(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<GuardEventsQuery>,
) -> Result<Json<crate::page::Page<GuardEventRow>>, (StatusCode, Json<Value>)> {
    let (pg, _) = state.database_for(&headers, UserRole::Admin).await.map_err(response_error)?;
    let (page, page_size, offset) = query.page.resolve();
    let (total, rows) = if let Some(node_id) = query.node_id {
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*)::bigint FROM guard_events WHERE node_id = $1")
            .bind(node_id)
            .fetch_one(&pg)
            .await
            .map_err(database_error)?;
        let rows = sqlx::query_as::<_, GuardEventRow>(
            "SELECT e.id, e.node_id, n.name AS node_name, e.rule, e.tunnel_id,
                    e.hit_count AS intensity,
                    GREATEST(0, EXTRACT(EPOCH FROM (e.last_seen_at - e.first_seen_at))::bigint) AS duration_secs,
                    e.first_seen_at::text AS first_seen_at, e.last_seen_at::text AS last_seen_at
             FROM guard_events e JOIN nodes n ON n.id = e.node_id
             WHERE e.node_id = $1
             ORDER BY e.last_seen_at DESC LIMIT $2 OFFSET $3",
        )
        .bind(node_id)
        .bind(page_size)
        .bind(offset)
        .fetch_all(&pg)
        .await
        .map_err(database_error)?;
        (total, rows)
    } else {
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*)::bigint FROM guard_events")
            .fetch_one(&pg)
            .await
            .map_err(database_error)?;
        let rows = sqlx::query_as::<_, GuardEventRow>(
            "SELECT e.id, e.node_id, n.name AS node_name, e.rule, e.tunnel_id,
                    e.hit_count AS intensity,
                    GREATEST(0, EXTRACT(EPOCH FROM (e.last_seen_at - e.first_seen_at))::bigint) AS duration_secs,
                    e.first_seen_at::text AS first_seen_at, e.last_seen_at::text AS last_seen_at
             FROM guard_events e JOIN nodes n ON n.id = e.node_id
             ORDER BY e.last_seen_at DESC LIMIT $1 OFFSET $2",
        )
        .bind(page_size)
        .bind(offset)
        .fetch_all(&pg)
        .await
        .map_err(database_error)?;
        (total, rows)
    };
    Ok(Json(crate::page::Page::new(rows, total, page, page_size)))
}

fn validate_guard_policy(policy: &Value) -> Result<(), (StatusCode, Json<Value>)> {
    let Some(entries) = policy.as_object() else {
        return Err(response_error(unavailable(StatusCode::BAD_REQUEST, "防护策略必须是对象")));
    };
    if entries
        .keys()
        .any(|key| !crate::guard_policy::is_allowed_policy_key(key.as_str()))
    {
        return Err(response_error(unavailable(StatusCode::BAD_REQUEST, "防护策略包含未知模块")));
    }
    for (module, config) in entries {
        if !config.is_object() {
            return Err(response_error(unavailable(StatusCode::BAD_REQUEST, "防护模块配置必须是对象")));
        }
        if let Some(enabled) = config.get("enabled") {
            if !enabled.is_boolean() {
                return Err(response_error(unavailable(StatusCode::BAD_REQUEST, "防护模块 enabled 必须是布尔值")));
            }
        }
        if *module == "ip_acl" {
            for list in ["allow", "deny"] {
                if let Some(values) = config.get(list) {
                    let Some(values) = values.as_array() else {
                        return Err(response_error(unavailable(StatusCode::BAD_REQUEST, "IP ACL 必须是 CIDR 字符串数组")));
                    };
                    if values.len() > 100_000 || values.iter().any(|value| !value.as_str().is_some_and(valid_cidr)) {
                        return Err(response_error(unavailable(StatusCode::BAD_REQUEST, "IP ACL CIDR 无效或数量超限")));
                    }
                }
            }
        }
        for (key, value) in config.as_object().into_iter().flat_map(|object| object.iter()) {
            if key == "enabled" {
                continue;
            }
            if key == "max_tunnels" || key == "max_distinct_ips" || key == "max_header_bytes" || key == "max_headers"
                || key.ends_with("_per_sec") || key.ends_with("_rps") || key.ends_with("_flows") || key.ends_with("_conns")
                || key.ends_with("_pps")
            {
                // 0 = 不限制（适用于隧道数等）；其余限额 0..=1_000_000。
                if !value.as_u64().is_some_and(|limit| limit <= 1_000_000) {
                    return Err(response_error(unavailable(
                        StatusCode::BAD_REQUEST,
                        "防护限额必须在 0 到 1000000 之间",
                    )));
                }
            }
        }
    }
    Ok(())
}

fn valid_cidr(value: &str) -> bool {
    let Some((address, prefix)) = value.split_once('/') else {
        return value.parse::<std::net::IpAddr>().is_ok();
    };
    let Ok(address) = address.parse::<std::net::IpAddr>() else { return false };
    let Ok(prefix) = prefix.parse::<u8>() else { return false };
    match address {
        std::net::IpAddr::V4(_) => prefix <= 32,
        std::net::IpAddr::V6(_) => prefix <= 128,
    }
}

async fn list_audit_logs(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<AuditQuery>,
) -> Result<Json<crate::page::Page<AuditRow>>, (StatusCode, Json<Value>)> {
    let (pg, _) = state.database_for(&headers, UserRole::Admin).await.map_err(response_error)?;
    let (page, page_size, offset) = query.page.resolve();
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*)::bigint FROM audit_logs")
        .fetch_one(&pg)
        .await
        .map_err(database_error)?;
    let rows = sqlx::query_as::<_, AuditRow>(
        "SELECT a.id, a.actor_id, u.email AS actor_email, a.action, a.target_type, a.target_id, a.details, a.remote_ip::text AS remote_ip, a.created_at::text AS created_at FROM audit_logs a LEFT JOIN users u ON u.id = a.actor_id ORDER BY a.created_at DESC LIMIT $1 OFFSET $2",
    )
    .bind(page_size)
    .bind(offset)
    .fetch_all(&pg)
    .await
    .map_err(database_error)?;
    Ok(Json(crate::page::Page::new(rows, total, page, page_size)))
}
