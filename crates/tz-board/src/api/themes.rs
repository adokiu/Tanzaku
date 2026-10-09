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
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

pub fn admin_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/admin/themes", get(list_themes))
        .route("/api/v1/admin/themes/upload", post(upload_theme))
        .route("/api/v1/admin/themes/active", post(set_active_theme))
        .route(
            "/api/v1/admin/themes/{short}/settings",
            get(get_theme_settings).post(update_theme_settings),
        )
        .route("/api/v1/admin/themes/{short}", delete(delete_theme))
}

#[derive(Serialize)]
struct ThemeRow {
    #[serde(flatten)]
    manifest: ThemeManifest,
    active: bool,
    /// 相对 TANZAKU_DATA 的路径，例如 `theme/Tanzaku`
    path: String,
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
    let (pg, _) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    let (page, page_size, offset) = page.resolve();
    let active = theme::active_theme_id(&pg).await;
    let rows: Vec<ThemeRow> = theme::list_installed_themes()
        .into_iter()
        .map(|manifest| {
            let path = theme::theme_storage_relpath(&manifest.short);
            ThemeRow {
                active: !active.is_empty() && manifest.short == active,
                path,
                manifest,
            }
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
    client_ip: crate::client_ip::ClientIp,
    mut multipart: Multipart,
) -> Result<Json<ThemeManifest>, (StatusCode, Json<Value>)> {
    let (pg, actor) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
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
                    .map_err(|_| {
                        response_error(unavailable(StatusCode::BAD_REQUEST, "读取上传文件失败"))
                    })?
                    .to_vec(),
            );
            break;
        }
    }
    let Some(bytes) = archive else {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "缺少主题压缩包 file 字段",
        )));
    };
    let manifest = theme::extract_theme_archive(&bytes)
        .map_err(|message| response_error(unavailable(StatusCode::BAD_REQUEST, &message)))?;
    sqlx::query(
        "INSERT INTO audit_logs (actor_id, action, target_type, target_id, details, remote_ip) VALUES ($1, 'theme.upload', 'theme', $2, $3, $4::inet)",
    )
    .bind(actor.user_id)
    .bind(&manifest.short)
    .bind(json!({ "name": manifest.name, "version": manifest.version, "path": theme::theme_storage_relpath(&manifest.short) }))
    .bind(client_ip.as_str())
    .execute(&pg)
    .await
    .map_err(database_error)?;
    Ok(Json(manifest))
}

async fn set_active_theme(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    client_ip: crate::client_ip::ClientIp,
    Query(query): Query<ActiveThemeQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (pg, actor) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    let short = query.theme.trim().to_owned();
    if short.is_empty() {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "主题名称不能为空",
        )));
    }
    theme::set_active_theme(&pg, &short)
        .await
        .map_err(|message| response_error(unavailable(StatusCode::BAD_REQUEST, &message)))?;
    sqlx::query(
        "INSERT INTO audit_logs (actor_id, action, target_type, target_id, details, remote_ip) VALUES ($1, 'theme.set_active', 'theme', $2, $3, $4::inet)",
    )
    .bind(actor.user_id)
    .bind(&short)
    .bind(json!({ "short": short }))
    .bind(client_ip.as_str())
    .execute(&pg)
    .await
    .map_err(database_error)?;
    Ok(Json(json!({ "theme": short })))
}

async fn get_theme_settings(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(short): Path<String>,
) -> Result<Json<theme::ThemeSettingsView>, (StatusCode, Json<Value>)> {
    let (pg, _) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    let view = theme::theme_settings_view(&pg, &short)
        .await
        .map_err(|message| response_error(unavailable(StatusCode::BAD_REQUEST, &message)))?;
    Ok(Json(view))
}

async fn update_theme_settings(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    client_ip: crate::client_ip::ClientIp,
    Path(short): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (pg, actor) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    theme::save_theme_settings(&pg, &short, body.clone())
        .await
        .map_err(|message| response_error(unavailable(StatusCode::BAD_REQUEST, &message)))?;
    sqlx::query(
        "INSERT INTO audit_logs (actor_id, action, target_type, target_id, details, remote_ip) VALUES ($1, 'theme.settings', 'theme', $2, $3, $4::inet)",
    )
    .bind(actor.user_id)
    .bind(&short)
    .bind(json!({ "short": short }))
    .bind(client_ip.as_str())
    .execute(&pg)
    .await
    .map_err(database_error)?;
    Ok(Json(json!({ "short": short, "settings": body })))
}

async fn delete_theme(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    client_ip: crate::client_ip::ClientIp,
    Path(short): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (pg, actor) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    theme::delete_installed_theme(&short)
        .map_err(|message| response_error(unavailable(StatusCode::BAD_REQUEST, &message)))?;
    let _ = theme::delete_theme_settings(&pg, &short).await;
    let active = theme::active_theme_id(&pg).await;
    if active == short {
        let next = theme::list_installed_themes()
            .into_iter()
            .next()
            .map(|item| item.short)
            .unwrap_or_default();
        theme::set_active_theme(&pg, &next).await.map_err(|message| {
            response_error(unavailable(StatusCode::INTERNAL_SERVER_ERROR, &message))
        })?;
    }
    sqlx::query(
        "INSERT INTO audit_logs (actor_id, action, target_type, target_id, details, remote_ip) VALUES ($1, 'theme.delete', 'theme', $2, $3, $4::inet)",
    )
    .bind(actor.user_id)
    .bind(&short)
    .bind(json!({ "short": short }))
    .bind(client_ip.as_str())
    .execute(&pg)
    .await
    .map_err(database_error)?;
    Ok(Json(json!({ "deleted": short })))
}
