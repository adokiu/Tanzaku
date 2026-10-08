use super::{database_error, response_error};
use crate::{
    setup::{AppState, UserRole, unavailable},
    theme::{self, ThemeManifest},
};
use axum::{
    Json, Router,
    extract::{Multipart, Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get, post},
};
use tower_http::limit::RequestBodyLimitLayer;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

pub fn admin_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/admin/themes", get(list_themes))
        .route("/api/v1/admin/themes/upload", post(upload_theme))
        .route("/api/v1/admin/themes/active", post(set_active_theme))
        .route("/api/v1/admin/themes/{short}", delete(delete_theme))
        .layer(RequestBodyLimitLayer::new(128 * 1024 * 1024))
}

#[derive(Serialize)]
struct ThemeRow {
    #[serde(flatten)]
    manifest: ThemeManifest,
    active: bool,
    embedded: bool,
}

#[derive(Deserialize)]
struct ActiveThemeQuery {
    theme: String,
}

async fn list_themes(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(page): Query<crate::page::PageQuery>,
) -> Result<Json<crate::page::Page<ThemeRow>>, (StatusCode, Json<Value>)> {
    let (pg, _) = state.database_for(&headers, UserRole::Admin).await.map_err(response_error)?;
    let (page, page_size, offset) = page.resolve();
    let active = theme::active_theme_id(&pg).await;
    let embedded_short = theme::load_embedded_manifest()
        .ok()
        .map(|manifest| manifest.short);
    let rows: Vec<ThemeRow> = theme::list_installed_themes()
        .into_iter()
        .map(|manifest| ThemeRow {
            active: manifest.short == active || (active == theme::DEFAULT_THEME_ID && embedded_short.as_deref() == Some(manifest.short.as_str())),
            embedded: embedded_short.as_deref() == Some(manifest.short.as_str()),
            manifest,
        })
        .collect();
    let total = rows.len() as i64;
    let start = offset as usize;
    let items = rows.into_iter().skip(start).take(page_size as usize).collect();
    Ok(Json(crate::page::Page::new(items, total, page, page_size)))
}

async fn upload_theme(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Json<ThemeManifest>, (StatusCode, Json<Value>)> {
    let (pg, actor) = state.database_for(&headers, UserRole::Admin).await.map_err(response_error)?;
    let mut archive = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| response_error(unavailable(StatusCode::BAD_REQUEST, "上传表单无效")))?
    {
        if field.name() == Some("file") || field.file_name().is_some() {
            archive = Some(
                field
                    .bytes()
                    .await
                    .map_err(|_| response_error(unavailable(StatusCode::BAD_REQUEST, "读取上传文件失败")))?
                    .to_vec(),
            );
            break;
        }
    }
    let Some(bytes) = archive else {
        return Err(response_error(unavailable(StatusCode::BAD_REQUEST, "缺少主题压缩包 file 字段")));
    };
    let manifest = theme::extract_theme_archive(&bytes)
        .map_err(|message| response_error(unavailable(StatusCode::BAD_REQUEST, &message)))?;
    sqlx::query("INSERT INTO audit_logs (actor_id, action, target_type, target_id, details) VALUES ($1, 'theme.upload', 'theme', $2, $3)")
        .bind(actor.user_id)
        .bind(&manifest.short)
        .bind(json!({ "name": manifest.name, "version": manifest.version }))
        .execute(&pg)
        .await
        .map_err(database_error)?;
    Ok(Json(manifest))
}

async fn set_active_theme(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<ActiveThemeQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (pg, actor) = state.database_for(&headers, UserRole::Admin).await.map_err(response_error)?;
    let short = query.theme.clone();
    if short.is_empty() {
        return Err(response_error(unavailable(StatusCode::BAD_REQUEST, "主题名称不能为空")));
    }
    theme::set_active_theme(&pg, &short)
        .await
        .map_err(|message| response_error(unavailable(StatusCode::BAD_REQUEST, &message)))?;
    sqlx::query("INSERT INTO audit_logs (actor_id, action, target_type, target_id, details) VALUES ($1, 'theme.set_active', 'theme', $2, $3)")
        .bind(actor.user_id)
        .bind(&short)
        .bind(json!({ "short": short }))
        .execute(&pg)
        .await
        .map_err(database_error)?;
    Ok(Json(json!({ "theme": short })))
}

async fn delete_theme(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(short): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (pg, actor) = state.database_for(&headers, UserRole::Admin).await.map_err(response_error)?;
    if theme::load_embedded_manifest()
        .ok()
        .is_some_and(|manifest| manifest.short == short)
    {
        return Err(response_error(unavailable(StatusCode::BAD_REQUEST, "内置默认主题不能删除")));
    }
    theme::delete_installed_theme(&short)
        .map_err(|message| response_error(unavailable(StatusCode::BAD_REQUEST, &message)))?;
    let active = theme::active_theme_id(&pg).await;
    if active == short {
        theme::set_active_theme(&pg, theme::DEFAULT_THEME_ID)
            .await
            .map_err(|message| response_error(unavailable(StatusCode::INTERNAL_SERVER_ERROR, &message)))?;
    }
    sqlx::query("INSERT INTO audit_logs (actor_id, action, target_type, target_id, details) VALUES ($1, 'theme.delete', 'theme', $2, $3)")
        .bind(actor.user_id)
        .bind(&short)
        .bind(json!({ "short": short }))
        .execute(&pg)
        .await
        .map_err(database_error)?;
    Ok(Json(json!({ "deleted": short })))
}
