use super::{database_error, response_error, write_audit};
use crate::{
    payment::{self, billing, CreatePayInput, PayError, PayOutcome},
    setup::{AppState, UserRole, unavailable},
};
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
        .route("/api/v1/orders", get(list_user_orders).post(create_user_order))
        .route("/api/v1/orders/{id}", get(get_user_order))
        .route("/api/v1/orders/{id}/pay", post(pay_user_order))
}

#[derive(Debug, Serialize, FromRow)]
struct OrderRow {
    id: Uuid,
    order_no: String,
    user_id: Uuid,
    user_email: String,
    plan_id: Option<Uuid>,
    plan_name: String,
    kind: String,
    period: String,
    amount_cents: i64,
    status: String,
    payment_channel_id: Option<Uuid>,
    paid_at: Option<String>,
    created_at: String,
}

#[derive(Debug, Serialize, FromRow)]
struct UserOrderRow {
    id: Uuid,
    order_no: String,
    plan_id: Option<Uuid>,
    plan_name: String,
    kind: String,
    period: String,
    amount_cents: i64,
    status: String,
    payment_channel_id: Option<Uuid>,
    paid_at: Option<String>,
    created_at: String,
}

#[derive(Debug, Serialize)]
struct OrderPage {
    items: Vec<OrderRow>,
    total: i64,
    page: u32,
    page_size: u32,
}

#[derive(Debug, Serialize)]
struct UserOrderPage {
    items: Vec<UserOrderRow>,
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
    #[serde(default = "default_page", deserialize_with = "crate::page::deserialize_query_u32")]
    page: u32,
    #[serde(
        default = "default_page_size",
        deserialize_with = "crate::page::deserialize_query_u32"
    )]
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
    period: String,
}

#[derive(Debug, Deserialize)]
struct PayOrder {
    channel_id: Uuid,
}

#[derive(Debug, Serialize)]
struct PayResult {
    order: UserOrderRow,
    pay_url: Option<String>,
}

#[derive(Debug, Serialize, FromRow)]
struct CatalogPlan {
    id: Uuid,
    name: String,
    description: String,
    traffic_period: String,
    speed_limit_mbps: i64,
    traffic_quota_bytes: Option<i64>,
    price_month_cents: Option<i64>,
    price_quarter_cents: Option<i64>,
    price_half_year_cents: Option<i64>,
    price_year_cents: Option<i64>,
    price_two_year_cents: Option<i64>,
    price_three_year_cents: Option<i64>,
    price_traffic_pack_cents: Option<i64>,
    price_reset_pack_cents: Option<i64>,
}

const ORDER_SELECT: &str = "SELECT o.id, o.order_no, o.user_id, u.email AS user_email, o.plan_id, o.plan_name, o.kind, o.period, o.amount_cents, o.status, o.payment_channel_id, o.paid_at::text AS paid_at, o.created_at::text AS created_at FROM orders o JOIN users u ON u.id = o.user_id";
const USER_ORDER_SELECT: &str = "SELECT id, order_no, plan_id, plan_name, kind, period, amount_cents, status, payment_channel_id, paid_at::text AS paid_at, created_at::text AS created_at FROM orders";

