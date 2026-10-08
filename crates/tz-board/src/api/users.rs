use super::{database_error, response_error, write_audit};
use crate::{
    setup::{AppState, UserRole, unavailable},
    store,
};
use crate::page::{Page, PageQuery};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get, patch, post, put},
};
use chrono::{DateTime, Datelike, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::sync::Arc;
use uuid::Uuid;

pub fn admin_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/admin/users", get(list_users).post(create_user))
        .route(
            "/api/v1/admin/users/{user_id}",
            put(admin_update_user).delete(admin_delete_user),
        )
        .route(
            "/api/v1/admin/users/{user_id}/status",
            patch(admin_set_user_status),
        )
}

pub fn user_router() -> Router<Arc<AppState>> {
    Router::new().route("/api/v1/subscription", get(user_subscription))
}

#[derive(Debug, Serialize, FromRow)]
struct UserRow {
    id: Uuid,
    email: String,
    role: String,
    status: String,
    last_login_at: Option<String>,
    created_at: String,
    subscription_plan_id: Option<Uuid>,
    subscription_plan_name: Option<String>,
    subscription_status: Option<String>,
    subscription_expires_at: Option<String>,
    traffic_used_bytes: i64,
    traffic_quota_bytes: Option<i64>,
    subscription_speed_limit_mbps: Option<i64>,
    subscription_max_tunnels: Option<i32>,
    traffic_exhausted: bool,
    #[serde(skip)]
    traffic_count_mode: Option<String>,
    balance_cents: i64,
}

const LIST_USERS_SQL: &str = r#"
SELECT
    u.id,
    u.email,
    u.role,
    u.status,
    u.last_login_at::text AS last_login_at,
    u.created_at::text AS created_at,
    sub.plan_id AS subscription_plan_id,
    sub.plan_name AS subscription_plan_name,
    sub.subscription_status AS subscription_status,
    sub.expires_at::text AS subscription_expires_at,
    COALESCE(sub.traffic_used_bytes, 0) AS traffic_used_bytes,
    sub.traffic_quota_bytes AS traffic_quota_bytes,
    sub.speed_limit_mbps AS subscription_speed_limit_mbps,
    sub.max_tunnels AS subscription_max_tunnels,
    COALESCE(sub.traffic_exhausted, FALSE) AS traffic_exhausted,
    sub.traffic_count_mode AS traffic_count_mode,
    u.balance_cents AS balance_cents
FROM users u
LEFT JOIN LATERAL (
    SELECT
        s.plan_id,
        s.status AS subscription_status,
        s.traffic_quota_bytes,
        s.speed_limit_mbps,
        s.max_tunnels,
        s.exhausted_period_start IS NOT NULL AS traffic_exhausted,
        s.traffic_count_mode,
        s.expires_at,
        p.name AS plan_name,
        COALESCE(tu.traffic_used_bytes, 0) AS traffic_used_bytes
    FROM user_subscriptions s
    LEFT JOIN plans p ON p.id = s.plan_id
    LEFT JOIN LATERAL (
        SELECT CASE s.traffic_count_mode
            WHEN 'inbound' THEN tu.bytes_in
            WHEN 'outbound' THEN tu.bytes_out
            WHEN 'max' THEN GREATEST(tu.bytes_in, tu.bytes_out)
            ELSE tu.bytes_in + tu.bytes_out
        END AS traffic_used_bytes
        FROM traffic_usage tu
        WHERE tu.subscription_id = s.id
        ORDER BY tu.period_start DESC
        LIMIT 1
    ) tu ON true
    WHERE s.user_id = u.id
    ORDER BY
        CASE
            WHEN s.status = 'active'
                AND s.starts_at <= now()
                AND (s.expires_at IS NULL OR s.expires_at > now())
                AND s.exhausted_period_start IS NULL
            THEN 0
            ELSE 1
        END,
        s.starts_at DESC
    LIMIT 1
) sub ON true
ORDER BY u.created_at DESC
"#;

