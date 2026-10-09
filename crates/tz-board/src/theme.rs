use axum::{
    body::Body,
    extract::Path,
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sqlx::PgPool;
use std::{
    env,
    fs,
    io::{Read, Write},
    path::{Component, Path as StdPath, PathBuf},
    sync::OnceLock,
};
use tracing::warn;

pub const USER_THEME_SETTING_KEY: &str = "user_theme";
const DIST_DIR: &str = "dist";
const INDEX_FILE: &str = "index.html";
const THEME_MANIFEST: &str = "tanzaku-theme.json";

#[derive(Debug, Serialize)]
pub struct PublicThemeInfo {
    pub short: String,
    pub version: String,
    /// 当前主题 managed 配置（已与 manifest 默认值合并）
    pub settings: Value,
}

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
    #[serde(default)]
    pub configuration: Option<ThemeConfiguration>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeConfiguration {
    #[serde(rename = "type")]
    pub config_type: String,
    #[serde(default)]
    pub name: Option<Value>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub data: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedThemeField {
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub name: Option<Value>,
    #[serde(default)]
    pub help: Option<Value>,
    #[serde(rename = "type")]
    pub field_type: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub options: Option<String>,
    #[serde(default)]
    pub default: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct ThemeSettingsView {
    pub short: String,
    pub configuration: Option<ThemeConfiguration>,
    pub settings: Value,
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
    if short.is_empty() {
        return false;
    }
    short
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
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
        Some(value) if value.is_string() => value.as_str().unwrap_or("").trim().to_owned(),
        Some(value) if value.get("short").and_then(|item| item.as_str()).is_some() => {
            value["short"].as_str().unwrap_or("").trim().to_owned()
        }
        _ => String::new(),
    }
}

/// 实际用于提供用户端静态资源的主题 ID；无可用主题时返回空（用户端不嵌入，必须磁盘安装）。
pub async fn resolve_serve_theme_id(pg: &PgPool) -> String {
    let active = active_theme_id(pg).await;
    if !active.is_empty()
        && is_valid_theme_short(&active)
        && themes_dir()
            .join(&active)
            .join(DIST_DIR)
            .join(INDEX_FILE)
            .is_file()
    {
        return active;
    }
    // 激活项无效时，尝试列表中第一个已安装主题
    list_installed_themes()
        .into_iter()
        .next()
        .map(|item| item.short)
        .unwrap_or_default()
}

/// 安装主题的磁盘相对路径展示：`theme/{short}`（相对 `TANZAKU_DATA`，默认 `./data`）。
pub fn theme_storage_relpath(short: &str) -> String {
    format!("theme/{short}")
}

/// 仅列出 `{TANZAKU_DATA}/theme/{short}/` 下真实存在且含 `tanzaku-theme.json` + `dist/index.html` 的主题。
pub fn list_installed_themes() -> Vec<ThemeManifest> {
    let mut themes = Vec::new();
    let Ok(entries) = fs::read_dir(themes_dir()) else {
        return themes;
    };
    for entry in entries.flatten() {
        if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            continue;
        }
        let dir = entry.path();
        let manifest_path = dir.join(THEME_MANIFEST);
        let index_path = dir.join(DIST_DIR).join(INDEX_FILE);
        if !manifest_path.is_file() || !index_path.is_file() {
            continue;
        }
        if let Ok(manifest) = read_manifest_file(&manifest_path) {
            if !is_valid_theme_short(&manifest.short) {
                continue;
            }
            // 目录名应与 short 一致，避免错位
            let folder = entry.file_name().to_string_lossy().to_string();
            if folder != manifest.short {
                continue;
            }
            if !themes.iter().any(|item| item.short == manifest.short) {
                themes.push(manifest);
            }
        }
    }
    themes.sort_by(|a, b| a.short.cmp(&b.short));
    themes
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
    if active_theme.is_empty() || !is_valid_theme_short(active_theme) {
        return service_unavailable(
            "未安装用户主题：请在管理端上传并启用主题包（$TANZAKU_DATA/theme/{short}/）",
            StatusCode::SERVICE_UNAVAILABLE,
        );
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
    is_valid_theme_short(theme_id)
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
    if !is_valid_theme_short(theme_id) {
        return None;
    }
    let base = themes_dir().join(theme_id);
    let local = base.join(relative);
    if !is_safe_path(&base, &local) {
        return None;
    }
    let meta = fs::metadata(&local).ok()?;
    if !meta.is_file() {
        return None;
    }
    let content = fs::read(&local).ok()?;
    Some((content, content_type_for_path(relative)))
}

pub async fn public_theme_info(pg: &PgPool) -> PublicThemeInfo {
    let short = resolve_serve_theme_id(pg).await;
    if short.is_empty() {
        return PublicThemeInfo {
            short,
            version: String::new(),
            settings: Value::Object(Map::new()),
        };
    }
    let manifest = load_theme_manifest(&short).ok();
    let version = manifest
        .as_ref()
        .map(|item| item.version.clone())
        .unwrap_or_default();
    let settings = resolve_theme_settings(pg, &short, manifest.as_ref()).await;
    PublicThemeInfo {
        short,
        version,
        settings,
    }
}

pub fn load_theme_manifest(short: &str) -> Result<ThemeManifest, String> {
    if !is_valid_theme_short(short) {
        return Err("无效的主题名称".into());
    }
    let path = themes_dir().join(short).join(THEME_MANIFEST);
    if !path.is_file() {
        return Err("主题不存在".into());
    }
    read_manifest_file(&path)
}

fn managed_fields(manifest: &ThemeManifest) -> Vec<ManagedThemeField> {
    let Some(configuration) = &manifest.configuration else {
        return Vec::new();
    };
    if configuration.config_type != "managed" {
        return Vec::new();
    }
    match &configuration.data {
        Value::Array(items) => items
            .iter()
            .filter_map(|item| serde_json::from_value::<ManagedThemeField>(item.clone()).ok())
            .collect(),
        _ => Vec::new(),
    }
}

fn managed_default_value(field: &ManagedThemeField) -> Value {
    if let Some(value) = &field.default {
        return value.clone();
    }
    if field.field_type == "select" {
        if let Some(options) = &field.options {
            if let Some(first) = options.split(',').next() {
                let trimmed = first.trim();
                if !trimmed.is_empty() {
                    return Value::String(trimmed.to_owned());
                }
            }
        }
    }
    match field.field_type.as_str() {
        "number" => Value::from(0),
        "switch" => Value::Bool(false),
        _ => Value::String(String::new()),
    }
}

async fn load_stored_theme_settings(pg: &PgPool, short: &str) -> Map<String, Value> {
    let row = sqlx::query_scalar::<_, Value>(
        "SELECT data FROM theme_configurations WHERE short = $1",
    )
    .bind(short)
    .fetch_optional(pg)
    .await
    .ok()
    .flatten();
    match row {
        Some(Value::Object(map)) => map,
        Some(Value::String(raw)) => serde_json::from_str(&raw)
            .ok()
            .and_then(|value: Value| value.as_object().cloned())
            .unwrap_or_default(),
        _ => Map::new(),
    }
}

pub async fn resolve_theme_settings(
    pg: &PgPool,
    short: &str,
    manifest: Option<&ThemeManifest>,
) -> Value {
    let mut settings = load_stored_theme_settings(pg, short).await;
    let fields = match manifest {
        Some(manifest) => managed_fields(manifest),
        None => load_theme_manifest(short)
            .map(|manifest| managed_fields(&manifest))
            .unwrap_or_default(),
    };
    for field in fields {
        let Some(key) = field.key.as_deref().filter(|key| !key.is_empty()) else {
            continue;
        };
        if !settings.contains_key(key) {
            settings.insert(key.to_owned(), managed_default_value(&field));
        }
    }
    Value::Object(settings)
}

pub async fn theme_settings_view(pg: &PgPool, short: &str) -> Result<ThemeSettingsView, String> {
    let manifest = load_theme_manifest(short)?;
    let settings = resolve_theme_settings(pg, short, Some(&manifest)).await;
    Ok(ThemeSettingsView {
        short: short.to_owned(),
        configuration: manifest.configuration.clone(),
        settings,
    })
}

pub async fn save_theme_settings(
    pg: &PgPool,
    short: &str,
    settings: Value,
) -> Result<(), String> {
    if !is_valid_theme_short(short) {
        return Err("无效的主题名称".into());
    }
    // 确认主题已安装
    load_theme_manifest(short)?;
    let Value::Object(_) = &settings else {
        return Err("主题设置必须是 JSON 对象".into());
    };
    sqlx::query(
        "INSERT INTO theme_configurations (short, data, updated_at) VALUES ($1, $2, now()) \
         ON CONFLICT (short) DO UPDATE SET data = EXCLUDED.data, updated_at = now()",
    )
    .bind(short)
    .bind(&settings)
    .execute(pg)
    .await
    .map_err(|error| error.to_string())?;
    Ok(())
}

pub async fn delete_theme_settings(pg: &PgPool, short: &str) -> Result<(), String> {
    sqlx::query("DELETE FROM theme_configurations WHERE short = $1")
        .bind(short)
        .execute(pg)
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn is_safe_path(base: &StdPath, target: &StdPath) -> bool {
    // 写入尚未落盘的文件时不能 canonicalize(target)，否则会误判为不安全并跳过全部解压。
    if target
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return false;
    }
    if let (Ok(abs_base), Ok(abs_target)) = (fs::canonicalize(base), fs::canonicalize(target)) {
        return abs_target.starts_with(abs_base);
    }
    let clean_base = base.components().collect::<PathBuf>();
    let clean_target = target.components().collect::<PathBuf>();
    clean_target.starts_with(&clean_base)
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
        let raw_name = file.name().replace('\\', "/");
        if raw_name.contains("..") {
            continue;
        }
        let mut content = Vec::new();
        file.read_to_end(&mut content)
            .map_err(|error| format!("读取压缩包内容失败: {error}"))?;
        let basename = raw_name.rsplit('/').next().unwrap_or(raw_name.as_str());
        if basename == THEME_MANIFEST {
            manifest = Some(parse_manifest(&content)?);
        }
        files.push((raw_name, content));
    }
    let manifest = manifest.ok_or_else(|| "缺少 tanzaku-theme.json".to_string())?;
    if !is_valid_theme_short(&manifest.short) {
        return Err("主题 short 名称无效".into());
    }
    // 若 zip 内全部文件位于同一顶层目录，剥掉该前缀（Komari 常见打包方式）
    let prefix = common_zip_root_prefix(&files);
    let files: Vec<(String, Vec<u8>)> = files
        .into_iter()
        .filter_map(|(name, content)| {
            let relative = if let Some(prefix) = prefix.as_deref() {
                name.strip_prefix(prefix)?.trim_start_matches('/').to_owned()
            } else {
                name
            };
            if relative.is_empty() {
                return None;
            }
            Some((relative, content))
        })
        .collect();
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
    let index_path = theme_dir.join(DIST_DIR).join(INDEX_FILE);
    if !index_path.is_file() {
        let _ = fs::remove_dir_all(&theme_dir);
        return Err("主题缺少 dist/index.html".into());
    }
    Ok(manifest)
}

fn common_zip_root_prefix(files: &[(String, Vec<u8>)]) -> Option<String> {
    let mut roots = files.iter().filter_map(|(name, _)| {
        let trimmed = name.trim_matches('/');
        if trimmed.is_empty() {
            return None;
        }
        let first = trimmed.split('/').next()?;
        if first.is_empty() || first == THEME_MANIFEST || first == DIST_DIR {
            return None;
        }
        Some(first.to_owned())
    });
    let first = roots.next()?;
    if roots.all(|root| root == first) {
        // 仅当存在「根目录/…」形式且根下没有直接的 tanzaku-theme.json / dist 时剥前缀
        let has_root_manifest = files.iter().any(|(name, _)| {
            name == THEME_MANIFEST || name.trim_matches('/') == THEME_MANIFEST
        });
        let has_root_dist = files.iter().any(|(name, _)| {
            name.starts_with("dist/") || name.trim_matches('/').starts_with("dist/")
        });
        if has_root_manifest || has_root_dist {
            return None;
        }
        Some(format!("{first}/"))
    } else {
        None
    }
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
    // 允许空字符串：清除激活主题（用户端将无法访问直至重新启用）
    if short.is_empty() {
        return persist_active_theme(pg, "").await;
    }
    if !is_valid_theme_short(short) {
        return Err("无效的主题名称".into());
    }
    let theme_dir = themes_dir().join(short);
    let manifest_path = theme_dir.join(THEME_MANIFEST);
    let index_path = theme_dir.join(DIST_DIR).join(INDEX_FILE);
    if !manifest_path.is_file() || !index_path.is_file() {
        return Err("主题不存在或缺少 dist/index.html".into());
    }
    persist_active_theme(pg, short).await
}

async fn persist_active_theme(pg: &PgPool, short: &str) -> Result<(), String> {
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
