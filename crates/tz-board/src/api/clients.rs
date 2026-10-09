use super::{AgentTokenView, database_error, new_agent_token, response_error, write_audit};
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
        .route(
            "/api/v1/admin/clients",
            get(list_all_clients).post(admin_create_client),
        )
        .route(
            "/api/v1/admin/clients/{client_id}/enabled",
            patch(admin_set_client_enabled),
        )
        .route(
            "/api/v1/admin/clients/{client_id}",
            put(admin_update_client).delete(admin_delete_client),
        )
        .route(
            "/api/v1/admin/clients/{client_id}/token",
            get(admin_get_client_token).post(admin_reset_client_token),
        )
}

pub fn user_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/clients", get(list_clients).post(create_client))
        .route(
            "/api/v1/clients/{client_id}/enabled",
            patch(user_set_client_enabled),
        )
        .route(
            "/api/v1/clients/{client_id}",
            put(user_update_client).delete(user_delete_client),
        )
        .route(
            "/api/v1/clients/{client_id}/token",
            get(user_get_client_token).post(user_reset_client_token),
        )
}

async fn admin_get_client_token(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
) -> Result<Json<AgentTokenView>, (StatusCode, Json<serde_json::Value>)> {
    let (pg, _) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    get_client_token(&pg, client_id, None).await
}

async fn user_get_client_token(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
) -> Result<Json<AgentTokenView>, (StatusCode, Json<serde_json::Value>)> {
    let (pg, account) = state
        .database_for(&headers, UserRole::User)
        .await
        .map_err(response_error)?;
    get_client_token(&pg, client_id, Some(account.user_id)).await
}

async fn admin_reset_client_token(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
) -> Result<Json<AgentTokenView>, (StatusCode, Json<serde_json::Value>)> {
    let (pg, actor) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    reset_client_token(&pg, actor.user_id, client_id, None).await
}

async fn user_reset_client_token(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
) -> Result<Json<AgentTokenView>, (StatusCode, Json<serde_json::Value>)> {
    let (pg, account) = state
        .database_for(&headers, UserRole::User)
        .await
        .map_err(response_error)?;
    reset_client_token(&pg, account.user_id, client_id, Some(account.user_id)).await
}

async fn get_client_token(
    pg: &sqlx::PgPool,
    client_id: Uuid,
    owner_user_id: Option<Uuid>,
) -> Result<Json<AgentTokenView>, (StatusCode, Json<serde_json::Value>)> {
    let row: Option<(Option<String>, String)> = if let Some(user_id) = owner_user_id {
        sqlx::query_as("SELECT token, token_prefix FROM clients WHERE id = $1 AND user_id = $2")
            .bind(client_id)
            .bind(user_id)
            .fetch_optional(pg)
            .await
    } else {
        sqlx::query_as("SELECT token, token_prefix FROM clients WHERE id = $1")
            .bind(client_id)
            .fetch_optional(pg)
            .await
    }
    .map_err(database_error)?;
    let Some((token, token_prefix)) = row else {
        return Err(response_error(unavailable(StatusCode::NOT_FOUND, "Client 不存在")));
    };
    Ok(Json(AgentTokenView { token, token_prefix }))
}

async fn reset_client_token(
    pg: &sqlx::PgPool,
    actor_id: Uuid,
    client_id: Uuid,
    owner_user_id: Option<Uuid>,
) -> Result<Json<AgentTokenView>, (StatusCode, Json<serde_json::Value>)> {
    let (token, token_hash, token_prefix) = new_agent_token("无法生成 Client 凭据")?;
    let mut transaction = pg.begin().await.map_err(database_error)?;
    let updated = if let Some(user_id) = owner_user_id {
        sqlx::query(
            "UPDATE clients SET token = $2, token_hash = $3, token_prefix = $4, updated_at = now() WHERE id = $1 AND user_id = $5",
        )
        .bind(client_id)
        .bind(&token)
        .bind(token_hash)
        .bind(&token_prefix)
        .bind(user_id)
        .execute(&mut *transaction)
        .await
    } else {
        sqlx::query(
            "UPDATE clients SET token = $2, token_hash = $3, token_prefix = $4, updated_at = now() WHERE id = $1",
        )
        .bind(client_id)
        .bind(&token)
        .bind(token_hash)
        .bind(&token_prefix)
        .execute(&mut *transaction)
        .await
    }
    .map_err(database_error)?;
    if updated.rows_affected() == 0 {
        return Err(response_error(unavailable(StatusCode::NOT_FOUND, "Client 不存在")));
    }
    write_audit(&mut transaction, actor_id, "client.token_reset", "client", client_id).await?;
    transaction.commit().await.map_err(database_error)?;
    Ok(Json(AgentTokenView {
        token: Some(token),
        token_prefix,
    }))
}