#[derive(Deserialize)]
struct CreateUser {
    email: String,
    password: String,
    #[serde(default = "default_account_role")]
    role: String,
    plan_id: Option<Uuid>,
    expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    balance_cents: i64,
}

fn default_account_role() -> String {
    "user".to_string()
}

#[derive(Deserialize)]
struct UpdateUser {
    email: String,
    role: String,
    password: Option<String>,
    plan_id: Option<Uuid>,
    expires_at: Option<DateTime<Utc>>,
    balance_cents: i64,
    /// 以下字段只改用户订阅副本，不影响套餐本身；缺省表示不修改。
    #[serde(default)]
    traffic_used_bytes: Option<i64>,
    /// `null` 表示不限流量。
    #[serde(default, deserialize_with = "explicit_option")]
    traffic_quota_bytes: Option<Option<i64>>,
    /// 0 表示不限速。
    #[serde(default)]
    speed_limit_mbps: Option<i64>,
    #[serde(default)]
    max_tunnels: Option<i32>,
}

fn explicit_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

impl UpdateUser {
    fn has_subscription_overrides(&self) -> bool {
        self.traffic_used_bytes.is_some()
            || self.traffic_quota_bytes.is_some()
            || self.speed_limit_mbps.is_some()
            || self.max_tunnels.is_some()
    }
}

#[derive(Deserialize)]
struct SetUserStatus {
    status: String,
}

#[derive(FromRow)]
struct PlanSnapshot {
    name: String,
    speed_limit_mbps: i64,
    max_conns_per_tunnel: i32,
    max_new_conns_per_sec: i32,
    max_tunnels: i32,
    allow_custom_port: bool,
    traffic_quota_bytes: Option<i64>,
    traffic_period: String,
    traffic_count_mode: String,
    allowed_protocols: Vec<String>,
}

async fn list_users(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(page): Query<PageQuery>,
) -> Result<Json<Page<UserRow>>, (StatusCode, Json<serde_json::Value>)> {
    let (pg, _) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    let (page, page_size, offset) = page.resolve();
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*)::bigint FROM users")
        .fetch_one(&pg)
        .await
        .map_err(database_error)?;
    let mut users = sqlx::query_as::<_, UserRow>(&format!("{LIST_USERS_SQL} LIMIT $1 OFFSET $2"))
        .bind(page_size)
        .bind(offset)
        .fetch_all(&pg)
        .await
        .map_err(database_error)?;
    add_pending_traffic(&pg, state.redis_connection(), &mut users).await;
    Ok(Json(Page::new(users, total, page, page_size)))
}

/// 已用流量 = PG 已结算 + Redis 中尚未落库的上报，与配额实时判定口径一致。
async fn add_pending_traffic(
    pg: &sqlx::PgPool,
    redis: Option<redis::aio::ConnectionManager>,
    users: &mut [UserRow],
) {
    let Some(mut redis) = redis else {
        return;
    };
    let user_ids: Vec<Uuid> = users
        .iter()
        .filter(|user| user.traffic_count_mode.is_some())
        .map(|user| user.id)
        .collect();
    if user_ids.is_empty() {
        return;
    }
    let Ok(tunnels) = sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT id, user_id FROM tunnels WHERE user_id = ANY($1)",
    )
    .bind(&user_ids)
    .fetch_all(pg)
    .await
    else {
        return;
    };
    let tunnel_ids: Vec<Uuid> = tunnels.iter().map(|(id, _)| *id).collect();
    let pending = crate::stats::redis_pending_settle_each(&mut redis, &tunnel_ids).await;
    let mut per_user: std::collections::HashMap<Uuid, (i64, i64)> = std::collections::HashMap::new();
    for ((_, user_id), (bytes_in, bytes_out)) in tunnels.iter().zip(pending) {
        let entry = per_user.entry(*user_id).or_default();
        entry.0 += bytes_in;
        entry.1 += bytes_out;
    }
    for user in users.iter_mut() {
        let (Some(mode), Some((bytes_in, bytes_out))) =
            (user.traffic_count_mode.as_deref(), per_user.get(&user.id))
        else {
            continue;
        };
        user.traffic_used_bytes = user
            .traffic_used_bytes
            .saturating_add(crate::quota::CountMode::parse(mode).used(*bytes_in, *bytes_out));
    }
}

