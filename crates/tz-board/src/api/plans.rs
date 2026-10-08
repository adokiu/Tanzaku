use super::{database_error, response_error, write_audit};
use crate::setup::{AppState, UserRole, unavailable};
use crate::page::{Page, PageQuery};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get, patch, post, put},
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::sync::Arc;
use uuid::Uuid;

pub fn admin_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/admin/plans", get(list_plans).post(create_plan))
        .route(
            "/api/v1/admin/plans/{plan_id}",
            put(update_plan).delete(delete_plan),
        )
        .route(
            "/api/v1/admin/plans/{plan_id}/enabled",
            patch(set_plan_enabled),
        )
}

#[derive(Debug, Serialize, FromRow)]
struct PlanRow {
    id: Uuid,
    name: String,
    description: String,
    speed_limit_mbps: i64,
    max_conns_per_tunnel: i32,
    max_new_conns_per_sec: i32,
    max_tunnels: i32,
    allow_custom_port: bool,
    traffic_quota_bytes: Option<i64>,
    traffic_period: String,
    duration_days: Option<i32>,
    allowed_protocols: Vec<String>,
    traffic_count_mode: String,
    enabled: bool,
}

#[derive(Debug, Serialize, FromRow)]
struct PlanListRow {
    id: Uuid,
    name: String,
    description: String,
    speed_limit_mbps: i64,
    max_conns_per_tunnel: i32,
    max_new_conns_per_sec: i32,
    max_tunnels: i32,
    allow_custom_port: bool,
    traffic_quota_bytes: Option<i64>,
    traffic_period: String,
    duration_days: Option<i32>,
    allowed_protocols: Vec<String>,
    traffic_count_mode: String,
    enabled: bool,
    node_group_ids: Vec<Uuid>,
    node_group_names: Vec<String>,
}

const PLAN_LIST_SQL: &str = "SELECT p.id, p.name, p.description, p.speed_limit_mbps, p.max_conns_per_tunnel, p.max_new_conns_per_sec, p.max_tunnels, p.allow_custom_port, p.traffic_quota_bytes, p.traffic_period, p.duration_days, p.allowed_protocols, p.traffic_count_mode, p.enabled, COALESCE(array_agg(png.node_group_id ORDER BY ng.name) FILTER (WHERE png.node_group_id IS NOT NULL), '{}') AS node_group_ids, COALESCE(array_agg(ng.name ORDER BY ng.name) FILTER (WHERE ng.name IS NOT NULL), '{}') AS node_group_names FROM plans p LEFT JOIN plan_node_groups png ON png.plan_id = p.id LEFT JOIN node_groups ng ON ng.id = png.node_group_id GROUP BY p.id ORDER BY p.name";

const PLAN_RETURNING: &str = "RETURNING id, name, description, speed_limit_mbps, max_conns_per_tunnel, max_new_conns_per_sec, max_tunnels, allow_custom_port, traffic_quota_bytes, traffic_period, duration_days, allowed_protocols, traffic_count_mode, enabled";

#[derive(Deserialize)]
struct PlanBody {
    name: String,
    description: Option<String>,
    speed_limit_mbps: i64,
    max_conns_per_tunnel: i32,
    max_new_conns_per_sec: i32,
    max_tunnels: i32,
    allow_custom_port: bool,
    traffic_quota_bytes: Option<i64>,
    traffic_period: String,
    allowed_protocols: Vec<String>,
    traffic_count_mode: Option<String>,
    node_group_ids: Vec<Uuid>,
}

#[derive(Deserialize)]
struct SetPlanEnabled {
    enabled: bool,
}

fn validate_plan_body(request: &PlanBody) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    let name = request.name.trim();
    if name.is_empty() || name.len() > 100 {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "套餐名称长度必须为 1 到 100 个字符",
        )));
    }
    if request.speed_limit_mbps <= 0
        || request.max_conns_per_tunnel <= 0
        || request.max_new_conns_per_sec <= 0
        || request.max_tunnels <= 0
    {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "套餐限额必须大于 0",
        )));
    }
    if request.traffic_quota_bytes.is_some_and(|quota| quota <= 0)
        || !matches!(
            request.traffic_period.as_str(),
            "day" | "week" | "month" | "quarter" | "year" | "lifetime"
        )
        || request.allowed_protocols.is_empty()
        || request
            .allowed_protocols
            .iter()
            .any(|protocol| !tz_ingress::registered_kinds().contains(&protocol.as_str()))
        || request.node_group_ids.is_empty()
    {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "套餐节点组、协议或周期配置无效",
        )));
    }
    let count_mode = request.traffic_count_mode.as_deref().unwrap_or("sum");
    if !matches!(
        count_mode,
        "sum" | "inbound" | "outbound" | "max"
    ) {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "流量统计方式无效",
        )));
    }
    Ok(())
}

