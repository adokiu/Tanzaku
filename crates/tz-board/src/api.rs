mod certificates;
mod channels;
mod clients;
pub(crate) mod csrf;
mod domains;
mod live;
mod management;
mod me;
mod orders;
mod nodes;
pub(crate) mod payments;
mod plans;
mod themes;
mod tunnels;
mod users;

use crate::{
    port_cache::RedisPortCache,
    setup::{ApiErrorContext, AppState, UserRole, unavailable},
};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get, patch, post, put},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool};
use std::{net::IpAddr, str::FromStr, sync::Arc};
use tz_common::ports::PortRanges;
use uuid::Uuid;

pub fn admin_router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/v1/admin/node-groups",
            get(list_node_groups).post(create_node_group),
        )
        .route("/api/v1/admin/dataplane", get(dataplane_catalog))
        .route("/api/v1/admin/nodes", get(list_nodes).post(create_node))
        .route(
            "/api/v1/admin/nodes/{node_id}",
            get(get_admin_node)
                .put(update_admin_node)
                .delete(delete_admin_node),
        )
        .route(
            "/api/v1/admin/nodes/{node_id}/enabled",
            patch(set_admin_node_enabled),
        )
        .route(
            "/api/v1/admin/nodes/{node_id}/token",
            get(get_node_token).post(reset_node_token),
        )
        .merge(users::admin_router())
        .merge(plans::admin_router())
        .merge(clients::admin_router())
        .merge(tunnels::admin_router())
        .merge(certificates::admin_router())
        .merge(domains::admin_router())
        .merge(management::admin_router())
        .merge(themes::admin_router())
        .merge(orders::admin_router())
}

pub fn user_router() -> Router<Arc<AppState>> {
    Router::new()
        .merge(nodes::user_router())
        .merge(users::user_router())
        .merge(clients::user_router())
        .merge(tunnels::user_router())
        .merge(certificates::user_router())
        .merge(live::router())
        .merge(orders::user_router())
        .merge(me::user_router())
        .merge(channels::user_router())
}

#[derive(Debug, Serialize, FromRow)]
struct NodeGroupListItem {
    id: Uuid,
    name: String,
    enabled: bool,
}

#[derive(Debug, Deserialize)]
struct ListNodeGroupsQuery {
    q: Option<String>,
    #[serde(flatten)]
    page: crate::page::PageQuery,
}

#[derive(Debug, Deserialize)]
struct CreateNodeGroup {
    name: String,
}

#[derive(Debug, Serialize, FromRow)]
struct AdminNodeListRow {
    id: Uuid,
    name: String,
    region: String,
    public_host: String,
    enabled: bool,
    online: bool,
    tunnel_count: i64,
    last_seen_at: Option<String>,
}

#[derive(Debug, Serialize)]
struct AdminHostMetrics {
    cpu_usage_percent: u8,
    cpu_cores: u16,
    memory_used_bytes: u64,
    memory_total_bytes: u64,
    net_up_bps: u64,
    net_down_bps: u64,
    connections_tcp: u32,
    connections_udp: u32,
}

#[derive(Debug, Serialize)]
struct AdminNodeListItem {
    #[serde(flatten)]
    node: AdminNodeListRow,
    #[serde(skip_serializing_if = "Option::is_none")]
    host_metrics: Option<AdminHostMetrics>,
}

#[derive(Debug, Serialize, FromRow)]
struct AdminNodeDetailRow {
    id: Uuid,
    name: String,
    region: String,
    public_host: String,
    bind_addr: String,
    enabled: bool,
    online: bool,
    protocols: Vec<String>,
    carrier_ports: Value,
    tcp_port_ranges: Value,
    udp_port_ranges: Value,
    port_exclude: Value,
    http_shared_port: i32,
    https_shared_port: i32,
}

