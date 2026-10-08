use axum::{
    body::Body,
    extract::Path,
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::{
    env,
    fs,
    io::{Read, Write},
    path::{Component, Path as StdPath, PathBuf},
    sync::OnceLock,
};
use tracing::warn;

pub const DEFAULT_THEME_ID: &str = "default";
pub const USER_THEME_SETTING_KEY: &str = "user_theme";
const DIST_DIR: &str = "dist";
const INDEX_FILE: &str = "index.html";
const THEME_MANIFEST: &str = "komari-theme.json";

#[derive(RustEmbed)]
#[folder = "../../web-user/"]
struct DefaultUserThemeEmbed;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeManifest {
    pub name: String,
    pub short: String,
    pub description: Option<String>,
    pub version: String,
    pub author: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub preview: Option<String>,
}

pub fn data_dir() -> PathBuf {
    static DATA: OnceLock<PathBuf> = OnceLock::new();
    DATA.get_or_init(|| {
        env::var("TANZAKU_DATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("./data"))
    })
    .clone()
}

pub fn themes_dir() -> PathBuf {
    let dir = data_dir().join("theme");
    if let Err(error) = fs::create_dir_all(&dir) {
        warn!(?error, "failed to create theme directory");
    }
    dir
}

pub fn is_valid_theme_short(short: &str) -> bool {
    if short.is_empty() || short == DEFAULT_THEME_ID {
        return false;
    }
    short.chars().all(|ch| {
        ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'
    })
}

pub async fn active_theme_id(pg: &PgPool) -> String {
    let row = sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT value FROM system_settings WHERE key = $1",
    )
    .bind(USER_THEME_SETTING_KEY)
    .fetch_optional(pg)
    .await
    .ok()
    .flatten();
    match row {
        Some(value) if value.is_string() => value.as_str().unwrap_or(DEFAULT_THEME_ID).to_string(),
        Some(value) if value.get("short").and_then(|item| item.as_str()).is_some() => {
            value["short"].as_str().unwrap().to_string()
        }
        _ => DEFAULT_THEME_ID.to_string(),
    }
}

pub fn list_installed_themes() -> Vec<ThemeManifest> {
    let mut themes = Vec::new();
    if let Ok(manifest) = load_embedded_manifest() {
        themes.push(manifest);
    }
    let Ok(entries) = fs::read_dir(themes_dir()) else {
        return themes;
    };
    for entry in entries.flatten() {
        if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            continue;
        }
        let manifest_path = entry.path().join(THEME_MANIFEST);
        if let Ok(manifest) = read_manifest_file(&manifest_path) {
            if !themes.iter().any(|item| item.short == manifest.short) {
                themes.push(manifest);
            }
        }
    }
    themes
}

pub fn load_embedded_manifest() -> Result<ThemeManifest, String> {
    let raw = DefaultUserThemeEmbed::get(THEME_MANIFEST)
        .ok_or_else(|| "embedded default theme manifest missing".to_string())?;
    parse_manifest(&raw.data)
}

fn read_manifest_file(path: &StdPath) -> Result<ThemeManifest, String> {
    let raw = fs::read(path).map_err(|error| error.to_string())?;
    parse_manifest(&raw)
}

fn parse_manifest(raw: &[u8]) -> Result<ThemeManifest, String> {
    serde_json::from_slice(raw).map_err(|error| error.to_string())
}

pub async fn theme_asset(Path((theme_id, rest)): Path<(String, String)>) -> Response {
    if !is_public_theme_id(&theme_id) {
        return not_found();
    }
    let relative = rest.trim_start_matches('/');
    if relative.is_empty() {
        return not_found();
    }
    serve_theme_relative(&theme_id, relative).await
}

pub async fn serve_user_portal(requested: &str, active_theme: &str) -> Response {
    if requested.starts_with("/api/") {
        return service_unavailable("接口不存在", StatusCode::NOT_FOUND);
    }
    let path = requested.trim_start_matches('/');
    if path.is_empty() {
        return serve_theme_relative(active_theme, &format!("{DIST_DIR}/{INDEX_FILE}")).await;
    }
    let dist_relative = format!("{DIST_DIR}/{path}");
    if let Some(response) = try_theme_relative(active_theme, &dist_relative) {
        return response;
    }
    if path.contains('.') {
        return not_found();
    }
    serve_theme_relative(active_theme, &format!("{DIST_DIR}/{INDEX_FILE}")).await
}

async fn serve_theme_relative(theme_id: &str, relative: &str) -> Response {
    try_theme_relative(theme_id, relative).unwrap_or_else(not_found)
}

fn try_theme_relative(theme_id: &str, relative: &str) -> Option<Response> {
    let clean = sanitize_relative_path(relative)?;
    let (content, content_type) = read_theme_file(theme_id, &clean)?;
    Some(static_response(&content_type, content))
}

fn is_public_theme_id(theme_id: &str) -> bool {
    theme_id == DEFAULT_THEME_ID || is_valid_theme_short(theme_id)
}