async fn ensure_node_groups(
    pg: &sqlx::PgPool,
    node_group_ids: &[Uuid],
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    let group_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM node_groups WHERE enabled = TRUE AND id = ANY($1)",
    )
    .bind(node_group_ids)
    .fetch_one(pg)
    .await
    .map_err(database_error)?;
    if group_count != node_group_ids.len() as i64 {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "套餐包含不存在或已停用的节点组",
        )));
    }
    Ok(())
}

async fn replace_plan_node_groups(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    plan_id: Uuid,
    node_group_ids: &[Uuid],
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    sqlx::query("DELETE FROM plan_node_groups WHERE plan_id = $1")
        .bind(plan_id)
        .execute(&mut **transaction)
        .await
        .map_err(database_error)?;
    for group_id in node_group_ids {
        sqlx::query("INSERT INTO plan_node_groups (plan_id, node_group_id) VALUES ($1, $2)")
            .bind(plan_id)
            .bind(group_id)
            .execute(&mut **transaction)
            .await
            .map_err(database_error)?;
    }
    Ok(())
}

async fn list_plans(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(page): Query<PageQuery>,
) -> Result<Json<Page<PlanListRow>>, (StatusCode, Json<serde_json::Value>)> {
    let (pg, _) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    let (page, page_size, offset) = page.resolve();
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*)::bigint FROM plans")
        .fetch_one(&pg)
        .await
        .map_err(database_error)?;
    let sql = [PLAN_LIST_SQL, " LIMIT $1 OFFSET $2"].concat();
    let plans = sqlx::query_as::<_, PlanListRow>(&sql)
        .bind(page_size)
        .bind(offset)
        .fetch_all(&pg)
        .await
        .map_err(database_error)?;
    Ok(Json(Page::new(plans, total, page, page_size)))
}

async fn create_plan(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<PlanBody>,
) -> Result<(StatusCode, Json<PlanRow>), (StatusCode, Json<serde_json::Value>)> {
    validate_plan_body(&request)?;
    let (pg, actor) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    ensure_node_groups(&pg, &request.node_group_ids).await?;

    let name = request.name.trim();
    let mut transaction = pg.begin().await.map_err(database_error)?;
    let plan_id = Uuid::new_v4();
    let plan = sqlx::query_as::<_, PlanRow>(&format!(
        "INSERT INTO plans (id, name, description, speed_limit_mbps, max_conns_per_tunnel, max_new_conns_per_sec, max_tunnels, allow_custom_port, traffic_quota_bytes, traffic_period, duration_days, allowed_protocols, traffic_count_mode) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13) {PLAN_RETURNING}",
    ))
    .bind(plan_id)
    .bind(name)
    .bind(request.description.unwrap_or_default())
    .bind(request.speed_limit_mbps)
    .bind(request.max_conns_per_tunnel)
    .bind(request.max_new_conns_per_sec)
    .bind(request.max_tunnels)
    .bind(request.allow_custom_port)
    .bind(request.traffic_quota_bytes)
    .bind(&request.traffic_period)
    .bind(None::<i32>)
    .bind(&request.allowed_protocols)
    .bind(request.traffic_count_mode.unwrap_or_else(|| "sum".into()))
    .fetch_one(&mut *transaction)
    .await
    .map_err(|_| response_error(unavailable(StatusCode::CONFLICT, "套餐名称已存在或数据库不可用")))?;

    replace_plan_node_groups(&mut transaction, plan.id, &request.node_group_ids).await?;
    write_audit(
        &mut transaction,
        actor.user_id,
        "plan.create",
        "plan",
        plan.id,
    )
    .await?;
    transaction.commit().await.map_err(database_error)?;
    Ok((StatusCode::CREATED, Json(plan)))
}

