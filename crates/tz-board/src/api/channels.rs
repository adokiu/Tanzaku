use super::{database_error, response_error, write_audit};
use crate::{
    payment::{self, PayError},
    setup::{AppState, UserRole, unavailable},
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get, post, put},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::FromRow;
use std::sync::Arc;
use uuid::Uuid;

pub fn admin_router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/v1/admin/payment-drivers",
            get(list_drivers),
        )
        .route(
            "/api/v1/admin/payment-channels",
            get(list_channels).post(create_channel),
        )
        .route(
            "/api/v1/admin/payment-channels/{id}",
            put(update_channel).delete(delete_channel),
        )
}

pub fn user_router() -> Router<Arc<AppState>> {
    Router::new().route("/api/v1/payment-channels", get(list_user_channels))
}

#[derive(Debug, Serialize)]
struct DriverRow {
    kind: &'static str,
    display_name: &'static str,
    config_fields: &'static [payment::ConfigField],
}

#[derive(Debug, Serialize, FromRow)]
struct ChannelRow {
    id: Uuid,
    name: String,
    kind: String,
    enabled: bool,
    sort_order: i32,
    config: Value,
}

#[derive(Debug, Serialize, FromRow)]
struct PublicChannelRow {
    id: Uuid,
    name: String,
    kind: String,
}

#[derive(Debug, Deserialize)]
struct ChannelBody {
    name: String,
    kind: String,
    enabled: bool,
    #[serde(default)]
    sort_order: i32,
    #[serde(default)]
    config: Value,
}

fn map_pay_error(error: PayError) -> (StatusCode, Json<Value>) {
    match error {
        PayError::Unavailable(message) => {
            response_error(unavailable(StatusCode::BAD_REQUEST, message))
        }
        PayError::Message(message) => response_error(unavailable(StatusCode::BAD_REQUEST, message)),
    }
}

async fn list_drivers(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Vec<DriverRow>>, (StatusCode, Json<Value>)> {
    let _ = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    Ok(Json(
        payment::registered_channels()
            .into_iter()
            .map(|driver| {
                let meta = driver.meta();
                DriverRow {
                    kind: meta.kind,
                    display_name: meta.display_name,
                    config_fields: meta.config_fields,
                }
            })
            .collect(),
    ))
}

async fn list_channels(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Vec<ChannelRow>>, (StatusCode, Json<Value>)> {
    let (pg, _) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    let items = sqlx::query_as::<_, ChannelRow>(
        "SELECT id, name, kind, enabled, sort_order, config FROM payment_channels ORDER BY sort_order, name",
    )
    .fetch_all(&pg)
    .await
    .map_err(database_error)?;
    Ok(Json(items))
}

async fn list_user_channels(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Vec<PublicChannelRow>>, (StatusCode, Json<Value>)> {
    let (pg, _) = state
        .database_for(&headers, UserRole::User)
        .await
        .map_err(response_error)?;
    let items = sqlx::query_as::<_, PublicChannelRow>(
        "SELECT id, name, kind FROM payment_channels WHERE enabled = TRUE ORDER BY sort_order, name",
    )
    .fetch_all(&pg)
    .await
    .map_err(database_error)?;
    Ok(Json(items))
}

async fn create_channel(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<ChannelBody>,
) -> Result<(StatusCode, Json<ChannelRow>), (StatusCode, Json<Value>)> {
    let (pg, actor) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    let name = request.name.trim();
    if name.is_empty() || name.len() > 80 {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "渠道名称长度必须为 1 到 80 个字符",
        )));
    }
    payment::validate_config(&request.kind, &request.config).map_err(map_pay_error)?;
    let mut transaction = pg.begin().await.map_err(database_error)?;
    let id = Uuid::new_v4();
    let row = sqlx::query_as::<_, ChannelRow>(
        "INSERT INTO payment_channels (id, name, kind, enabled, sort_order, config) VALUES ($1,$2,$3,$4,$5,$6) RETURNING id, name, kind, enabled, sort_order, config",
    )
    .bind(id)
    .bind(name)
    .bind(&request.kind)
    .bind(request.enabled)
    .bind(request.sort_order)
    .bind(&request.config)
    .fetch_one(&mut *transaction)
    .await
    .map_err(|_| response_error(unavailable(StatusCode::CONFLICT, "渠道名称已存在或数据库不可用")))?;
    write_audit(&mut transaction, actor.user_id, "payment_channel.create", "payment_channel", id).await?;
    transaction.commit().await.map_err(database_error)?;
    Ok((StatusCode::CREATED, Json(row)))
}

async fn update_channel(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<ChannelBody>,
) -> Result<Json<ChannelRow>, (StatusCode, Json<Value>)> {
    let (pg, actor) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    let name = request.name.trim();
    if name.is_empty() || name.len() > 80 {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "渠道名称长度必须为 1 到 80 个字符",
        )));
    }
    payment::validate_config(&request.kind, &request.config).map_err(map_pay_error)?;
    let mut transaction = pg.begin().await.map_err(database_error)?;
    let row = sqlx::query_as::<_, ChannelRow>(
        "UPDATE payment_channels SET name = $2, kind = $3, enabled = $4, sort_order = $5, config = $6, updated_at = now() WHERE id = $1 RETURNING id, name, kind, enabled, sort_order, config",
    )
    .bind(id)
    .bind(name)
    .bind(&request.kind)
    .bind(request.enabled)
    .bind(request.sort_order)
    .bind(&request.config)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| response_error(unavailable(StatusCode::CONFLICT, "渠道名称已存在或数据库不可用")))?
    .ok_or_else(|| response_error(unavailable(StatusCode::NOT_FOUND, "支付渠道不存在")))?;
    write_audit(&mut transaction, actor.user_id, "payment_channel.update", "payment_channel", id).await?;
    transaction.commit().await.map_err(database_error)?;
    Ok(Json(row))
}

async fn delete_channel(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, Json<Value>)> {
    let (pg, actor) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    let mut transaction = pg.begin().await.map_err(database_error)?;
    let deleted = sqlx::query("DELETE FROM payment_channels WHERE id = $1")
        .bind(id)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
    if deleted.rows_affected() == 0 {
        return Err(response_error(unavailable(StatusCode::NOT_FOUND, "支付渠道不存在")));
    }
    write_audit(&mut transaction, actor.user_id, "payment_channel.delete", "payment_channel", id).await?;
    transaction.commit().await.map_err(database_error)?;
    Ok(StatusCode::NO_CONTENT)
}