#[derive(Debug, Serialize)]
struct AdminNodeDetail {
    #[serde(flatten)]
    node: AdminNodeDetailRow,
    node_group_names: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct SetNodeEnabled {
    enabled: bool,
}

#[derive(Debug, Deserialize)]
struct CreateNode {
    name: String,
    region: Option<String>,
    public_host: String,
    bind_addr: Option<String>,
    #[serde(default)]
    node_group_names: Vec<String>,
    protocols: Vec<String>,
    carriers: Vec<NodeCarrierSetting>,
    tcp_port_ranges: String,
    udp_port_ranges: String,
    port_exclude: Option<String>,
    http_shared_port: Option<u16>,
    https_shared_port: Option<u16>,
}

#[derive(Debug, Deserialize)]
struct NodeCarrierSetting {
    kind: String,
    port: u16,
    #[serde(default = "default_carrier_enabled")]
    enabled: bool,
}

fn default_carrier_enabled() -> bool {
    true
}

#[derive(Debug, Serialize)]
struct NodeCreated {
    id: Uuid,
    token: String,
    token_prefix: String,
}

/// 节点 / Client 凭据再次查看：旧版创建的记录未保存明文，`token` 为 null，需重置。
#[derive(Debug, Serialize)]
pub(super) struct AgentTokenView {
    pub(super) token: Option<String>,
    pub(super) token_prefix: String,
}

/// 生成新的接入凭据：(明文, SHA-256, 前缀)。明文入库供面板再次查看与生成安装命令。
pub(super) fn new_agent_token(
    failure: &'static str,
) -> Result<(String, Vec<u8>, String), (StatusCode, Json<Value>)> {
    let mut token_bytes = [0u8; 32];
    getrandom::fill(&mut token_bytes)
        .map_err(|_| response_error(unavailable(StatusCode::INTERNAL_SERVER_ERROR, failure)))?;
    let token = token_bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let hash = Sha256::digest(token.as_bytes()).to_vec();
    let prefix = token.chars().take(12).collect::<String>();
    Ok((token, hash, prefix))
}

async fn get_node_token(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(node_id): Path<Uuid>,
) -> Result<Json<AgentTokenView>, (StatusCode, Json<Value>)> {
    let (pg, _) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    let row: Option<(Option<String>, String)> =
        sqlx::query_as("SELECT token, token_prefix FROM nodes WHERE id = $1")
            .bind(node_id)
            .fetch_optional(&pg)
            .await
            .map_err(database_error)?;
    let Some((token, token_prefix)) = row else {
        return Err(response_error(unavailable(StatusCode::NOT_FOUND, "节点不存在")));
    };
    Ok(Json(AgentTokenView { token, token_prefix }))
}

async fn reset_node_token(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(node_id): Path<Uuid>,
) -> Result<Json<AgentTokenView>, (StatusCode, Json<Value>)> {
    let (pg, actor) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    let (token, token_hash, token_prefix) = new_agent_token("无法生成节点凭据")?;
    let mut transaction = pg.begin().await.map_err(database_error)?;
    let updated = sqlx::query(
        "UPDATE nodes SET token = $2, token_hash = $3, token_prefix = $4, updated_at = now() WHERE id = $1",
    )
    .bind(node_id)
    .bind(&token)
    .bind(token_hash)
    .bind(&token_prefix)
    .execute(&mut *transaction)
    .await
    .map_err(database_error)?;
    if updated.rows_affected() == 0 {
        return Err(response_error(unavailable(StatusCode::NOT_FOUND, "节点不存在")));
    }
    write_audit(&mut transaction, actor.user_id, "node.token_reset", "node", node_id).await?;
    transaction.commit().await.map_err(database_error)?;
    Ok(Json(AgentTokenView {
        token: Some(token),
        token_prefix,
    }))
}

async fn dataplane_catalog(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let _ = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    let carriers: Vec<Value> = tz_carrier::registered_carriers()
        .into_iter()
        .map(|carrier| json!({ "kind": carrier.kind, "socket": carrier.socket }))
        .collect();
    Ok(Json(json!({
        "protocols": tz_ingress::registered_kinds(),
        "carriers": carriers,
    })))
}

async fn list_node_groups(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<ListNodeGroupsQuery>,
) -> Result<Json<crate::page::Page<NodeGroupListItem>>, (StatusCode, Json<Value>)> {
    let (pg, _) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    let (page, page_size, offset) = query.page.resolve();
    let needle = query
        .q
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| format!("%{value}%"));
    let (total, groups) = if let Some(needle) = needle {
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*)::bigint FROM node_groups WHERE name ILIKE $1")
            .bind(&needle)
            .fetch_one(&pg)
            .await
            .map_err(|_| response_error(unavailable(StatusCode::SERVICE_UNAVAILABLE, "数据库暂不可用")))?;
        let groups = sqlx::query_as::<_, NodeGroupListItem>(
            "SELECT id, name, enabled FROM node_groups WHERE name ILIKE $1 ORDER BY name LIMIT $2 OFFSET $3",
        )
        .bind(needle)
        .bind(page_size)
        .bind(offset)
        .fetch_all(&pg)
        .await
        .map_err(|_| response_error(unavailable(StatusCode::SERVICE_UNAVAILABLE, "数据库暂不可用")))?;
        (total, groups)
    } else {
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*)::bigint FROM node_groups")
            .fetch_one(&pg)
            .await
            .map_err(|_| response_error(unavailable(StatusCode::SERVICE_UNAVAILABLE, "数据库暂不可用")))?;
        let groups = sqlx::query_as::<_, NodeGroupListItem>(
            "SELECT id, name, enabled FROM node_groups ORDER BY name LIMIT $1 OFFSET $2",
        )
        .bind(page_size)
        .bind(offset)
        .fetch_all(&pg)
        .await
        .map_err(|_| response_error(unavailable(StatusCode::SERVICE_UNAVAILABLE, "数据库暂不可用")))?;
        (total, groups)
    };
    Ok(Json(crate::page::Page::new(groups, total, page, page_size)))
}

