use super::{database_error, response_error};
use crate::{
    port_cache::{PortCacheError, PortProtocol, RedisPortCache, reservation_token},
    setup::{AppState, UserRole, unavailable},
    store,
};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::FromRow;
use std::{collections::HashSet, sync::Arc};
use tz_common::ports::PortRanges;
use uuid::Uuid;

pub fn user_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/nodes", get(list_available_nodes))
        .route("/api/v1/nodes/{node_id}/ports/check", get(check_port))
        .route("/api/v1/nodes/{node_id}/ports/random", post(random_port))
}

#[derive(Debug, Serialize, FromRow)]
struct NodeChoice {
    id: Uuid,
    name: String,
    region: String,
    public_host: String,
    capabilities: Value,
    protocols: Vec<String>,
    carrier_ports: Value,
    tcp_port_ranges: Value,
    udp_port_ranges: Value,
    port_exclude: Value,
    http_shared_port: i32,
    https_shared_port: i32,
}

#[derive(Deserialize)]
struct PortCheckQuery {
    l4: String,
    port: u16,
}

#[derive(Deserialize)]
struct PortRandomRequest {
    l4: String,
}

#[derive(Serialize)]
struct PortCheckResult {
    available: bool,
    reason: Option<&'static str>,
}

#[derive(Serialize)]
struct PortRandomResult {
    port: u16,
}

async fn list_available_nodes(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(page): Query<crate::page::PageQuery>,
) -> Result<Json<crate::page::Page<NodeChoice>>, (StatusCode, Json<Value>)> {
    let (pg, user) = state
        .database_for(&headers, UserRole::User)
        .await
        .map_err(response_error)?;
    let (page, page_size, offset) = page.resolve();
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM (SELECT DISTINCT n.id FROM nodes n JOIN node_group_members ngm ON ngm.node_id = n.id JOIN subscription_node_groups sng ON sng.node_group_id = ngm.node_group_id JOIN user_subscriptions s ON s.id = sng.subscription_id WHERE s.user_id = $1 AND s.status = 'active' AND s.starts_at <= now() AND (s.expires_at IS NULL OR s.expires_at > now()) AND s.exhausted_period_start IS NULL AND n.enabled = TRUE AND n.online = TRUE) nodes",
    )
    .bind(user.user_id)
    .fetch_one(&pg)
    .await
    .map_err(database_error)?;
    let nodes = sqlx::query_as::<_, NodeChoice>(
        "SELECT DISTINCT n.id, n.name, n.region, n.public_host, n.capabilities, n.protocols, n.carrier_ports, n.tcp_port_ranges, n.udp_port_ranges, n.port_exclude, n.http_shared_port, n.https_shared_port FROM nodes n JOIN node_group_members ngm ON ngm.node_id = n.id JOIN subscription_node_groups sng ON sng.node_group_id = ngm.node_group_id JOIN user_subscriptions s ON s.id = sng.subscription_id WHERE s.user_id = $1 AND s.status = 'active' AND s.starts_at <= now() AND (s.expires_at IS NULL OR s.expires_at > now()) AND s.exhausted_period_start IS NULL AND n.enabled = TRUE AND n.online = TRUE ORDER BY n.name LIMIT $2 OFFSET $3",
    )
    .bind(user.user_id)
    .bind(page_size)
    .bind(offset)
    .fetch_all(&pg)
    .await
    .map_err(database_error)?;
    Ok(Json(crate::page::Page::new(nodes, total, page, page_size)))
}