async fn list_admin_orders(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<ListOrdersQuery>,
) -> Result<Json<OrderPage>, (StatusCode, Json<Value>)> {
    let (pg, _) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    let kind = optional_enum(&query.kind, &["new", "upgrade", "addon", "gift"], "订单类型无效")?;
    let period = optional_enum(&query.period, billing::ORDER_PERIODS, "订单周期无效")?;
    let status = optional_enum(&query.status, billing::ORDER_STATUSES, "订单状态无效")?;
    let pattern = query.q.as_deref().map(str::trim).filter(|value| !value.is_empty()).map(like_pattern);
    let page = query.page.max(1);
    let page_size = query.page_size.clamp(1, 100);
    let offset = i64::from(page - 1) * i64::from(page_size);
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM orders o JOIN users u ON u.id = o.user_id WHERE ($1::text IS NULL OR u.email ILIKE $1 OR o.plan_name ILIKE $1 OR o.order_no ILIKE $1 OR o.id::text ILIKE $1) AND ($2::text IS NULL OR o.kind = $2) AND ($3::text IS NULL OR o.period = $3) AND ($4::text IS NULL OR o.status = $4)",
    )
    .bind(&pattern)
    .bind(&kind)
    .bind(&period)
    .bind(&status)
    .fetch_one(&pg)
    .await
    .map_err(database_error)?;
    let items = sqlx::query_as::<_, OrderRow>(&format!(
        "{ORDER_SELECT} WHERE ($1::text IS NULL OR u.email ILIKE $1 OR o.plan_name ILIKE $1 OR o.order_no ILIKE $1 OR o.id::text ILIKE $1) AND ($2::text IS NULL OR o.kind = $2) AND ($3::text IS NULL OR o.period = $3) AND ($4::text IS NULL OR o.status = $4) ORDER BY o.created_at DESC, o.id DESC LIMIT $5 OFFSET $6"
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

async fn list_user_orders(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<ListOrdersQuery>,
) -> Result<Json<UserOrderPage>, (StatusCode, Json<Value>)> {
    let (pg, user) = state
        .database_for(&headers, UserRole::User)
        .await
        .map_err(response_error)?;
    let kind = optional_enum(&query.kind, &["new", "upgrade", "addon", "gift"], "订单类型无效")?;
    let period = optional_enum(&query.period, billing::ORDER_PERIODS, "订单周期无效")?;
    let status = optional_enum(&query.status, billing::ORDER_STATUSES, "订单状态无效")?;
    let page = query.page.max(1);
    let page_size = query.page_size.clamp(1, 100);
    let offset = i64::from(page - 1) * i64::from(page_size);
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM orders o WHERE o.user_id = $1 AND ($2::text IS NULL OR o.kind = $2) AND ($3::text IS NULL OR o.period = $3) AND ($4::text IS NULL OR o.status = $4)",
    )
    .bind(user.user_id)
    .bind(&kind)
    .bind(&period)
    .bind(&status)
    .fetch_one(&pg)
    .await
    .map_err(database_error)?;
    let items = sqlx::query_as::<_, UserOrderRow>(&format!(
        "{USER_ORDER_SELECT} WHERE user_id = $1 AND ($2::text IS NULL OR kind = $2) AND ($3::text IS NULL OR period = $3) AND ($4::text IS NULL OR status = $4) ORDER BY created_at DESC, id DESC LIMIT $5 OFFSET $6"
    ))
    .bind(user.user_id)
    .bind(&kind)
    .bind(&period)
    .bind(&status)
    .bind(i64::from(page_size))
    .bind(offset)
    .fetch_all(&pg)
    .await
    .map_err(database_error)?;
    Ok(Json(UserOrderPage {
        items,
        total,
        page,
        page_size,
    }))
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
        "SELECT id, name, description, traffic_period, speed_limit_mbps, traffic_quota_bytes, price_month_cents, price_quarter_cents, price_half_year_cents, price_year_cents, price_two_year_cents, price_three_year_cents, price_traffic_pack_cents, price_reset_pack_cents FROM plans WHERE enabled = TRUE ORDER BY name LIMIT $1 OFFSET $2",
    )
    .bind(page_size)
    .bind(offset)
    .fetch_all(&pg)
    .await
    .map_err(database_error)?;
    Ok(Json(crate::page::Page::new(plans, total, page, page_size)))
}

async fn create_user_order(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<PurchaseOrder>,
) -> Result<(StatusCode, Json<UserOrderRow>), (StatusCode, Json<Value>)> {
    let (pg, actor) = state
        .database_for(&headers, UserRole::User)
        .await
        .map_err(response_error)?;
    if !billing::BILLING_PERIODS.contains(&request.period.as_str()) {
        return Err(response_error(unavailable(StatusCode::BAD_REQUEST, "订单周期无效")));
    }
    let plan = sqlx::query_as::<_, CatalogPlan>(
        "SELECT id, name, description, traffic_period, speed_limit_mbps, traffic_quota_bytes, price_month_cents, price_quarter_cents, price_half_year_cents, price_year_cents, price_two_year_cents, price_three_year_cents, price_traffic_pack_cents, price_reset_pack_cents FROM plans WHERE id = $1 AND enabled = TRUE",
    )
    .bind(request.plan_id)
    .fetch_optional(&pg)
    .await
    .map_err(database_error)?
    .ok_or_else(|| response_error(unavailable(StatusCode::NOT_FOUND, "套餐不存在或已停用")))?;
    let prices = billing::PlanPrices {
        price_month_cents: plan.price_month_cents,
        price_quarter_cents: plan.price_quarter_cents,
        price_half_year_cents: plan.price_half_year_cents,
        price_year_cents: plan.price_year_cents,
        price_two_year_cents: plan.price_two_year_cents,
        price_three_year_cents: plan.price_three_year_cents,
        price_traffic_pack_cents: plan.price_traffic_pack_cents,
        price_reset_pack_cents: plan.price_reset_pack_cents,
    };
    let amount_cents = prices.amount_for(&request.period).ok_or_else(|| {
        response_error(unavailable(StatusCode::BAD_REQUEST, "该套餐未设置此周期价格"))
    })?;
    if billing::is_addon(&request.period) {
        let has_sub: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM user_subscriptions WHERE user_id = $1 AND status = 'active')",
        )
        .bind(actor.user_id)
        .fetch_one(&pg)
        .await
        .map_err(database_error)?;
        if !has_sub {
            return Err(response_error(unavailable(
                StatusCode::BAD_REQUEST,
                "请先购买订阅后再购买流量包或重置包",
            )));
        }
    }
    let kind = if billing::is_addon(&request.period) {
        "addon"
    } else {
        let upgrading: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM user_subscriptions WHERE user_id = $1 AND status = 'active')",
        )
        .bind(actor.user_id)
        .fetch_one(&pg)
        .await
        .map_err(database_error)?;
        if upgrading { "upgrade" } else { "new" }
    };
    let order_id = Uuid::new_v4();
    let order_no = billing::new_order_no(Utc::now());
    let mut transaction = pg.begin().await.map_err(database_error)?;
    sqlx::query(
        "INSERT INTO orders (id, order_no, user_id, plan_id, plan_name, kind, period, amount_cents, status) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,'pending')",
    )
    .bind(order_id)
    .bind(&order_no)
    .bind(actor.user_id)
    .bind(plan.id)
    .bind(&plan.name)
    .bind(kind)
    .bind(&request.period)
    .bind(amount_cents)
    .execute(&mut *transaction)
    .await
    .map_err(database_error)?;
    write_audit(&mut transaction, actor.user_id, "order.create", "order", order_id).await?;
    transaction.commit().await.map_err(database_error)?;
    let row = fetch_user_order(&pg, actor.user_id, order_id).await?;
    Ok((StatusCode::CREATED, Json(row)))
}