async fn create_user(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<CreateUser>,
) -> Result<(StatusCode, Json<UserRow>), (StatusCode, Json<serde_json::Value>)> {
    let (pg, actor) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    let email = request.email.trim().to_lowercase();
    if !valid_email(&email) {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "请输入有效的邮箱",
        )));
    }
    if let Err(message) = store::validate_password(&request.password) {
        return Err(response_error(unavailable(StatusCode::BAD_REQUEST, message)));
    }
    let password_hash = store::hash_password(&request.password).map_err(|_| {
        response_error(unavailable(
            StatusCode::INTERNAL_SERVER_ERROR,
            "密码处理失败",
        ))
    })?;
    let role = request.role.trim();
    if role != "admin" && role != "user" {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "角色必须为 admin 或 user",
        )));
    }
    if request.expires_at.is_some() && request.plan_id.is_none() {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "指定到期时间时必须选择订阅套餐",
        )));
    }
    if request.balance_cents < 0 {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "余额不能为负数",
        )));
    }
    let mut transaction = pg.begin().await.map_err(database_error)?;
    let user_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, email, password_hash, role, balance_cents) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(user_id)
    .bind(&email)
    .bind(&password_hash)
    .bind(role)
    .bind(request.balance_cents)
    .execute(&mut *transaction)
        .await
        .map_err(|_| response_error(unavailable(StatusCode::CONFLICT, "邮箱已存在或数据库不可用")))?;
    write_audit(&mut transaction, actor.user_id, "user.create", "user", user_id).await?;
    if let Some(plan_id) = request.plan_id {
        replace_active_subscription_for_user(
            &mut transaction,
            actor.user_id,
            user_id,
            plan_id,
            Utc::now(),
            request.expires_at,
            0,
        )
        .await?;
    }
    let user = sqlx::query_as::<_, UserRow>(&format!(
        "SELECT * FROM ({LIST_USERS_SQL}) listed WHERE listed.id = $1"
    ))
    .bind(user_id)
    .fetch_one(&mut *transaction)
    .await
    .map_err(database_error)?;
    transaction.commit().await.map_err(database_error)?;
    if request.plan_id.is_some() {
        crate::ws::sync_user_tunnel_limits_from_active_subscription(&pg, user_id).await;
    }
    Ok((StatusCode::CREATED, Json(user)))
}