async fn check_port(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(node_id): Path<Uuid>,
    Query(query): Query<PortCheckQuery>,
) -> Result<Json<PortCheckResult>, (StatusCode, Json<Value>)> {
    let (pg, user) = state
        .database_for(&headers, UserRole::User)
        .await
        .map_err(response_error)?;
    let allow_custom: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM user_subscriptions WHERE user_id = $1 AND status = 'active' AND starts_at <= now() AND (expires_at IS NULL OR expires_at > now()) AND exhausted_period_start IS NULL AND allow_custom_port = TRUE)",
    )
    .bind(user.user_id)
    .fetch_one(&pg)
    .await
    .map_err(|_| response_error(unavailable(StatusCode::SERVICE_UNAVAILABLE, "数据库暂不可用")))?;
    if !allow_custom {
        return Err(response_error(unavailable(
            StatusCode::FORBIDDEN,
            "当前订阅不允许自定义公网端口",
        )));
    }
    let port_type = match query.l4.as_str() {
        "tcp" => PortProtocol::Tcp,
        "udp" => PortProtocol::Udp,
        _ => {
            return Err(response_error(unavailable(
                StatusCode::BAD_REQUEST,
                "端口协议必须是 tcp 或 udp",
            )));
        }
    };
    let node = sqlx::query_as::<_, NodeChoice>(
        "SELECT n.id, n.name, n.region, n.public_host, n.capabilities, n.protocols, n.carrier_ports, n.tcp_port_ranges, n.udp_port_ranges, n.port_exclude, n.http_shared_port, n.https_shared_port FROM nodes n WHERE n.id = $1 AND n.enabled = TRUE AND n.online = TRUE AND EXISTS (SELECT 1 FROM node_group_members ngm JOIN subscription_node_groups sng ON sng.node_group_id = ngm.node_group_id JOIN user_subscriptions s ON s.id = sng.subscription_id WHERE ngm.node_id = n.id AND s.id = sng.subscription_id AND s.user_id = $2 AND s.status = 'active' AND s.starts_at <= now() AND (s.expires_at IS NULL OR s.expires_at > now()) AND s.exhausted_period_start IS NULL)",
    )
    .bind(node_id)
    .bind(user.user_id)
    .fetch_optional(&pg)
    .await
    .map_err(|_| response_error(unavailable(StatusCode::SERVICE_UNAVAILABLE, "数据库暂不可用")))?
    .ok_or_else(|| response_error(unavailable(StatusCode::NOT_FOUND, "节点不存在或当前订阅不可用")))?;
    let (ranges, protocol) = if port_type == PortProtocol::Tcp {
        (node.tcp_port_ranges, PortProtocol::Tcp)
    } else {
        (node.udp_port_ranges, PortProtocol::Udp)
    };
    let ranges = store::decode_port_ranges(ranges).map_err(|_| {
        response_error(unavailable(
            StatusCode::INTERNAL_SERVER_ERROR,
            "节点端口配置无效",
        ))
    })?;
    let exclusions = store::decode_port_ranges(node.port_exclude.clone()).map_err(|_| {
        response_error(unavailable(
            StatusCode::INTERNAL_SERVER_ERROR,
            "节点端口配置无效",
        ))
    })?;
    let mut fixed = if protocol == PortProtocol::Udp {
        crate::store::reserved_ports_for_socket(&node.carrier_ports, "udp")
            .into_iter()
            .map(Some)
            .collect::<Vec<_>>()
    } else {
        let mut ports = crate::store::reserved_ports_for_socket(&node.carrier_ports, "tcp")
            .into_iter()
            .map(Some)
            .collect::<Vec<_>>();
        if node.protocols.iter().any(|item| item == "http") {
            ports.push(u16::try_from(node.http_shared_port).ok());
            ports.push(u16::try_from(node.https_shared_port).ok());
        }
        ports
    };
    let fixed_ranges = PortRanges::new(
        fixed
            .drain(..)
            .flatten()
            .map(|port| tz_common::ports::PortRange {
                start: port,
                end: port,
            })
            .collect(),
    )
    .unwrap_or_else(|_| PortRanges::empty());
    if !ranges.contains(query.port)
        || exclusions.contains(query.port)
        || fixed_ranges.contains(query.port)
    {
        return Ok(Json(PortCheckResult {
            available: false,
            reason: Some("outside_or_reserved"),
        }));
    }
    if let Some(redis) = state.redis_connection() {
        match RedisPortCache::new(redis)
            .is_available(node_id, protocol, query.port)
            .await
        {
            Ok(Some(true)) => {
                return Ok(Json(PortCheckResult {
                    available: true,
                    reason: None,
                }));
            }
            Ok(Some(false)) => {
                return Ok(Json(PortCheckResult {
                    available: false,
                    reason: Some("occupied"),
                }));
            }
            Ok(None) | Err(_) => {}
        }
    }
    let occupied: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM tunnels WHERE node_id = $1 AND remote_port = $2 AND status <> 'deleted' AND CASE WHEN protocol = 'udp' THEN 'udp' ELSE 'tcp' END = $3)",
    )
    .bind(node_id)
    .bind(i32::from(query.port))
    .bind(protocol.name())
    .fetch_one(&pg)
    .await
    .map_err(|_| response_error(unavailable(StatusCode::SERVICE_UNAVAILABLE, "数据库暂不可用")))?;
    Ok(Json(PortCheckResult {
        available: !occupied,
        reason: occupied.then_some("occupied"),
    }))
}