async fn create_node_group(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<CreateNodeGroup>,
) -> Result<(StatusCode, Json<NodeGroupListItem>), (StatusCode, Json<Value>)> {
    let (pg, actor) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    let name = normalize_node_group_name(&request.name).ok_or_else(|| {
        response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "节点组名称长度必须为 1 到 100 个字符",
        ))
    })?;
    let mut transaction = pg.begin().await.map_err(database_error)?;
    let group_id = resolve_or_create_node_group(&mut transaction, actor.user_id, &name).await?;
    let group = sqlx::query_as::<_, NodeGroupListItem>(
        "SELECT id, name, enabled FROM node_groups WHERE id = $1",
    )
    .bind(group_id)
    .fetch_one(&mut *transaction)
    .await
    .map_err(database_error)?;
    transaction.commit().await.map_err(database_error)?;
    Ok((StatusCode::CREATED, Json(group)))
}

async fn list_nodes(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(page): Query<crate::page::PageQuery>,
) -> Result<Json<crate::page::Page<AdminNodeListItem>>, (StatusCode, Json<Value>)> {
    let (pg, _) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    let (page, page_size, offset) = page.resolve();
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*)::bigint FROM nodes")
        .fetch_one(&pg)
        .await
        .map_err(|_| response_error(unavailable(StatusCode::SERVICE_UNAVAILABLE, "数据库暂不可用")))?;
    let nodes = sqlx::query_as::<_, AdminNodeListRow>(
        "SELECT n.id, n.name, n.region, n.public_host, n.enabled, n.online, n.last_seen_at::text AS last_seen_at, (SELECT COUNT(*)::bigint FROM tunnels t WHERE t.node_id = n.id AND t.status <> 'deleted') AS tunnel_count FROM nodes n ORDER BY n.name LIMIT $1 OFFSET $2",
    )
    .bind(page_size)
    .bind(offset)
    .fetch_all(&pg)
    .await
    .map_err(|_| response_error(unavailable(StatusCode::SERVICE_UNAVAILABLE, "数据库暂不可用")))?;
    let items = nodes
        .into_iter()
        .map(|node| AdminNodeListItem {
            host_metrics: host_metrics_for_admin(node.id, node.online),
            node,
        })
        .collect();
    Ok(Json(crate::page::Page::new(items, total, page, page_size)))
}

fn host_metrics_for_admin(node_id: Uuid, online: bool) -> Option<AdminHostMetrics> {
    if !online {
        return None;
    }
    crate::node_metrics::snapshot(node_id).map(|metrics| AdminHostMetrics {
        cpu_usage_percent: metrics.cpu_usage_percent,
        cpu_cores: metrics.cpu_cores,
        memory_used_bytes: metrics.memory_used_bytes,
        memory_total_bytes: metrics.memory_total_bytes,
        net_up_bps: metrics.net_up_bps,
        net_down_bps: metrics.net_down_bps,
        connections_tcp: metrics.connections_tcp,
        connections_udp: metrics.connections_udp,
    })
}