async fn admin_update_user(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
    Json(request): Json<UpdateUser>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let (pg, actor) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    let email = request.email.trim().to_lowercase();
    if !valid_email(&email) {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "请输入有效的邮箱",
        )));
    }
    let role = request.role.trim();
    if role != "admin" && role != "user" {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "角色必须为 admin 或 user",
        )));
    }
    if request.balance_cents < 0 {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "余额不能为负数",
        )));
    }
    validate_subscription_overrides(&request)?;
    let password_hash = if let Some(password) = request.password.clone() {
        let trimmed = password.trim();
        if trimmed.is_empty() {
            None
        } else {
            if let Err(message) = store::validate_password(trimmed) {
                return Err(response_error(unavailable(StatusCode::BAD_REQUEST, message)));
            }
            Some(store::hash_password(trimmed).map_err(|_| {
                response_error(unavailable(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "密码处理失败",
                ))
            })?)
        }
    } else {
        None
    };
    let settle_guard = if request.traffic_used_bytes.is_some() {
        Some(crate::stats::settle_lock().await)
    } else {
        None
    };
    let mut transaction = pg.begin().await.map_err(database_error)?;
    let updated = if let Some(hash) = password_hash {
        sqlx::query(
            "UPDATE users SET email = $2, role = $3, password_hash = $4, balance_cents = $5, updated_at = now() WHERE id = $1",
        )
        .bind(user_id)
        .bind(&email)
        .bind(role)
        .bind(hash)
        .bind(request.balance_cents)
        .execute(&mut *transaction)
        .await
    } else {
        sqlx::query(
            "UPDATE users SET email = $2, role = $3, balance_cents = $4, updated_at = now() WHERE id = $1",
        )
        .bind(user_id)
        .bind(&email)
        .bind(role)
        .bind(request.balance_cents)
        .execute(&mut *transaction)
        .await
    }
    .map_err(|_| {
        response_error(unavailable(
            StatusCode::CONFLICT,
            "邮箱已存在或数据库不可用",
        ))
    })?;
    if updated.rows_affected() == 0 {
        return Err(response_error(unavailable(
            StatusCode::NOT_FOUND,
            "用户不存在",
        )));
    }
    write_audit(&mut transaction, actor.user_id, "user.update", "user", user_id).await?;
    let mut subscription_changed = false;
    if let Some(plan_id) = request.plan_id {
        replace_active_subscription_for_user(
            &mut transaction,
            actor.user_id,
            user_id,
            plan_id,
            Utc::now(),
            request.expires_at,
            0,
        )
        .await?;
        subscription_changed = true;
    } else if let Some(expires_at) = request.expires_at {
        patch_active_subscription_expires(&mut transaction, actor.user_id, user_id, expires_at)
            .await?;
        subscription_changed = true;
    }
    if request.has_subscription_overrides() {
        patch_subscription_overrides(&mut transaction, actor.user_id, user_id, &request).await?;
        subscription_changed = true;
    }
    transaction.commit().await.map_err(database_error)?;
    if settle_guard.is_some() {
        let tunnel_ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM tunnels WHERE user_id = $1")
            .bind(user_id)
            .fetch_all(&pg)
            .await
            .unwrap_or_default();
        let mut redis = state.redis_connection();
        crate::stats::discard_pending_settle(redis.as_mut(), &tunnel_ids).await;
    }
    drop(settle_guard);
    if subscription_changed {
        crate::ws::sync_user_tunnel_limits_from_active_subscription(&pg, user_id).await;
        crate::quota::reevaluate_user(&pg, state.redis_connection(), user_id).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

fn validate_subscription_overrides(
    request: &UpdateUser,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    let invalid = |message: &str| Err(response_error(unavailable(StatusCode::BAD_REQUEST, message)));
    if request.traffic_used_bytes.is_some_and(|used| used < 0) {
        return invalid("已用流量不能为负数");
    }
    if matches!(request.traffic_quota_bytes, Some(Some(quota)) if quota <= 0) {
        return invalid("流量上限必须大于 0，不限请留空");
    }
    if request.speed_limit_mbps.is_some_and(|speed| speed < 0) {
        return invalid("限速不能为负数（0 表示不限速）");
    }
    if request.max_tunnels.is_some_and(|max| max < 0) {
        return invalid("隧道数上限不能为负数");
    }
    Ok(())
}

/// 改用户订阅副本的已用流量 / 流量上限 / 限速 / 隧道数上限（套餐本身不变）。
async fn patch_subscription_overrides(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor_id: Uuid,
    user_id: Uuid,
    request: &UpdateUser,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    #[derive(FromRow)]
    struct ActiveSub {
        id: Uuid,
        traffic_period: String,
        traffic_count_mode: String,
        starts_at: DateTime<Utc>,
        period_anchor: i16,
    }
    let sub = sqlx::query_as::<_, ActiveSub>(
        "SELECT id, traffic_period, traffic_count_mode, starts_at, period_anchor FROM user_subscriptions WHERE user_id = $1 AND status = 'active' ORDER BY starts_at DESC LIMIT 1 FOR UPDATE",
    )
    .bind(user_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .ok_or_else(|| {
        response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "用户没有有效订阅，请先分配套餐",
        ))
    })?;
    if let Some(quota) = request.traffic_quota_bytes {
        sqlx::query("UPDATE user_subscriptions SET traffic_quota_bytes = $2, updated_at = now() WHERE id = $1")
            .bind(sub.id)
            .bind(quota)
            .execute(&mut **transaction)
            .await
            .map_err(database_error)?;
    }
    if let Some(speed) = request.speed_limit_mbps {
        sqlx::query("UPDATE user_subscriptions SET speed_limit_mbps = $2, updated_at = now() WHERE id = $1")
            .bind(sub.id)
            .bind(speed)
            .execute(&mut **transaction)
            .await
            .map_err(database_error)?;
    }
    if let Some(max_tunnels) = request.max_tunnels {
        sqlx::query("UPDATE user_subscriptions SET max_tunnels = $2, updated_at = now() WHERE id = $1")
            .bind(sub.id)
            .bind(max_tunnels)
            .execute(&mut **transaction)
            .await
            .map_err(database_error)?;
    }
    if let Some(used) = request.traffic_used_bytes {
        let period_start = crate::subscription_period::subscription_period_start(
            &sub.traffic_period,
            sub.starts_at,
            sub.period_anchor,
            Utc::now(),
        );
        let (bytes_in, bytes_out) =
            crate::quota::CountMode::parse(&sub.traffic_count_mode).split_used(used);
        sqlx::query(
            "INSERT INTO traffic_usage (subscription_id, period_start, bytes_in, bytes_out, updated_at) VALUES ($1, $2, $3, $4, now()) ON CONFLICT (subscription_id, period_start) DO UPDATE SET bytes_in = EXCLUDED.bytes_in, bytes_out = EXCLUDED.bytes_out, updated_at = now()",
        )
        .bind(sub.id)
        .bind(period_start)
        .bind(bytes_in)
        .bind(bytes_out)
        .execute(&mut **transaction)
        .await
        .map_err(database_error)?;
    }
    write_audit(
        transaction,
        actor_id,
        "subscription.override",
        "subscription",
        sub.id,
    )
    .await?;
    Ok(())
}