fn sanitize_relative_path(relative: &str) -> Option<String> {
    let path = StdPath::new(relative);
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return None;
    }
    Some(path.to_string_lossy().replace('\\', "/"))
}

fn read_theme_file(theme_id: &str, relative: &str) -> Option<(Vec<u8>, &'static str)> {
    if theme_id != DEFAULT_THEME_ID {
        if !is_valid_theme_short(theme_id) {
            return None;
        }
        let base = themes_dir().join(theme_id);
        let local = base.join(relative);
        if !is_safe_path(&base, &local) {
            return None;
        }
        if let Ok(meta) = fs::metadata(&local) {
            if meta.is_file() {
                if let Ok(content) = fs::read(&local) {
                    return Some((content, content_type_for_path(relative)));
                }
            }
        }
    }
    let embed_path = relative.replace('\\', "/");
    if embed_path.contains("..") {
        return None;
    }
    DefaultUserThemeEmbed::get(&embed_path).map(|file| {
        (
            file.data.into_owned(),
            content_type_for_path(relative),
        )
    })
}

fn is_safe_path(base: &StdPath, target: &StdPath) -> bool {
    let Ok(abs_base) = fs::canonicalize(base) else {
        let clean_base = base.components().collect::<PathBuf>();
        let clean_target = target.components().collect::<PathBuf>();
        return clean_target.starts_with(&clean_base);
    };
    let Ok(abs_target) = fs::canonicalize(target) else {
        return false;
    };
    abs_target.starts_with(abs_base)
}

fn content_type_for_path(relative: &str) -> &'static str {
    match relative.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("json") => "application/json",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

pub fn extract_theme_archive(zip_bytes: &[u8]) -> Result<ThemeManifest, String> {
    let reader = std::io::Cursor::new(zip_bytes);
    let mut archive =
        zip::ZipArchive::new(reader).map_err(|error| format!("无法打开主题压缩包: {error}"))?;
    let mut manifest: Option<ThemeManifest> = None;
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|error| format!("读取压缩包条目失败: {error}"))?;
        if file.name().contains("..") {
            continue;
        }
        let mut content = Vec::new();
        file.read_to_end(&mut content)
            .map_err(|error| format!("读取压缩包内容失败: {error}"))?;
        if file.name() == THEME_MANIFEST {
            manifest = Some(parse_manifest(&content)?);
        }
        files.push((file.name().to_string(), content));
    }
    let manifest = manifest.ok_or_else(|| "缺少 komari-theme.json".to_string())?;
    if !is_valid_theme_short(&manifest.short) {
        return Err("主题 short 名称无效".into());
    }
    let theme_dir = themes_dir().join(&manifest.short);
    if theme_dir.exists() {
        fs::remove_dir_all(&theme_dir).map_err(|error| error.to_string())?;
    }
    fs::create_dir_all(&theme_dir).map_err(|error| error.to_string())?;
    for (name, content) in files {
        let target = theme_dir.join(&name);
        if !is_safe_path(&theme_dir, &target) {
            continue;
        }
        if name.ends_with('/') {
            fs::create_dir_all(&target).map_err(|error| error.to_string())?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let mut out = fs::File::create(&target).map_err(|error| error.to_string())?;
        out.write_all(&content).map_err(|error| error.to_string())?;
    }
    Ok(manifest)
}

fn static_response(content_type: &str, content: Vec<u8>) -> Response {
    let mut response = Response::new(Body::from(content));
    let headers = response.headers_mut();
    let _ = headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(content_type).unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    let _ = headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    let _ = headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    response
}

fn not_found() -> Response {
    service_unavailable("资源不存在", StatusCode::NOT_FOUND)
}

fn service_unavailable(message: &str, status: StatusCode) -> Response {
    (status, message.to_string()).into_response()
}

pub async fn set_active_theme(pg: &PgPool, short: &str) -> Result<(), String> {
    if short != DEFAULT_THEME_ID && !is_valid_theme_short(short) {
        return Err("无效的主题名称".into());
    }
    if short != DEFAULT_THEME_ID {
        let manifest_path = themes_dir().join(short).join(THEME_MANIFEST);
        if !manifest_path.is_file() && !embedded_theme_matches(short) {
            return Err("主题不存在".into());
        }
    }
    sqlx::query(
        "INSERT INTO system_settings (key, value) VALUES ($1, to_jsonb($2::text)) ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
    )
    .bind(USER_THEME_SETTING_KEY)
    .bind(short)
    .execute(pg)
    .await
    .map_err(|error| error.to_string())?;
    Ok(())
}

fn embedded_theme_matches(short: &str) -> bool {
    load_embedded_manifest()
        .ok()
        .is_some_and(|manifest| manifest.short == short)
}

pub fn delete_installed_theme(short: &str) -> Result<(), String> {
    if !is_valid_theme_short(short) {
        return Err("无效的主题名称".into());
    }
    let theme_dir = themes_dir().join(short);
    if !theme_dir.is_dir() {
        return Err("主题不存在".into());
    }
    fs::remove_dir_all(theme_dir).map_err(|error| error.to_string())
}