async fn get_admin_node(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(node_id): Path<Uuid>,
) -> Result<Json<AdminNodeDetail>, (StatusCode, Json<Value>)> {
    let (pg, _) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    let node = sqlx::query_as::<_, AdminNodeDetailRow>(
        "SELECT n.id, n.name, n.region, n.public_host, n.bind_addr, n.enabled, n.online, n.protocols, n.carrier_ports, n.tcp_port_ranges, n.udp_port_ranges, n.port_exclude, n.http_shared_port, n.https_shared_port FROM nodes n WHERE n.id = $1",
    )
    .bind(node_id)
    .fetch_optional(&pg)
    .await
    .map_err(database_error)?
    .ok_or_else(|| response_error(unavailable(StatusCode::NOT_FOUND, "节点不存在")))?;
    let node_group_names = fetch_node_group_names(&pg, node_id).await?;
    Ok(Json(AdminNodeDetail {
        node,
        node_group_names,
    }))
}

async fn set_admin_node_enabled(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(node_id): Path<Uuid>,
    Json(body): Json<SetNodeEnabled>,
) -> Result<StatusCode, (StatusCode, Json<Value>)> {
    let (pg, actor) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    let mut transaction = pg.begin().await.map_err(database_error)?;
    let updated = sqlx::query(
        "UPDATE nodes SET enabled = $2, config_revision = config_revision + 1, updated_at = now() WHERE id = $1",
    )
    .bind(node_id)
    .bind(body.enabled)
    .execute(&mut *transaction)
    .await
    .map_err(database_error)?;
    if updated.rows_affected() == 0 {
        return Err(response_error(unavailable(StatusCode::NOT_FOUND, "节点不存在")));
    }
    write_audit(
        &mut transaction,
        actor.user_id,
        if body.enabled {
            "node.enable"
        } else {
            "node.disable"
        },
        "node",
        node_id,
    )
    .await?;
    transaction.commit().await.map_err(database_error)?;
    if body.enabled {
        crate::ws::push_full_node_config(&pg, node_id).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_admin_node(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(node_id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, Json<Value>)> {
    let (pg, actor) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    let tunnel_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM tunnels WHERE node_id = $1 AND status <> 'deleted'",
    )
    .bind(node_id)
    .fetch_one(&pg)
    .await
    .map_err(database_error)?;
    if tunnel_count > 0 {
        return Err(response_error(unavailable(
            StatusCode::CONFLICT,
            "节点上仍有隧道，无法删除",
        )));
    }
    let mut transaction = pg.begin().await.map_err(database_error)?;
    let deleted = sqlx::query("DELETE FROM nodes WHERE id = $1")
        .bind(node_id)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
    if deleted.rows_affected() == 0 {
        return Err(response_error(unavailable(StatusCode::NOT_FOUND, "节点不存在")));
    }
    write_audit(
        &mut transaction,
        actor.user_id,
        "node.delete",
        "node",
        node_id,
    )
    .await?;
    transaction.commit().await.map_err(database_error)?;
    crate::node_metrics::clear(node_id);
    Ok(StatusCode::NO_CONTENT)
}

async fn update_admin_node(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(node_id): Path<Uuid>,
    Json(request): Json<CreateNode>,
) -> Result<Json<AdminNodeDetail>, (StatusCode, Json<Value>)> {
    let (pg, actor) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    let validated = validate_create_node_request(&request)?;
    let group_names = normalize_node_group_names(&request.node_group_names)?;
    let used = sqlx::query_as::<_, (String, String)>(
        "SELECT DISTINCT protocol, carrier FROM tunnels WHERE node_id = $1 AND status <> 'deleted'",
    )
    .bind(node_id)
    .fetch_all(&pg)
    .await
    .map_err(database_error)?;
    for (protocol, carrier) in &used {
        if !validated.protocols.iter().any(|item| item == protocol) {
            return Err(response_error(unavailable(
                StatusCode::CONFLICT,
                "节点上仍有使用该协议的隧道",
            )));
        }
        let enabled = validated
            .carrier_ports
            .get(carrier)
            .and_then(|entry| entry.get("enabled"))
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        if !enabled {
            return Err(response_error(unavailable(
                StatusCode::CONFLICT,
                "节点上仍有使用该 Carrier 的隧道",
            )));
        }
    }
    let mut transaction = pg.begin().await.map_err(database_error)?;
    let updated = sqlx::query(
        "UPDATE nodes SET name = $2, region = $3, public_host = $4, bind_addr = $5, protocols = $6, carrier_ports = $7, tcp_port_ranges = $8, udp_port_ranges = $9, port_exclude = $10, http_shared_port = $11, https_shared_port = $12, config_revision = config_revision + 1, updated_at = now() WHERE id = $1",
    )
    .bind(node_id)
    .bind(validated.name)
    .bind(&validated.region)
    .bind(validated.host)
    .bind(validated.bind_addr)
    .bind(&validated.protocols)
    .bind(validated.carrier_ports)
    .bind(validated.tcp_port_ranges)
    .bind(validated.udp_port_ranges)
    .bind(validated.port_exclude)
    .bind(i32::from(validated.http_port))
    .bind(i32::from(validated.https_port))
    .execute(&mut *transaction)
    .await
    .map_err(|_| {
        response_error(unavailable(
            StatusCode::CONFLICT,
            "节点名称已存在或数据库不可用",
        ))
    })?;
    if updated.rows_affected() == 0 {
        return Err(response_error(unavailable(StatusCode::NOT_FOUND, "节点不存在")));
    }
    sync_node_group_members(&mut transaction, actor.user_id, node_id, &group_names).await?;
    write_audit(
        &mut transaction,
        actor.user_id,
        "node.update",
        "node",
        node_id,
    )
    .await?;
    transaction.commit().await.map_err(database_error)?;

    if let Some(redis) = state.redis_connection() {
        let port_cache = RedisPortCache::new(redis);
        let _ = port_cache
            .rebuild_node(
                node_id,
                &validated.tcp_ranges,
                &validated.udp_ranges,
                &validated.exclusions,
                &validated.tcp_reserved,
                &validated.udp_reserved,
                &[],
                &[],
            )
            .await;
    }
    crate::ws::push_full_node_config(&pg, node_id).await;

    get_admin_node(State(state), headers, Path(node_id)).await
}

struct ValidatedCreateNode<'a> {
    name: &'a str,
    host: &'a str,
    region: String,
    bind_addr: String,
    protocols: Vec<String>,
    tcp_ranges: PortRanges,
    udp_ranges: PortRanges,
    exclusions: PortRanges,
    http_port: u16,
    https_port: u16,
    carrier_ports: Value,
    tcp_port_ranges: Value,
    udp_port_ranges: Value,
    port_exclude: Value,
    tcp_reserved: Vec<u16>,
    udp_reserved: Vec<u16>,
}

fn validate_create_node_request(
    request: &CreateNode,
) -> Result<ValidatedCreateNode<'_>, (StatusCode, Json<Value>)> {
    let name = request.name.trim();
    let host = request.public_host.trim();
    if name.is_empty() || name.len() > 100 || host.is_empty() || host.len() > 253 {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "节点名称或公网地址无效",
        )));
    }
    let region = parse_iso3166_region(request.region.as_deref())?;
    let protocols = normalize_registered_names(
        &request.protocols,
        &tz_ingress::registered_kinds(),
        "请至少启用一种协议",
        "节点协议不受支持",
    )?;
    let carriers = normalize_node_carriers(&request.carriers)?;
    let tcp_ranges = parse_ranges(&request.tcp_port_ranges)?;
    let udp_ranges = parse_ranges(&request.udp_port_ranges)?;
    let exclusions = match request
        .port_exclude
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(value) => PortRanges::from_str(value).map_err(|_| {
            response_error(unavailable(StatusCode::BAD_REQUEST, "端口排除区间格式无效"))
        })?,
        None => PortRanges::empty(),
    };
    let http_port = request.http_shared_port.unwrap_or(80);
    let https_port = request.https_shared_port.unwrap_or(443);
    let http_enabled = protocols.iter().any(|protocol| protocol == "http");
    let mut tcp_reserved: Vec<u16> = carriers
        .iter()
        .filter(|carrier| carrier.enabled && carrier.socket == "tcp")
        .map(|carrier| carrier.port)
        .collect();
    let udp_reserved: Vec<u16> = carriers
        .iter()
        .filter(|carrier| carrier.enabled && carrier.socket == "udp")
        .map(|carrier| carrier.port)
        .collect();
    if http_enabled {
        tcp_reserved.push(http_port);
        tcp_reserved.push(https_port);
    }
    if http_enabled && http_port == https_port
        || tcp_reserved.iter().any(|port| tcp_ranges.contains(*port))
        || udp_reserved.iter().any(|port| udp_ranges.contains(*port))
        || has_duplicate(&tcp_reserved)
        || has_duplicate(&udp_reserved)
    {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "Carrier、共享 HTTP(S) 端口不能与隧道公网端口区间重叠",
        )));
    }
    if tcp_ranges.excludes(&exclusions).port_count() == 0
        || udp_ranges.excludes(&exclusions).port_count() == 0
    {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "排除端口后没有可用端口",
        )));
    }
    let mut carrier_ports = serde_json::Map::new();
    for carrier in &carriers {
        carrier_ports.insert(
            carrier.kind.clone(),
            json!({ "port": carrier.port, "enabled": carrier.enabled }),
        );
    }
    let carrier_ports = Value::Object(carrier_ports);
    let tcp_port_ranges = ranges_json(&tcp_ranges);
    let udp_port_ranges = ranges_json(&udp_ranges);
    let port_exclude = ranges_json(&exclusions);
    Ok(ValidatedCreateNode {
        name,
        host,
        region,
        bind_addr: request
            .bind_addr
            .clone()
            .unwrap_or_else(|| "0.0.0.0".into()),
        protocols,
        tcp_ranges,
        udp_ranges,
        exclusions,
        http_port,
        https_port,
        carrier_ports,
        tcp_port_ranges,
        udp_port_ranges,
        port_exclude,
        tcp_reserved,
        udp_reserved,
    })
}