async fn admin_set_user_status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
    Json(request): Json<SetUserStatus>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let (pg, actor) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    let status = request.status.trim();
    if status != "active" && status != "disabled" {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "状态必须为 active 或 disabled",
        )));
    }
    if actor.user_id == user_id {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "不能修改当前登录账户的状态",
        )));
    }
    let mut transaction = pg.begin().await.map_err(database_error)?;
    let updated = sqlx::query(
        "UPDATE users SET status = $2, updated_at = now() WHERE id = $1",
    )
    .bind(user_id)
    .bind(status)
    .execute(&mut *transaction)
    .await
    .map_err(database_error)?;
    if updated.rows_affected() == 0 {
        return Err(response_error(unavailable(
            StatusCode::NOT_FOUND,
            "用户不存在",
        )));
    }
    write_audit(
        &mut transaction,
        actor.user_id,
        "user.status",
        "user",
        user_id,
    )
    .await?;
    transaction.commit().await.map_err(database_error)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn admin_delete_user(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let (pg, actor) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    if actor.user_id == user_id {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "不能删除当前登录账户",
        )));
    }
    let mut transaction = pg.begin().await.map_err(database_error)?;
    let deleted = sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user_id)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
    if deleted.rows_affected() == 0 {
        return Err(response_error(unavailable(
            StatusCode::NOT_FOUND,
            "用户不存在",
        )));
    }
    write_audit(
        &mut transaction,
        actor.user_id,
        "user.delete",
        "user",
        user_id,
    )
    .await?;
    transaction.commit().await.map_err(database_error)?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn replace_active_subscription_for_user(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor_id: Uuid,
    user_id: Uuid,
    plan_id: Uuid,
    starts_at: DateTime<Utc>,
    expires_at_override: Option<DateTime<Utc>>,
    amount_cents: i64,
) -> Result<Uuid, (StatusCode, Json<serde_json::Value>)> {
    if amount_cents < 0 {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "支付金额不能为负数",
        )));
    }
    let plan = sqlx::query_as::<_, PlanSnapshot>(
        "SELECT name, speed_limit_mbps, max_conns_per_tunnel, max_new_conns_per_sec, max_tunnels, allow_custom_port, traffic_quota_bytes, traffic_period, traffic_count_mode, allowed_protocols FROM plans WHERE id = $1 AND enabled = TRUE",
    )
    .bind(plan_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .ok_or_else(|| response_error(unavailable(StatusCode::NOT_FOUND, "套餐不存在或已停用")))?;
    let group_ids = sqlx::query_scalar::<_, Uuid>(
        "SELECT node_group_id FROM plan_node_groups WHERE plan_id = $1 ORDER BY node_group_id",
    )
    .bind(plan_id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    if group_ids.is_empty() {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "套餐必须至少允许一个节点组",
        )));
    }
    if expires_at_override.is_some_and(|at| at <= starts_at) {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "到期时间必须晚于开始时间",
        )));
    }
    let period_anchor = starts_at.day() as i16;

    let existing_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM user_subscriptions WHERE user_id = $1 AND status = 'active' FOR UPDATE",
    )
    .bind(user_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;

    let subscription_id = if let Some(subscription_id) = existing_id {
        sqlx::query(
            "UPDATE user_subscriptions SET plan_id = $2, speed_limit_mbps = $3, max_conns_per_tunnel = $4, max_new_conns_per_sec = $5, max_tunnels = $6, allow_custom_port = $7, traffic_quota_bytes = $8, traffic_period = $9, duration_days = NULL, allowed_protocols = $10, traffic_count_mode = $11, starts_at = $12, expires_at = COALESCE($13, expires_at), period_anchor = $14, exhausted_period_start = NULL, updated_at = now() WHERE id = $1",
        )
        .bind(subscription_id)
        .bind(plan_id)
        .bind(plan.speed_limit_mbps)
        .bind(plan.max_conns_per_tunnel)
        .bind(plan.max_new_conns_per_sec)
        .bind(plan.max_tunnels)
        .bind(plan.allow_custom_port)
        .bind(plan.traffic_quota_bytes)
        .bind(&plan.traffic_period)
        .bind(&plan.allowed_protocols)
        .bind(&plan.traffic_count_mode)
        .bind(starts_at)
        .bind(expires_at_override)
        .bind(period_anchor)
        .execute(&mut **transaction)
        .await
        .map_err(database_error)?;
        sqlx::query("DELETE FROM subscription_node_groups WHERE subscription_id = $1")
            .bind(subscription_id)
            .execute(&mut **transaction)
            .await
            .map_err(database_error)?;
        write_audit(
            transaction,
            actor_id,
            "subscription.update",
            "subscription",
            subscription_id,
        )
        .await?;
        subscription_id
    } else {
        let subscription_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO user_subscriptions (id, user_id, plan_id, status, speed_limit_mbps, max_conns_per_tunnel, max_new_conns_per_sec, max_tunnels, allow_custom_port, traffic_quota_bytes, traffic_period, duration_days, allowed_protocols, traffic_count_mode, starts_at, expires_at, period_anchor) VALUES ($1,$2,$3,'active',$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16)",
        )
        .bind(subscription_id)
        .bind(user_id)
        .bind(plan_id)
        .bind(plan.speed_limit_mbps)
        .bind(plan.max_conns_per_tunnel)
        .bind(plan.max_new_conns_per_sec)
        .bind(plan.max_tunnels)
        .bind(plan.allow_custom_port)
        .bind(plan.traffic_quota_bytes)
        .bind(&plan.traffic_period)
        .bind(None::<i32>)
        .bind(&plan.allowed_protocols)
        .bind(&plan.traffic_count_mode)
        .bind(starts_at)
        .bind(expires_at_override)
        .bind(period_anchor)
        .execute(&mut **transaction)
        .await
        .map_err(database_error)?;
        write_audit(
            transaction,
            actor_id,
            "subscription.assign",
            "subscription",
            subscription_id,
        )
        .await?;
        subscription_id
    };

    for group_id in group_ids {
        sqlx::query(
            "INSERT INTO subscription_node_groups (subscription_id, node_group_id) VALUES ($1, $2)",
        )
        .bind(subscription_id)
        .bind(group_id)
        .execute(&mut **transaction)
        .await
        .map_err(database_error)?;
    }
    let upgrading = existing_id.is_some();
    if upgrading {
        sqlx::query(
            "UPDATE orders SET status = 'credited' WHERE id = (SELECT id FROM orders WHERE user_id = $1 AND status = 'completed' ORDER BY created_at DESC LIMIT 1)",
        )
        .bind(user_id)
        .execute(&mut **transaction)
        .await
        .map_err(database_error)?;
    }
    let order_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO orders (id, user_id, plan_id, plan_name, kind, period, amount_cents, status, subscription_id) VALUES ($1,$2,$3,$4,$5,$6,$7,'completed',$8)",
    )
    .bind(order_id)
    .bind(user_id)
    .bind(plan_id)
    .bind(&plan.name)
    .bind(if upgrading { "upgrade" } else { "new" })
    .bind(&plan.traffic_period)
    .bind(amount_cents)
    .bind(subscription_id)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    write_audit(transaction, actor_id, "order.create", "order", order_id).await?;
    Ok(order_id)
}

