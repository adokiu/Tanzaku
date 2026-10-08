use super::{database_error, response_error, write_audit};
use crate::setup::{AppState, UserRole, unavailable};
use crate::page::{Page, PageQuery};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::FromRow;
use std::{net::IpAddr, str::FromStr, sync::Arc};
use uuid::Uuid;

pub fn admin_router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/v1/admin/domain-whitelist",
            get(list_whitelist).post(create_whitelist),
        )
        .route(
            "/api/v1/admin/domain-whitelist/{id}",
            delete(delete_whitelist),
        )
}

#[derive(Debug, Serialize, FromRow)]
struct WhitelistRow {
    id: Uuid,
    domain: String,
    note: String,
    created_at: String,
}

#[derive(Deserialize)]
struct CreateWhitelist {
    domain: String,
    #[serde(default)]
    note: String,
}

pub(crate) fn domain_covers(whitelist: &[String], domain: &str) -> bool {
    let name = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    whitelist.iter().any(|entry| {
        let entry = entry
            .trim()
            .trim_end_matches('.')
            .trim_start_matches("*.")
            .to_ascii_lowercase();
        !entry.is_empty() && (name == entry || name.ends_with(&format!(".{entry}")))
    })
}

fn normalize_whitelist_domain(raw: &str) -> Result<String, (StatusCode, Json<Value>)> {
    let domain = raw
        .trim()
        .trim_end_matches('.')
        .trim_start_matches("*.")
        .trim_start_matches('.')
        .to_ascii_lowercase();
    if domain.is_empty()
        || domain.len() > 253
        || domain.contains('/')
        || domain.contains(':')
        || domain.contains(' ')
        || IpAddr::from_str(&domain).is_ok()
        || !domain.contains('.')
        || domain.split('.').any(|label| label.is_empty() || label.len() > 63)
    {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "域名格式无效",
        )));
    }
    Ok(domain)
}

async fn list_whitelist(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(page): Query<PageQuery>,
) -> Result<Json<Page<WhitelistRow>>, (StatusCode, Json<Value>)> {
    let (pg, _) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    let (page, page_size, offset) = page.resolve();
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*)::bigint FROM domain_whitelist")
        .fetch_one(&pg)
        .await
        .map_err(database_error)?;
    let rows = sqlx::query_as::<_, WhitelistRow>(
        "SELECT id, domain, note, created_at::text AS created_at FROM domain_whitelist ORDER BY domain LIMIT $1 OFFSET $2",
    )
    .bind(page_size)
    .bind(offset)
    .fetch_all(&pg)
    .await
    .map_err(database_error)?;
    Ok(Json(Page::new(rows, total, page, page_size)))
}

async fn create_whitelist(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<CreateWhitelist>,
) -> Result<Json<WhitelistRow>, (StatusCode, Json<Value>)> {
    let (pg, actor) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    let domain = normalize_whitelist_domain(&body.domain)?;
    let note = body.note.trim();
    if note.len() > 200 {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "备注过长",
        )));
    }
    let id = Uuid::new_v4();
    let mut transaction = pg.begin().await.map_err(database_error)?;
    let inserted = sqlx::query_as::<_, WhitelistRow>(
        "INSERT INTO domain_whitelist (id, domain, note) VALUES ($1, $2, $3) ON CONFLICT (domain) DO NOTHING RETURNING id, domain, note, created_at::text AS created_at",
    )
    .bind(id)
    .bind(&domain)
    .bind(note)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(database_error)?;
    let Some(row) = inserted else {
        return Err(response_error(unavailable(
            StatusCode::CONFLICT,
            "该域名已在过白名单中",
        )));
    };
    write_audit(
        &mut transaction,
        actor.user_id,
        "domain_whitelist.create",
        "domain_whitelist",
        row.id,
    )
    .await?;
    transaction.commit().await.map_err(database_error)?;
    crate::ws::push_node_configs_to_online_nodes(&pg).await;
    Ok(Json(row))
}

async fn delete_whitelist(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, Json<Value>)> {
    let (pg, actor) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    let mut transaction = pg.begin().await.map_err(database_error)?;
    let deleted = sqlx::query("DELETE FROM domain_whitelist WHERE id = $1")
        .bind(id)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
    if deleted.rows_affected() == 0 {
        return Err(response_error(unavailable(
            StatusCode::NOT_FOUND,
            "过白记录不存在",
        )));
    }
    write_audit(
        &mut transaction,
        actor.user_id,
        "domain_whitelist.delete",
        "domain_whitelist",
        id,
    )
    .await?;
    transaction.commit().await.map_err(database_error)?;
    crate::ws::push_node_configs_to_online_nodes(&pg).await;
    Ok(StatusCode::NO_CONTENT)
}