struct NormalizedCarrier {
    kind: String,
    socket: &'static str,
    port: u16,
    enabled: bool,
}

fn normalize_registered_names(
    values: &[String],
    registered: &[&str],
    empty_message: &'static str,
    unknown_message: &'static str,
) -> Result<Vec<String>, (StatusCode, Json<Value>)> {
    let mut names = Vec::new();
    for value in values {
        let name = value.trim().to_ascii_lowercase();
        if name.is_empty() || !registered.contains(&name.as_str()) {
            return Err(response_error(unavailable(StatusCode::BAD_REQUEST, unknown_message)));
        }
        if names.iter().any(|existing: &String| existing == &name) {
            return Err(response_error(unavailable(StatusCode::BAD_REQUEST, unknown_message)));
        }
        names.push(name);
    }
    if names.is_empty() {
        return Err(response_error(unavailable(StatusCode::BAD_REQUEST, empty_message)));
    }
    names.sort();
    Ok(names)
}

fn normalize_node_carriers(
    carriers: &[NodeCarrierSetting],
) -> Result<Vec<NormalizedCarrier>, (StatusCode, Json<Value>)> {
    let registered = tz_carrier::registered_carriers();
    if carriers.is_empty() {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "请至少启用一种 Carrier",
        )));
    }
    let mut normalized = Vec::new();
    for carrier in carriers {
        let kind = carrier.kind.trim().to_ascii_lowercase();
        let Some(desc) = registered.iter().find(|item| item.kind == kind) else {
            return Err(response_error(unavailable(
                StatusCode::BAD_REQUEST,
                "节点 Carrier 不受支持",
            )));
        };
        if normalized.iter().any(|item: &NormalizedCarrier| item.kind == kind) {
            return Err(response_error(unavailable(
                StatusCode::BAD_REQUEST,
                "节点 Carrier 不能重复",
            )));
        }
        if carrier.enabled && carrier.port == 0 {
            return Err(response_error(unavailable(
                StatusCode::BAD_REQUEST,
                "Carrier 监听端口不能为 0",
            )));
        }
        normalized.push(NormalizedCarrier {
            kind,
            socket: desc.socket,
            port: if carrier.port == 0 { 7000 } else { carrier.port },
            enabled: carrier.enabled,
        });
    }
    if !normalized.iter().any(|carrier| carrier.enabled) {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "请至少启用一种 Carrier",
        )));
    }
    Ok(normalized)
}