async fn get_user_order(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<UserOrderRow>, (StatusCode, Json<Value>)> {
    let (pg, user) = state
        .database_for(&headers, UserRole::User)
        .await
        .map_err(response_error)?;
    fetch_user_order(&pg, user.user_id, id).await.map(Json)
}

async fn pay_user_order(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<PayOrder>,
) -> Result<Json<PayResult>, (StatusCode, Json<Value>)> {
    let (pg, actor) = state
        .database_for(&headers, UserRole::User)
        .await
        .map_err(response_error)?;
    let mut transaction = pg.begin().await.map_err(database_error)?;
    let order = sqlx::query_as::<_, UserOrderRow>(
        &format!("{USER_ORDER_SELECT} WHERE id = $1 AND user_id = $2 FOR UPDATE"),
    )
    .bind(id)
    .bind(actor.user_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(database_error)?
    .ok_or_else(|| response_error(unavailable(StatusCode::NOT_FOUND, "订单不存在")))?;
    if order.status != "pending" {
        return Err(response_error(unavailable(
            StatusCode::CONFLICT,
            "订单已支付或已关闭",
        )));
    }
    let Some(plan_id) = order.plan_id else {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "订单套餐已失效",
        )));
    };
    let channel = sqlx::query_as::<_, (String, Value)>(
        "SELECT kind, config FROM payment_channels WHERE id = $1 AND enabled = TRUE",
    )
    .bind(request.channel_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(database_error)?
    .ok_or_else(|| response_error(unavailable(StatusCode::BAD_REQUEST, "支付渠道不可用")))?;
    let driver = payment::find(&channel.0).ok_or_else(|| {
        response_error(unavailable(StatusCode::BAD_REQUEST, "未知支付渠道类型"))
    })?;
    let input = CreatePayInput {
        order_id: order.id,
        user_id: actor.user_id,
        amount_cents: order.amount_cents,
        subject: &order.plan_name,
        channel_config: &channel.1,
        notify_url: "",
        return_url: "",
    };
    let outcome = driver.create_payment(&mut transaction, &input).await.map_err(map_pay_error)?;
    let pay_url = match outcome {
        PayOutcome::Completed => {
            let subscription_id = super::users::fulfill_paid_order(
                &mut transaction,
                actor.user_id,
                actor.user_id,
                plan_id,
                &order.period,
                Utc::now(),
            )
            .await?;
            sqlx::query(
                "UPDATE orders SET status = 'completed', payment_channel_id = $2, subscription_id = $3, paid_at = now() WHERE id = $1",
            )
            .bind(order.id)
            .bind(request.channel_id)
            .bind(subscription_id)
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
            None
        }
        PayOutcome::Pending { pay_url } => {
            sqlx::query(
                "UPDATE orders SET status = 'paying', payment_channel_id = $2 WHERE id = $1",
            )
            .bind(order.id)
            .bind(request.channel_id)
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
            pay_url
        }
    };
    transaction.commit().await.map_err(database_error)?;
    crate::ws::sync_user_tunnel_limits_from_active_subscription(&pg, actor.user_id).await;
    crate::quota::reevaluate_user(&pg, state.redis_connection(), actor.user_id).await;
    let row = fetch_user_order(&pg, actor.user_id, id).await?;
    Ok(Json(PayResult { order: row, pay_url }))
}

fn map_pay_error(error: PayError) -> (StatusCode, Json<Value>) {
    match error {
        PayError::Unavailable(message) | PayError::Message(message) => {
            response_error(unavailable(StatusCode::BAD_REQUEST, message))
        }
    }
}

async fn fetch_user_order(
    pg: &PgPool,
    user_id: Uuid,
    id: Uuid,
) -> Result<UserOrderRow, (StatusCode, Json<Value>)> {
    sqlx::query_as::<_, UserOrderRow>(
        &format!("{USER_ORDER_SELECT} WHERE id = $1 AND user_id = $2"),
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(pg)
    .await
    .map_err(database_error)?
    .ok_or_else(|| response_error(unavailable(StatusCode::NOT_FOUND, "订单不存在")))
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