async fn patch_active_subscription_expires(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor_id: Uuid,
    user_id: Uuid,
    expires_at: DateTime<Utc>,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    let subscription_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM user_subscriptions WHERE user_id = $1 AND status = 'active' FOR UPDATE",
    )
    .bind(user_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    let Some(subscription_id) = subscription_id else {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "用户没有可修改的有效订阅",
        )));
    };
    let starts_at: DateTime<Utc> = sqlx::query_scalar(
        "SELECT starts_at FROM user_subscriptions WHERE id = $1",
    )
    .bind(subscription_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(database_error)?;
    if expires_at <= starts_at {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "到期时间必须晚于开始时间",
        )));
    }
    sqlx::query(
        "UPDATE user_subscriptions SET expires_at = $2, updated_at = now() WHERE id = $1",
    )
    .bind(subscription_id)
    .bind(expires_at)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    write_audit(
        transaction,
        actor_id,
        "subscription.update",
        "subscription",
        subscription_id,
    )
    .await?;
    Ok(())
}

#[derive(Debug, Serialize, FromRow)]
struct UserSubscriptionRow {
    id: Uuid,
    plan_id: Option<Uuid>,
    status: String,
    speed_limit_mbps: i64,
    max_conns_per_tunnel: i32,
    max_new_conns_per_sec: i32,
    max_tunnels: i32,
    allow_custom_port: bool,
    traffic_quota_bytes: Option<i64>,
    traffic_period: String,
    duration_days: Option<i32>,
    allowed_protocols: Vec<String>,
    starts_at: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
}

async fn user_subscription(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Option<UserSubscriptionRow>>, (StatusCode, Json<serde_json::Value>)> {
    let (pg, account) = state
        .database_for(&headers, UserRole::User)
        .await
        .map_err(response_error)?;
    let subscription = sqlx::query_as::<_, UserSubscriptionRow>(
        "SELECT id, plan_id, status, speed_limit_mbps, max_conns_per_tunnel, max_new_conns_per_sec, max_tunnels, allow_custom_port, traffic_quota_bytes, traffic_period, duration_days, allowed_protocols, starts_at, expires_at FROM user_subscriptions WHERE user_id = $1 AND status = 'active' AND starts_at <= now() AND (expires_at IS NULL OR expires_at > now()) AND exhausted_period_start IS NULL ORDER BY starts_at DESC LIMIT 1",
    )
    .bind(account.user_id)
    .fetch_optional(&pg)
    .await
    .map_err(database_error)?;
    Ok(Json(subscription))
}

fn valid_email(email: &str) -> bool {
    let Some((local, domain)) = email.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.is_empty()
        && domain.contains('.')
        && email.len() <= 254
        && email.matches('@').count() == 1
        && !email.chars().any(char::is_whitespace)
}