fn has_duplicate(ports: &[u16]) -> bool {
    ports.iter().enumerate().any(|(index, port)| ports[..index].contains(port))
}

async fn create_node(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<CreateNode>,
) -> Result<(StatusCode, Json<NodeCreated>), (StatusCode, Json<Value>)> {
    let (pg, actor) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    let validated = validate_create_node_request(&request)?;
    let group_names = normalize_node_group_names(&request.node_group_names)?;
    let (token, token_hash, token_prefix) = new_agent_token("无法生成节点凭据")?;
    let id = Uuid::new_v4();
    let mut transaction = pg.begin().await.map_err(database_error)?;
    sqlx::query(
        "INSERT INTO nodes (id, name, region, public_host, bind_addr, token_hash, token_prefix, protocols, carrier_ports, tcp_port_ranges, udp_port_ranges, port_exclude, http_shared_port, https_shared_port, token) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)",
    )
    .bind(id)
    .bind(validated.name)
    .bind(&validated.region)
    .bind(validated.host)
    .bind(validated.bind_addr)
    .bind(token_hash)
    .bind(&token_prefix)
    .bind(&validated.protocols)
    .bind(validated.carrier_ports)
    .bind(validated.tcp_port_ranges)
    .bind(validated.udp_port_ranges)
    .bind(validated.port_exclude)
    .bind(i32::from(validated.http_port))
    .bind(i32::from(validated.https_port))
    .bind(&token)
    .execute(&mut *transaction)
    .await
    .map_err(|_| response_error(unavailable(StatusCode::CONFLICT, "节点名称已存在或数据库不可用")))?;
    sync_node_group_members(&mut transaction, actor.user_id, id, &group_names).await?;
    write_audit(&mut transaction, actor.user_id, "node.create", "node", id).await?;
    let outbox_id: i64 = sqlx::query_scalar(
        "INSERT INTO redis_outbox (operation, payload) VALUES ('rebuild_node_ports', $1) RETURNING id",
    )
    .bind(json!({ "node_id": id }))
    .fetch_one(&mut *transaction)
    .await
    .map_err(database_error)?;
    transaction.commit().await.map_err(database_error)?;

    if let Some(redis) = state.redis_connection() {
        let port_cache = RedisPortCache::new(redis);
        if port_cache
            .rebuild_node(
                id,
                &validated.tcp_ranges,
                &validated.udp_ranges,
                &validated.exclusions,
                &validated.tcp_reserved,
                &validated.udp_reserved,
                &[],
                &[],
            )
            .await
            .is_ok()
        {
            let _: Result<sqlx::postgres::PgQueryResult, _> =
                sqlx::query("DELETE FROM redis_outbox WHERE id = $1")
                    .bind(outbox_id)
                    .execute(&pg)
                    .await;
        } else {
            tracing::warn!(node_id = %id, "Redis node port-map rebuild deferred");
        }
    }
    Ok((
        StatusCode::CREATED,
        Json(NodeCreated {
            id,
            token,
            token_prefix,
        }),
    ))
}

