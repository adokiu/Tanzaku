use super::{database_error, response_error, write_audit_with_ip};
use crate::{
    client_ip::ClientIp,
    guard_policy,
    page::{Page, PageQuery},
    setup::{self, AppState, UserRole, unavailable},
    store,
};
use argon2::PasswordVerifier;
use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::FromRow;
use std::sync::Arc;
use uuid::Uuid;

pub fn user_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/me/overview", get(me_overview))
        .route("/api/v1/me/ledger", get(me_ledger))
        .route("/api/v1/me/password", post(change_password))
        .route("/api/v1/me/email", post(change_email))
        .route("/api/v1/me/security/events", get(me_security_events))
        .route("/api/v1/me/security/policy", get(me_security_policy))
        .route("/api/v1/audit-logs", get(list_user_audit_logs))
}

#[derive(Debug, Serialize)]
struct MeOverview {
    subscription_plan_name: Option<String>,
    subscription_status: Option<String>,
    subscription_expires_at: Option<String>,
    traffic_used_bytes: i64,
    traffic_quota_bytes: Option<i64>,
    traffic_exhausted: bool,
    client_count: i64,
    tunnel_count: i64,
    balance_cents: i64,
}

#[derive(Debug, FromRow)]
struct OverviewRow {
    plan_name: Option<String>,
    subscription_status: Option<String>,
    expires_at: Option<String>,
    traffic_used_bytes: Option<i64>,
    traffic_quota_bytes: Option<i64>,
    traffic_exhausted: Option<bool>,
    balance_cents: i64,
}

async fn me_overview(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<MeOverview>, (StatusCode, Json<Value>)> {
    let (pg, user) = state
        .database_for(&headers, UserRole::User)
        .await
        .map_err(response_error)?;
    let row = sqlx::query_as::<_, OverviewRow>(
        r#"
        SELECT
            p.name AS plan_name,
            s.status AS subscription_status,
            s.expires_at::text AS expires_at,
            COALESCE(tu.traffic_used_bytes, 0) AS traffic_used_bytes,
            s.traffic_quota_bytes,
            (s.exhausted_period_start IS NOT NULL) AS traffic_exhausted,
            u.balance_cents
        FROM users u
        LEFT JOIN LATERAL (
            SELECT s.plan_id, s.status, s.traffic_quota_bytes, s.exhausted_period_start, s.expires_at, s.traffic_count_mode, s.id
            FROM user_subscriptions s
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
        ) s ON true
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
        WHERE u.id = $1
        "#,
    )
    .bind(user.user_id)
    .fetch_one(&pg)
    .await
    .map_err(database_error)?;
    let client_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM clients WHERE user_id = $1",
    )
    .bind(user.user_id)
    .fetch_one(&pg)
    .await
    .map_err(database_error)?;
    let tunnel_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM tunnels WHERE user_id = $1 AND status <> 'deleted'",
    )
    .bind(user.user_id)
    .fetch_one(&pg)
    .await
    .map_err(database_error)?;
    Ok(Json(MeOverview {
        subscription_plan_name: row.plan_name,
        subscription_status: row.subscription_status,
        subscription_expires_at: row.expires_at,
        traffic_used_bytes: row.traffic_used_bytes.unwrap_or(0),
        traffic_quota_bytes: row.traffic_quota_bytes,
        traffic_exhausted: row.traffic_exhausted.unwrap_or(false),
        client_count,
        tunnel_count,
        balance_cents: row.balance_cents,
    }))
}

#[derive(Debug, Serialize, FromRow)]
struct LedgerEntry {
    id: Uuid,
    kind: String,
    amount_cents: i64,
    title: String,
    created_at: String,
}

#[derive(Debug, Serialize)]
struct LedgerPage {
    balance_cents: i64,
    items: Vec<LedgerEntry>,
    total: i64,
    page: i64,
    page_size: i64,
}

async fn me_ledger(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(page): Query<PageQuery>,
) -> Result<Json<LedgerPage>, (StatusCode, Json<Value>)> {
    let (pg, user) = state
        .database_for(&headers, UserRole::User)
        .await
        .map_err(response_error)?;
    let (page, page_size, offset) = page.resolve();
    let balance_cents: i64 = sqlx::query_scalar("SELECT balance_cents FROM users WHERE id = $1")
        .bind(user.user_id)
        .fetch_one(&pg)
        .await
        .map_err(database_error)?;
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*)::bigint FROM orders WHERE user_id = $1 AND status = 'completed'")
        .bind(user.user_id)
        .fetch_one(&pg)
        .await
        .map_err(database_error)?;
    let items = sqlx::query_as::<_, LedgerEntry>(
        "SELECT id, 'order'::text AS kind, amount_cents, plan_name AS title, created_at::text AS created_at FROM orders WHERE user_id = $1 AND status = 'completed' ORDER BY created_at DESC, id DESC LIMIT $2 OFFSET $3",
    )
    .bind(user.user_id)
    .bind(page_size)
    .bind(offset)
    .fetch_all(&pg)
    .await
    .map_err(database_error)?;
    Ok(Json(LedgerPage {
        balance_cents,
        items,
        total,
        page,
        page_size,
    }))
}