async fn update_plan(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(plan_id): Path<Uuid>,
    Json(request): Json<PlanBody>,
) -> Result<Json<PlanRow>, (StatusCode, Json<serde_json::Value>)> {
    validate_plan_body(&request)?;
    let (pg, actor) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    ensure_node_groups(&pg, &request.node_group_ids).await?;

    let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM plans WHERE id = $1)")
        .bind(plan_id)
        .fetch_one(&pg)
        .await
        .map_err(database_error)?;
    if !exists {
        return Err(response_error(unavailable(
            StatusCode::NOT_FOUND,
            "套餐不存在",
        )));
    }

    let name = request.name.trim();
    let mut transaction = pg.begin().await.map_err(database_error)?;
    let plan = sqlx::query_as::<_, PlanRow>(&format!(
        "UPDATE plans SET name = $2, description = $3, speed_limit_mbps = $4, max_conns_per_tunnel = $5, max_new_conns_per_sec = $6, max_tunnels = $7, allow_custom_port = $8, traffic_quota_bytes = $9, traffic_period = $10, duration_days = $11, allowed_protocols = $12, traffic_count_mode = $13, updated_at = now() WHERE id = $1 {PLAN_RETURNING}",
    ))
    .bind(plan_id)
    .bind(name)
    .bind(request.description.unwrap_or_default())
    .bind(request.speed_limit_mbps)
    .bind(request.max_conns_per_tunnel)
    .bind(request.max_new_conns_per_sec)
    .bind(request.max_tunnels)
    .bind(request.allow_custom_port)
    .bind(request.traffic_quota_bytes)
    .bind(&request.traffic_period)
    .bind(None::<i32>)
    .bind(&request.allowed_protocols)
    .bind(request.traffic_count_mode.unwrap_or_else(|| "sum".into()))
    .fetch_one(&mut *transaction)
    .await
    .map_err(|_| response_error(unavailable(StatusCode::CONFLICT, "套餐名称已存在或数据库不可用")))?;

    replace_plan_node_groups(&mut transaction, plan_id, &request.node_group_ids).await?;
    write_audit(
        &mut transaction,
        actor.user_id,
        "plan.update",
        "plan",
        plan_id,
    )
    .await?;
    transaction.commit().await.map_err(database_error)?;
    Ok(Json(plan))
}

async fn set_plan_enabled(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(plan_id): Path<Uuid>,
    Json(body): Json<SetPlanEnabled>,
) -> Result<Json<PlanRow>, (StatusCode, Json<serde_json::Value>)> {
    let (pg, actor) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    let mut transaction = pg.begin().await.map_err(database_error)?;
    let plan = sqlx::query_as::<_, PlanRow>(&format!(
        "UPDATE plans SET enabled = $2, updated_at = now() WHERE id = $1 {PLAN_RETURNING}",
    ))
    .bind(plan_id)
    .bind(body.enabled)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(database_error)?
    .ok_or_else(|| response_error(unavailable(StatusCode::NOT_FOUND, "套餐不存在")))?;
    write_audit(
        &mut transaction,
        actor.user_id,
        if body.enabled {
            "plan.enable"
        } else {
            "plan.disable"
        },
        "plan",
        plan_id,
    )
    .await?;
    transaction.commit().await.map_err(database_error)?;
    Ok(Json(plan))
}

async fn delete_plan(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(plan_id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let (pg, actor) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    let in_use: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM user_subscriptions WHERE plan_id = $1",
    )
    .bind(plan_id)
    .fetch_one(&pg)
    .await
    .map_err(database_error)?;
    if in_use > 0 {
        return Err(response_error(unavailable(
            StatusCode::CONFLICT,
            "仍有用户订阅引用该套餐，无法删除",
        )));
    }

    let mut transaction = pg.begin().await.map_err(database_error)?;
    let deleted = sqlx::query("DELETE FROM plans WHERE id = $1")
        .bind(plan_id)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
    if deleted.rows_affected() == 0 {
        return Err(response_error(unavailable(
            StatusCode::NOT_FOUND,
            "套餐不存在",
        )));
    }
    write_audit(
        &mut transaction,
        actor.user_id,
        "plan.delete",
        "plan",
        plan_id,
    )
    .await?;
    transaction.commit().await.map_err(database_error)?;
    Ok(StatusCode::NO_CONTENT)
}