fn normalize_node_group_name(raw: &str) -> Option<String> {
    let name = raw.trim().split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() || name.len() > 100 {
        return None;
    }
    Some(name)
}

fn normalize_node_group_names(
    names: &[String],
) -> Result<Vec<String>, (StatusCode, Json<Value>)> {
    let mut normalized = Vec::new();
    for raw in names {
        let Some(name) = normalize_node_group_name(raw) else {
            continue;
        };
        if normalized
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(&name))
        {
            continue;
        }
        normalized.push(name);
    }
    if normalized.is_empty() {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "请至少添加一个节点组",
        )));
    }
    Ok(normalized)
}

async fn fetch_node_group_names(
    pg: &PgPool,
    node_id: Uuid,
) -> Result<Vec<String>, (StatusCode, Json<Value>)> {
    let names = sqlx::query_scalar::<_, String>(
        "SELECT ng.name FROM node_group_members ngm JOIN node_groups ng ON ng.id = ngm.node_group_id WHERE ngm.node_id = $1 ORDER BY ng.name",
    )
    .bind(node_id)
    .fetch_all(pg)
    .await
    .map_err(database_error)?;
    Ok(names)
}

async fn resolve_or_create_node_group(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor_id: Uuid,
    name: &str,
) -> Result<Uuid, (StatusCode, Json<Value>)> {
    let existing: Option<(Uuid, bool)> = sqlx::query_as(
        "SELECT id, enabled FROM node_groups WHERE name = $1",
    )
    .bind(name)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    if let Some((id, enabled)) = existing {
        if !enabled {
            return Err(response_error(unavailable(
                StatusCode::BAD_REQUEST,
                "所选节点组已停用",
            )));
        }
        return Ok(id);
    }
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO node_groups (id, name) VALUES ($1, $2)",
    )
    .bind(id)
    .bind(name)
    .execute(&mut **transaction)
    .await
    .map_err(|_| {
        response_error(unavailable(
            StatusCode::CONFLICT,
            "节点组名称已存在或数据库不可用",
        ))
    })?;
    write_audit(
        transaction,
        actor_id,
        "node_group.create",
        "node_group",
        id,
    )
    .await?;
    Ok(id)
}