#[derive(Debug, Serialize, FromRow)]
struct UserGuardEventRow {
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

async fn me_security_events(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(page): Query<PageQuery>,
) -> Result<Json<Page<UserGuardEventRow>>, (StatusCode, Json<Value>)> {
    let (pg, user) = state
        .database_for(&headers, UserRole::User)
        .await
        .map_err(response_error)?;
    let (page, page_size, offset) = page.resolve();
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM guard_events e JOIN tunnels t ON t.id = e.tunnel_id WHERE t.user_id = $1",
    )
    .bind(user.user_id)
    .fetch_one(&pg)
    .await
    .map_err(database_error)?;
    let items = sqlx::query_as::<_, UserGuardEventRow>(
        "SELECT e.id, e.node_id, n.name AS node_name, e.rule, e.tunnel_id,
                e.hit_count AS intensity,
                GREATEST(0, EXTRACT(EPOCH FROM (e.last_seen_at - e.first_seen_at))::bigint) AS duration_secs,
                e.first_seen_at::text AS first_seen_at, e.last_seen_at::text AS last_seen_at
         FROM guard_events e
         JOIN tunnels t ON t.id = e.tunnel_id
         JOIN nodes n ON n.id = e.node_id
         WHERE t.user_id = $1
         ORDER BY e.last_seen_at DESC LIMIT $2 OFFSET $3",
    )
    .bind(user.user_id)
    .bind(page_size)
    .bind(offset)
    .fetch_all(&pg)
    .await
    .map_err(database_error)?;
    Ok(Json(Page::new(items, total, page, page_size)))
}

#[derive(Debug, Serialize)]
struct UserSecurityPolicy {
    safe_mode: bool,
    guard_policy: Value,
    notes: String,
}

async fn me_security_policy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<UserSecurityPolicy>, (StatusCode, Json<Value>)> {
    let (pg, _) = state
        .database_for(&headers, UserRole::User)
        .await
        .map_err(response_error)?;
    let safe_mode = sqlx::query_scalar::<_, Value>(
        "SELECT value FROM system_settings WHERE key = 'security_safe_mode'",
    )
    .fetch_optional(&pg)
    .await
    .map_err(database_error)?
    .and_then(|value| value.as_bool())
    .unwrap_or(false);
    let guard_policy = guard_policy::load_global_guard_policy(&pg).await;
    Ok(Json(UserSecurityPolicy {
        safe_mode,
        guard_policy,
        notes: "以下为平台全局节点防护策略摘要，个人隧道仍受套餐与节点策略约束。".into(),
    }))
}

#[derive(Debug, Serialize, FromRow)]
struct UserAuditRow {
    id: i64,
    action: String,
    target_type: String,
    target_id: Option<String>,
    details: Value,
    remote_ip: Option<String>,
    created_at: String,
}

async fn list_user_audit_logs(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(page): Query<PageQuery>,
) -> Result<Json<Page<UserAuditRow>>, (StatusCode, Json<Value>)> {
    let (pg, user) = state
        .database_for(&headers, UserRole::User)
        .await
        .map_err(response_error)?;
    let (page, page_size, offset) = page.resolve();
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM audit_logs WHERE actor_id = $1",
    )
    .bind(user.user_id)
    .fetch_one(&pg)
    .await
    .map_err(database_error)?;
    let items = sqlx::query_as::<_, UserAuditRow>(
        "SELECT id, action, target_type, target_id, details, remote_ip::text AS remote_ip, created_at::text AS created_at FROM audit_logs WHERE actor_id = $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3",
    )
    .bind(user.user_id)
    .bind(page_size)
    .bind(offset)
    .fetch_all(&pg)
    .await
    .map_err(database_error)?;
    Ok(Json(Page::new(items, total, page, page_size)))
}

#[derive(Debug, Deserialize)]
struct ChangePasswordRequest {
    current_password: String,
    new_password: String,
}

#[derive(Debug, Deserialize)]
struct ChangeEmailRequest {
    current_password: String,
    new_email: String,
}

#[derive(Debug, Serialize)]
struct ChangeEmailResponse {
    email: String,
}

