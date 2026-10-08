use super::{database_error, response_error, write_audit};
use crate::setup::{AppState, UserRole, unavailable};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get, post},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{FromRow, PgPool};
use std::sync::Arc;
use uuid::Uuid;

pub fn admin_router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/v1/admin/orders",
            get(list_admin_orders).post(create_admin_order),
        )
        .route(
            "/api/v1/admin/orders/{id}",
            get(get_admin_order).delete(delete_admin_order),
        )
}

pub fn user_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/plans", get(list_user_plans))
        .route("/api/v1/orders", post(purchase_plan))
}

#[derive(Debug, Serialize, FromRow)]
struct OrderRow {
    id: Uuid,
    user_id: Uuid,
    user_email: String,
    plan_id: Option<Uuid>,
    plan_name: String,
    kind: String,
    period: String,
    amount_cents: i64,
    status: String,
    created_at: String,
}

#[derive(Debug, Serialize)]
struct OrderPage {
    items: Vec<OrderRow>,
    total: i64,
    page: u32,
    page_size: u32,
}

#[derive(Debug, Deserialize)]
struct ListOrdersQuery {
    q: Option<String>,
    kind: Option<String>,
    period: Option<String>,
    status: Option<String>,
    #[serde(default = "default_page")]
    page: u32,
    #[serde(default = "default_page_size")]
    page_size: u32,
}

fn default_page() -> u32 {
    1
}

fn default_page_size() -> u32 {
    20
}

#[derive(Debug, Deserialize)]
struct CreateOrder {
    user_id: Uuid,
    plan_id: Uuid,
    #[serde(default)]
    amount_cents: i64,
    expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
struct PurchaseOrder {
    plan_id: Uuid,
}

#[derive(Debug, Serialize, FromRow)]
struct CatalogPlan {
    id: Uuid,
    name: String,
    description: String,
    traffic_period: String,
    speed_limit_mbps: i64,
    traffic_quota_bytes: Option<i64>,
}

const ORDER_SELECT: &str = "SELECT o.id, o.user_id, u.email AS user_email, o.plan_id, o.plan_name, o.kind, o.period, o.amount_cents, o.status, o.created_at::text AS created_at FROM orders o JOIN users u ON u.id = o.user_id";

async fn list_admin_orders(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<ListOrdersQuery>,
) -> Result<Json<OrderPage>, (StatusCode, Json<Value>)> {
    let (pg, _) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    let kind = optional_enum(&query.kind, &["new", "upgrade"], "订单类型无效")?;
    let period = optional_enum(
        &query.period,
        &["day", "week", "month", "quarter", "year", "lifetime"],
        "订单周期无效",
    )?;
    let status = optional_enum(
        &query.status,
        &["completed", "cancelled", "credited"],
        "订单状态无效",
    )?;
    let pattern = query.q.as_deref().map(str::trim).filter(|value| !value.is_empty()).map(like_pattern);
    let page = query.page.max(1);
    let page_size = query.page_size.clamp(1, 100);
    let offset = i64::from(page - 1) * i64::from(page_size);
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM orders o JOIN users u ON u.id = o.user_id WHERE ($1::text IS NULL OR u.email ILIKE $1 OR o.plan_name ILIKE $1 OR o.id::text ILIKE $1) AND ($2::text IS NULL OR o.kind = $2) AND ($3::text IS NULL OR o.period = $3) AND ($4::text IS NULL OR o.status = $4)",
    )
    .bind(&pattern)
    .bind(&kind)
    .bind(&period)
    .bind(&status)
    .fetch_one(&pg)
    .await
    .map_err(database_error)?;
    let items = sqlx::query_as::<_, OrderRow>(&format!(
        "{ORDER_SELECT} WHERE ($1::text IS NULL OR u.email ILIKE $1 OR o.plan_name ILIKE $1 OR o.id::text ILIKE $1) AND ($2::text IS NULL OR o.kind = $2) AND ($3::text IS NULL OR o.period = $3) AND ($4::text IS NULL OR o.status = $4) ORDER BY o.created_at DESC, o.id DESC LIMIT $5 OFFSET $6"
    ))
    .bind(&pattern)
    .bind(&kind)
    .bind(&period)
    .bind(&status)
    .bind(i64::from(page_size))
    .bind(offset)
    .fetch_all(&pg)
    .await
    .map_err(database_error)?;
    Ok(Json(OrderPage {
        items,
        total,
        page,
        page_size,
    }))
}