async fn random_port(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(node_id): Path<Uuid>,
    Json(request): Json<PortRandomRequest>,
) -> Result<Json<PortRandomResult>, (StatusCode, Json<Value>)> {
    let (pg, user) = state
        .database_for(&headers, UserRole::User)
        .await
        .map_err(response_error)?;
    let allow_custom: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM user_subscriptions WHERE user_id = $1 AND status = 'active' AND starts_at <= now() AND (expires_at IS NULL OR expires_at > now()) AND exhausted_period_start IS NULL AND allow_custom_port = TRUE)",
    )
    .bind(user.user_id)
    .fetch_one(&pg)
    .await
    .map_err(database_error)?;
    if !allow_custom {
        return Err(response_error(unavailable(
            StatusCode::FORBIDDEN,
            "当前订阅不允许自定义公网端口",
        )));
    }
    let protocol = match request.l4.as_str() {
        "tcp" => PortProtocol::Tcp,
        "udp" => PortProtocol::Udp,
        _ => {
            return Err(response_error(unavailable(
                StatusCode::BAD_REQUEST,
                "端口协议必须是 tcp 或 udp",
            )));
        }
    };
    let node = sqlx::query_as::<_, NodeChoice>(
        "SELECT n.id, n.name, n.region, n.public_host, n.capabilities, n.protocols, n.carrier_ports, n.tcp_port_ranges, n.udp_port_ranges, n.port_exclude, n.http_shared_port, n.https_shared_port FROM nodes n WHERE n.id = $1 AND n.enabled = TRUE AND n.online = TRUE AND EXISTS (SELECT 1 FROM node_group_members ngm JOIN subscription_node_groups sng ON sng.node_group_id = ngm.node_group_id JOIN user_subscriptions s ON s.id = sng.subscription_id WHERE ngm.node_id = n.id AND s.user_id = $2 AND s.status = 'active' AND s.starts_at <= now() AND (s.expires_at IS NULL OR s.expires_at > now()) AND s.exhausted_period_start IS NULL)",
    )
    .bind(node_id)
    .bind(user.user_id)
    .fetch_optional(&pg)
    .await
    .map_err(database_error)?
    .ok_or_else(|| response_error(unavailable(StatusCode::NOT_FOUND, "节点不存在或当前订阅不可用")))?;
    let range_json = if protocol == PortProtocol::Udp {
        node.udp_port_ranges
    } else {
        node.tcp_port_ranges
    };
    let ranges = store::decode_port_ranges(range_json).map_err(super::config_data_error)?;
    let exclusions = store::decode_port_ranges(node.port_exclude).map_err(super::config_data_error)?;
    let mut reserved = if protocol == PortProtocol::Udp {
        crate::store::reserved_ports_for_socket(&node.carrier_ports, "udp")
            .into_iter()
            .map(Some)
            .collect::<Vec<_>>()
    } else {
        let mut ports = crate::store::reserved_ports_for_socket(&node.carrier_ports, "tcp")
            .into_iter()
            .map(Some)
            .collect::<Vec<_>>();
        if node.protocols.iter().any(|item| item == "http") {
            ports.push(u16::try_from(node.http_shared_port).ok());
            ports.push(u16::try_from(node.https_shared_port).ok());
        }
        ports
    };
    let fixed = PortRanges::new(
        reserved
            .drain(..)
            .flatten()
            .map(|port| tz_common::ports::PortRange {
                start: port,
                end: port,
            })
            .collect(),
    )
    .unwrap_or_else(|_| PortRanges::empty());
    let available = ranges.excludes(&exclusions).excludes(&fixed);
    let mut selected = None;
    if let Some(redis) = state.redis_connection() {
        let cache = RedisPortCache::new(redis);
        let reservation = reservation_token();
        match cache
            .reserve_random(node_id, protocol, &available, reservation)
            .await
        {
            Ok(Some(port)) => {
                if !matches!(
                    cache
                        .rollback_reservation(node_id, protocol, port, reservation)
                        .await,
                    Ok(true)
                ) {
                    if cache.invalidate(node_id, protocol).await.is_err() {
                        state.disable_redis();
                    }
                    queue_port_rebuild(&pg, node_id).await;
                }
                selected = Some(port);
            }
            Ok(None) => {
                return Err(response_error(unavailable(
                    StatusCode::CONFLICT,
                    "所选节点端口区间已用尽",
                )));
            }
            Err(PortCacheError::NotReady) => {}
            Err(PortCacheError::Redis(_)) => {
                if cache.invalidate(node_id, protocol).await.is_err() {
                    state.disable_redis();
                }
                queue_port_rebuild(&pg, node_id).await;
            }
            Err(PortCacheError::Randomness(_)) => {
                return Err(response_error(unavailable(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "无法随机选择公网端口",
                )));
            }
        }
    }
    if selected.is_none() {
        let query = if protocol == PortProtocol::Udp {
            "SELECT remote_port FROM tunnels WHERE node_id = $1 AND protocol = 'udp' AND remote_port IS NOT NULL AND status <> 'deleted'"
        } else {
            "SELECT remote_port FROM tunnels WHERE node_id = $1 AND protocol <> 'udp' AND remote_port IS NOT NULL AND status <> 'deleted'"
        };
        let occupied = sqlx::query_scalar::<_, i32>(query)
            .bind(node_id)
            .fetch_all(&pg)
            .await
            .map_err(database_error)?
            .into_iter()
            .filter_map(|port| u16::try_from(port).ok())
            .collect::<HashSet<_>>();
        selected = choose_port(&available, &occupied);
    }
    let port = selected.ok_or_else(|| {
        response_error(unavailable(StatusCode::CONFLICT, "所选节点端口区间已用尽"))
    })?;
    Ok(Json(PortRandomResult { port }))
}

async fn queue_port_rebuild(pg: &sqlx::PgPool, node_id: Uuid) {
    let _ = sqlx::query(
        "INSERT INTO redis_outbox (operation, payload) VALUES ('rebuild_node_ports', $1)",
    )
    .bind(json!({ "node_id": node_id }))
    .execute(pg)
    .await;
}

fn choose_port(ranges: &PortRanges, occupied: &HashSet<u16>) -> Option<u16> {
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
                let port = u16::try_from(u32::from(range.start) + ordinal).ok()?;
                if !occupied.contains(&port) {
                    return Some(port);
                }
                break;
            }
            ordinal -= length;
        }
    }
    None
}