async fn change_password(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    client_ip: ClientIp,
    Json(request): Json<ChangePasswordRequest>,
) -> Result<StatusCode, (StatusCode, Json<Value>)> {
    let (pg, user) = state
        .database_for(&headers, UserRole::User)
        .await
        .map_err(response_error)?;
    if request.current_password.len() > 1024 || request.new_password.len() > 1024 {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "密码过长",
        )));
    }
    if let Err(message) = store::validate_password(&request.new_password) {
        return Err(response_error(unavailable(StatusCode::BAD_REQUEST, message)));
    }
    if request.current_password == request.new_password {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "新密码不能与当前密码相同",
        )));
    }
    let hash = sqlx::query_scalar::<_, String>("SELECT password_hash FROM users WHERE id = $1")
        .bind(user.user_id)
        .fetch_optional(&pg)
        .await
        .map_err(database_error)?
        .ok_or_else(|| {
            response_error(unavailable(
                StatusCode::UNAUTHORIZED,
                "请重新登录后重试",
            ))
        })?;
    let valid = argon2::PasswordHash::new(&hash).ok().is_some_and(|parsed| {
        argon2::Argon2::default()
            .verify_password(request.current_password.as_bytes(), &parsed)
            .is_ok()
    });
    if !valid {
        return Err(response_error(unavailable(
            StatusCode::UNAUTHORIZED,
            "当前密码不正确",
        )));
    }
    let password_hash = store::hash_password(&request.new_password).map_err(|_| {
        response_error(unavailable(
            StatusCode::INTERNAL_SERVER_ERROR,
            "密码处理失败",
        ))
    })?;
    let mut transaction = pg.begin().await.map_err(database_error)?;
    sqlx::query("UPDATE users SET password_hash = $2, updated_at = now() WHERE id = $1")
        .bind(user.user_id)
        .bind(&password_hash)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
    write_audit_with_ip(
        &mut transaction,
        user.user_id,
        "user.password.change",
        "user",
        user.user_id,
        Some(client_ip.0),
    )
    .await?;
    transaction.commit().await.map_err(database_error)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn change_email(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    client_ip: ClientIp,
    Json(request): Json<ChangeEmailRequest>,
) -> Result<Json<ChangeEmailResponse>, (StatusCode, Json<Value>)> {
    let (pg, user) = state
        .database_for(&headers, UserRole::User)
        .await
        .map_err(response_error)?;
    if request.current_password.len() > 1024 {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "密码过长",
        )));
    }
    let new_email = request.new_email.trim().to_lowercase();
    if !valid_email(&new_email) {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "请输入有效的邮箱",
        )));
    }
    let hash = sqlx::query_scalar::<_, String>(
        "SELECT password_hash FROM users WHERE id = $1",
    )
    .bind(user.user_id)
    .fetch_optional(&pg)
    .await
    .map_err(database_error)?
    .ok_or_else(|| {
        response_error(unavailable(
            StatusCode::UNAUTHORIZED,
            "请重新登录后重试",
        ))
    })?;
    let valid = argon2::PasswordHash::new(&hash).ok().is_some_and(|parsed| {
        argon2::Argon2::default()
            .verify_password(request.current_password.as_bytes(), &parsed)
            .is_ok()
    });
    if !valid {
        return Err(response_error(unavailable(
            StatusCode::UNAUTHORIZED,
            "当前密码不正确",
        )));
    }
    let current_email =
        sqlx::query_scalar::<_, String>("SELECT email FROM users WHERE id = $1")
            .bind(user.user_id)
            .fetch_one(&pg)
            .await
            .map_err(database_error)?;
    if current_email.eq_ignore_ascii_case(&new_email) {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "新邮箱不能与当前邮箱相同",
        )));
    }
    let mut transaction = pg.begin().await.map_err(database_error)?;
    let updated = sqlx::query(
        "UPDATE users SET email = $2, updated_at = now() WHERE id = $1 AND email <> $2",
    )
    .bind(user.user_id)
    .bind(&new_email)
    .execute(&mut *transaction)
    .await
    .map_err(|_| {
        response_error(unavailable(
            StatusCode::CONFLICT,
            "该邮箱已被使用",
        ))
    })?;
    if updated.rows_affected() == 0 {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "新邮箱不能与当前邮箱相同",
        )));
    }
    write_audit_with_ip(
        &mut transaction,
        user.user_id,
        "user.email.change",
        "user",
        user.user_id,
        Some(client_ip.0),
    )
    .await?;
    transaction.commit().await.map_err(database_error)?;
    let _: Result<(), _> =
        setup::update_session_email(&state, &headers, UserRole::User, &new_email).await;
    Ok(Json(ChangeEmailResponse { email: new_email }))
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