async fn get_admin_order(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<OrderRow>, (StatusCode, Json<Value>)> {
    let (pg, _) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    fetch_order(&pg, id).await.map(Json)
}

async fn create_admin_order(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<CreateOrder>,
) -> Result<(StatusCode, Json<OrderRow>), (StatusCode, Json<Value>)> {
    let (pg, actor) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    let user_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM users WHERE id = $1 AND status = 'active')",
    )
    .bind(request.user_id)
    .fetch_one(&pg)
    .await
    .map_err(database_error)?;
    if !user_exists {
        return Err(response_error(unavailable(
            StatusCode::NOT_FOUND,
            "用户不存在或已停用",
        )));
    }
    let mut transaction = pg.begin().await.map_err(database_error)?;
    let order_id = super::users::replace_active_subscription_for_user(
        &mut transaction,
        actor.user_id,
        request.user_id,
        request.plan_id,
        Utc::now(),
        request.expires_at,
        request.amount_cents,
    )
    .await?;
    transaction.commit().await.map_err(database_error)?;
    crate::ws::sync_user_tunnel_limits_from_active_subscription(&pg, request.user_id).await;
    crate::quota::reevaluate_user(&pg, state.redis_connection(), request.user_id).await;
    let row = fetch_order(&pg, order_id).await?;
    Ok((StatusCode::CREATED, Json(row)))
}

async fn delete_admin_order(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, Json<Value>)> {
    let (pg, actor) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    let mut transaction = pg.begin().await.map_err(database_error)?;
    let deleted = sqlx::query("DELETE FROM orders WHERE id = $1")
        .bind(id)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
    if deleted.rows_affected() == 0 {
        return Err(response_error(unavailable(StatusCode::NOT_FOUND, "订单不存在")));
    }
    write_audit(&mut transaction, actor.user_id, "order.delete", "order", id).await?;
    transaction.commit().await.map_err(database_error)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_user_plans(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(page): Query<crate::page::PageQuery>,
) -> Result<Json<crate::page::Page<CatalogPlan>>, (StatusCode, Json<Value>)> {
    let (pg, _) = state
        .database_for(&headers, UserRole::User)
        .await
        .map_err(response_error)?;
    let (page, page_size, offset) = page.resolve();
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*)::bigint FROM plans WHERE enabled = TRUE")
        .fetch_one(&pg)
        .await
        .map_err(database_error)?;
    let plans = sqlx::query_as::<_, CatalogPlan>(
        "SELECT id, name, description, traffic_period, speed_limit_mbps, traffic_quota_bytes FROM plans WHERE enabled = TRUE ORDER BY name LIMIT $1 OFFSET $2",
    )
    .bind(page_size)
    .bind(offset)
    .fetch_all(&pg)
    .await
    .map_err(database_error)?;
    Ok(Json(crate::page::Page::new(plans, total, page, page_size)))
}

async fn purchase_plan(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<PurchaseOrder>,
) -> Result<(StatusCode, Json<OrderRow>), (StatusCode, Json<Value>)> {
    let (pg, actor) = state
        .database_for(&headers, UserRole::User)
        .await
        .map_err(response_error)?;
    let mut transaction = pg.begin().await.map_err(database_error)?;
    let order_id = super::users::replace_active_subscription_for_user(
        &mut transaction,
        actor.user_id,
        actor.user_id,
        request.plan_id,
        Utc::now(),
        None,
        0,
    )
    .await?;
    transaction.commit().await.map_err(database_error)?;
    crate::ws::sync_user_tunnel_limits_from_active_subscription(&pg, actor.user_id).await;
    crate::quota::reevaluate_user(&pg, state.redis_connection(), actor.user_id).await;
    let row = fetch_order(&pg, order_id).await?;
    Ok((StatusCode::CREATED, Json(row)))
}

async fn fetch_order(pg: &PgPool, id: Uuid) -> Result<OrderRow, (StatusCode, Json<Value>)> {
    sqlx::query_as::<_, OrderRow>(&format!("{ORDER_SELECT} WHERE o.id = $1"))
        .bind(id)
        .fetch_optional(pg)
        .await
        .map_err(database_error)?
        .ok_or_else(|| response_error(unavailable(StatusCode::NOT_FOUND, "订单不存在")))
}

fn optional_enum(
    value: &Option<String>,
    allowed: &[&str],
    message: &'static str,
) -> Result<Option<String>, (StatusCode, Json<Value>)> {
    let Some(value) = value.as_deref().map(str::trim).filter(|item| !item.is_empty()) else {
        return Ok(None);
    };
    if allowed.contains(&value) {
        Ok(Some(value.to_owned()))
    } else {
        Err(response_error(unavailable(StatusCode::BAD_REQUEST, message)))
    }
}

fn like_pattern(value: &str) -> String {
    let cleaned: String = value.chars().filter(|ch| !matches!(ch, '%' | '_' | '\\')).collect();
    format!("%{cleaned}%")
}