#[derive(Debug, Serialize)]
struct ClientTrafficMetrics {
    bytes_in: u64,
    bytes_out: u64,
    in_bps: u64,
    out_bps: u64,
}

#[derive(Debug, Serialize, FromRow)]
struct ClientListRow {
    id: Uuid,
    user_id: Uuid,
    user_email: Option<String>,
    name: String,
    enabled: bool,
    online: bool,
    version: Option<String>,
    os: Option<String>,
    arch: Option<String>,
    public_ip: String,
    region: String,
    last_seen_at: Option<String>,
    tunnel_count: i64,
}

#[derive(Debug, Serialize)]
struct ClientListItem {
    #[serde(flatten)]
    client: ClientListRow,
    #[serde(skip_serializing_if = "Option::is_none")]
    traffic_metrics: Option<ClientTrafficMetrics>,
}

#[derive(Deserialize)]
struct CreateClient {
    name: String,
}

#[derive(Deserialize)]
struct AdminCreateClient {
    name: String,
}

#[derive(Deserialize)]
struct SetClientEnabled {
    enabled: bool,
}

#[derive(Deserialize)]
struct UpdateClient {
    name: String,
}

#[derive(Serialize)]
struct CreatedClient {
    id: Uuid,
    name: String,
    token: String,
    token_prefix: String,
}

const CLIENT_LIST_SQL: &str = "SELECT c.id, c.user_id, u.email AS user_email, c.name, c.enabled, c.online, c.version, c.os, c.arch, c.public_ip, c.region, c.last_seen_at::text AS last_seen_at, (SELECT COUNT(*)::bigint FROM tunnels t WHERE t.client_id = c.id AND t.status <> 'deleted') AS tunnel_count FROM clients c JOIN users u ON u.id = c.user_id";

const CLIENT_LIST_USER_SQL: &str = "SELECT c.id, c.user_id, NULL::text AS user_email, c.name, c.enabled, c.online, c.version, c.os, c.arch, c.public_ip, c.region, c.last_seen_at::text AS last_seen_at, (SELECT COUNT(*)::bigint FROM tunnels t WHERE t.client_id = c.id AND t.status <> 'deleted') AS tunnel_count FROM clients c";

fn traffic_metrics_for_client(client_id: Uuid, online: bool) -> Option<ClientTrafficMetrics> {
    if !online {
        return None;
    }
    crate::client_traffic::snapshot(client_id).map(|metrics| ClientTrafficMetrics {
        bytes_in: metrics.bytes_in,
        bytes_out: metrics.bytes_out,
        in_bps: metrics.in_bps,
        out_bps: metrics.out_bps,
    })
}

async fn list_all_clients(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(page): Query<PageQuery>,
) -> Result<Json<Page<ClientListItem>>, (StatusCode, Json<serde_json::Value>)> {
    let (pg, _) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    let (page, page_size, offset) = page.resolve();
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*)::bigint FROM clients")
        .fetch_one(&pg)
        .await
        .map_err(database_error)?;
    let rows = sqlx::query_as::<_, ClientListRow>(&format!(
        "{CLIENT_LIST_SQL} ORDER BY c.created_at DESC LIMIT $1 OFFSET $2"
    ))
    .bind(page_size)
    .bind(offset)
    .fetch_all(&pg)
    .await
    .map_err(database_error)?;
    Ok(Json(Page::new(
        rows.into_iter()
            .map(|client| ClientListItem {
                traffic_metrics: traffic_metrics_for_client(client.id, client.online),
                client,
            })
            .collect(),
        total,
        page,
        page_size,
    )))
}