async fn sync_node_group_members(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor_id: Uuid,
    node_id: Uuid,
    names: &[String],
) -> Result<(), (StatusCode, Json<Value>)> {
    sqlx::query("DELETE FROM node_group_members WHERE node_id = $1")
        .bind(node_id)
        .execute(&mut **transaction)
        .await
        .map_err(database_error)?;
    for name in names {
        let group_id = resolve_or_create_node_group(transaction, actor_id, name).await?;
        sqlx::query(
            "INSERT INTO node_group_members (node_group_id, node_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        )
        .bind(group_id)
        .bind(node_id)
        .execute(&mut **transaction)
        .await
        .map_err(database_error)?;
    }
    Ok(())
}

#[allow(dead_code)]
fn certificate_covers_suffix(certificates: &[String], suffix: &str) -> bool {
    certificates.iter().any(|domain| {
        domain
            .trim()
            .to_ascii_lowercase()
            .strip_prefix("*.")
            .is_some_and(|wildcard| wildcard == suffix)
    })
}

#[allow(dead_code)]
fn valid_domain_suffix(suffix: &str) -> bool {
    if suffix.is_empty() || suffix.len() > 253 || IpAddr::from_str(suffix).is_ok() {
        return false;
    }
    suffix.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
            && label
                .as_bytes()
                .last()
                .is_some_and(u8::is_ascii_alphanumeric)
            && label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    })
}

fn parse_ranges(input: &str) -> Result<PortRanges, (StatusCode, Json<Value>)> {
    PortRanges::from_str(input)
        .map_err(|_| response_error(unavailable(StatusCode::BAD_REQUEST, "端口区间格式无效")))
}

fn ranges_json(ranges: &PortRanges) -> Value {
    Value::Array(
        ranges
            .ranges()
            .iter()
            .map(|range| json!([range.start, range.end]))
            .collect(),
    )
}

pub(super) async fn write_audit(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor: Uuid,
    action: &str,
    target_type: &str,
    target_id: Uuid,
) -> Result<(), (StatusCode, Json<Value>)> {
    write_audit_with_ip(transaction, actor, action, target_type, target_id, None).await
}

pub(super) async fn write_audit_with_ip(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor: Uuid,
    action: &str,
    target_type: &str,
    target_id: Uuid,
    remote_ip: Option<std::net::IpAddr>,
) -> Result<(), (StatusCode, Json<Value>)> {
    sqlx::query(
        "INSERT INTO audit_logs (actor_id, action, target_type, target_id, remote_ip) \
         VALUES ($1, $2, $3, $4, $5::inet)",
    )
    .bind(actor)
    .bind(action)
    .bind(target_type)
    .bind(target_id.to_string())
    .bind(remote_ip.map(|ip| ip.to_string()))
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(())
}

pub(super) fn database_error(_: sqlx::Error) -> (StatusCode, Json<Value>) {
    response_error(unavailable(
        StatusCode::SERVICE_UNAVAILABLE,
        "数据库暂不可用",
    ))
}

fn parse_iso3166_region(region: Option<&str>) -> Result<String, (StatusCode, Json<Value>)> {
    let value = region.unwrap_or("").trim();
    if value.is_empty() {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "请选择节点地区",
        )));
    }
    if value.len() != 2 || !value.chars().all(|ch| ch.is_ascii_alphabetic()) {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "地区必须为 ISO 3166-1 两字代码",
        )));
    }
    Ok(value.to_ascii_uppercase())
}

pub(super) fn config_data_error(_: anyhow::Error) -> (StatusCode, Json<Value>) {
    response_error(unavailable(
        StatusCode::INTERNAL_SERVER_ERROR,
        "节点端口配置无效",
    ))
}

pub(super) fn response_error(response: axum::response::Response) -> (StatusCode, Json<Value>) {
    let status = response.status();
    let message = response
        .extensions()
        .get::<ApiErrorContext>()
        .map(|context| context.0.clone())
        .unwrap_or_else(|| "请求无法完成".into());
    (status, Json(json!({ "error": message })))
}