async fn list_clients(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(page): Query<PageQuery>,
) -> Result<Json<Page<ClientListItem>>, (StatusCode, Json<serde_json::Value>)> {
    let (pg, account) = state
        .database_for(&headers, UserRole::User)
        .await
        .map_err(response_error)?;
    let (page, page_size, offset) = page.resolve();
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*)::bigint FROM clients WHERE user_id = $1")
        .bind(account.user_id)
        .fetch_one(&pg)
        .await
        .map_err(database_error)?;
    let rows = sqlx::query_as::<_, ClientListRow>(&format!(
        "{CLIENT_LIST_USER_SQL} WHERE c.user_id = $1 ORDER BY c.created_at DESC LIMIT $2 OFFSET $3"
    ))
    .bind(account.user_id)
    .bind(page_size)
    .bind(offset)
    .fetch_all(&pg)
    .await
    .map_err(database_error)?;
    Ok(Json(Page::new(
        rows.into_iter()
            .map(|client| ClientListItem {
                traffic_metrics: traffic_metrics_for_client(client.id, client.online),
                client,
            })
            .collect(),
        total,
        page,
        page_size,
    )))
}

async fn admin_create_client(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<AdminCreateClient>,
) -> Result<(StatusCode, Json<CreatedClient>), (StatusCode, Json<serde_json::Value>)> {
    let (pg, actor) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    insert_client(&pg, actor.user_id, actor.user_id, &request.name).await
}

async fn create_client(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<CreateClient>,
) -> Result<(StatusCode, Json<CreatedClient>), (StatusCode, Json<serde_json::Value>)> {
    let (pg, account) = state
        .database_for(&headers, UserRole::User)
        .await
        .map_err(response_error)?;
    insert_client(&pg, account.user_id, account.user_id, &request.name).await
}

async fn admin_set_client_enabled(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Json(body): Json<SetClientEnabled>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let (pg, actor) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    set_client_enabled(&pg, actor.user_id, client_id, None, body.enabled).await
}

async fn user_set_client_enabled(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Json(body): Json<SetClientEnabled>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let (pg, account) = state
        .database_for(&headers, UserRole::User)
        .await
        .map_err(response_error)?;
    set_client_enabled(&pg, account.user_id, client_id, Some(account.user_id), body.enabled).await
}

async fn admin_update_client(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Json(request): Json<UpdateClient>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let (pg, actor) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    update_client_name(&pg, actor.user_id, client_id, None, &request.name).await
}

async fn user_update_client(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Json(request): Json<UpdateClient>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let (pg, account) = state
        .database_for(&headers, UserRole::User)
        .await
        .map_err(response_error)?;
    update_client_name(&pg, account.user_id, client_id, Some(account.user_id), &request.name).await
}

async fn update_client_name(
    pg: &sqlx::PgPool,
    actor_id: Uuid,
    client_id: Uuid,
    owner_user_id: Option<Uuid>,
    raw_name: &str,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let name = raw_name.trim();
    if name.is_empty() || name.len() > 100 {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "Client 名称长度必须为 1 到 100 个字符",
        )));
    }
    let mut transaction = pg.begin().await.map_err(database_error)?;
    let updated = if let Some(user_id) = owner_user_id {
        sqlx::query(
            "UPDATE clients SET name = $2, updated_at = now() WHERE id = $1 AND user_id = $3",
        )
        .bind(client_id)
        .bind(name)
        .bind(user_id)
        .execute(&mut *transaction)
        .await
    } else {
        sqlx::query("UPDATE clients SET name = $2, updated_at = now() WHERE id = $1")
            .bind(client_id)
            .bind(name)
            .execute(&mut *transaction)
            .await
    }
    .map_err(|_| {
        response_error(unavailable(
            StatusCode::CONFLICT,
            "Client 名称已存在或数据库不可用",
        ))
    })?;
    if updated.rows_affected() == 0 {
        return Err(response_error(unavailable(
            StatusCode::NOT_FOUND,
            "Client 不存在",
        )));
    }
    write_audit(
        &mut transaction,
        actor_id,
        "client.update",
        "client",
        client_id,
    )
    .await?;
    transaction.commit().await.map_err(database_error)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn admin_delete_client(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let (pg, actor) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    delete_client(&pg, actor.user_id, client_id, None).await
}

async fn user_delete_client(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let (pg, account) = state
        .database_for(&headers, UserRole::User)
        .await
        .map_err(response_error)?;
    delete_client(&pg, account.user_id, client_id, Some(account.user_id)).await
}

async fn set_client_enabled(
    pg: &sqlx::PgPool,
    actor_id: Uuid,
    client_id: Uuid,
    owner_user_id: Option<Uuid>,
    enabled: bool,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let mut transaction = pg.begin().await.map_err(database_error)?;
    let updated = if let Some(user_id) = owner_user_id {
        sqlx::query("UPDATE clients SET enabled = $2, updated_at = now() WHERE id = $1 AND user_id = $3")
            .bind(client_id)
            .bind(enabled)
            .bind(user_id)
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?
    } else {
        sqlx::query("UPDATE clients SET enabled = $2, updated_at = now() WHERE id = $1")
            .bind(client_id)
            .bind(enabled)
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?
    };
    if updated.rows_affected() == 0 {
        return Err(response_error(unavailable(
            StatusCode::NOT_FOUND,
            "Client 不存在",
        )));
    }
    write_audit(
        &mut transaction,
        actor_id,
        if enabled {
            "client.enable"
        } else {
            "client.disable"
        },
        "client",
        client_id,
    )
    .await?;
    transaction.commit().await.map_err(database_error)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_client(
    pg: &sqlx::PgPool,
    actor_id: Uuid,
    client_id: Uuid,
    owner_user_id: Option<Uuid>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let tunnel_count: i64 = if let Some(user_id) = owner_user_id {
        sqlx::query_scalar(
            "SELECT COUNT(*)::bigint FROM tunnels WHERE client_id = $1 AND user_id = $2 AND status <> 'deleted'",
        )
        .bind(client_id)
        .bind(user_id)
        .fetch_one(pg)
        .await
        .map_err(database_error)?
    } else {
        sqlx::query_scalar(
            "SELECT COUNT(*)::bigint FROM tunnels WHERE client_id = $1 AND status <> 'deleted'",
        )
        .bind(client_id)
        .fetch_one(pg)
        .await
        .map_err(database_error)?
    };
    if tunnel_count > 0 {
        return Err(response_error(unavailable(
            StatusCode::CONFLICT,
            "Client 上仍有隧道，无法删除",
        )));
    }
    let mut transaction = pg.begin().await.map_err(database_error)?;
    let deleted = if let Some(user_id) = owner_user_id {
        sqlx::query("DELETE FROM clients WHERE id = $1 AND user_id = $2")
            .bind(client_id)
            .bind(user_id)
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?
    } else {
        sqlx::query("DELETE FROM clients WHERE id = $1")
            .bind(client_id)
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?
    };
    if deleted.rows_affected() == 0 {
        return Err(response_error(unavailable(
            StatusCode::NOT_FOUND,
            "Client 不存在",
        )));
    }
    write_audit(
        &mut transaction,
        actor_id,
        "client.delete",
        "client",
        client_id,
    )
    .await?;
    transaction.commit().await.map_err(database_error)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn insert_client(
    pg: &sqlx::PgPool,
    actor_id: Uuid,
    owner_user_id: Uuid,
    raw_name: &str,
) -> Result<(StatusCode, Json<CreatedClient>), (StatusCode, Json<serde_json::Value>)> {
    let name = raw_name.trim();
    if name.is_empty() || name.len() > 100 {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "Client 名称长度必须为 1 到 100 个字符",
        )));
    }
    let (token, token_hash, token_prefix) = new_agent_token("无法生成 Client 凭据")?;
    let mut transaction = pg.begin().await.map_err(database_error)?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO clients (id, user_id, name, token_hash, token_prefix, token) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(owner_user_id)
    .bind(name)
    .bind(token_hash)
    .bind(&token_prefix)
    .bind(&token)
    .execute(&mut *transaction)
    .await
    .map_err(|_| response_error(unavailable(StatusCode::CONFLICT, "Client 名称已存在或数据库不可用")))?;
    write_audit(
        &mut transaction,
        actor_id,
        "client.create",
        "client",
        id,
    )
    .await?;
    transaction.commit().await.map_err(database_error)?;
    Ok((
        StatusCode::CREATED,
        Json(CreatedClient {
            id,
            name: name.to_owned(),
            token,
            token_prefix,
        }),
    ))
}
