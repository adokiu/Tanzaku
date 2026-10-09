use crate::{
    config::{BoardConfig, ListenConfig, PgConfig, RedisConfig},
    store,
};
use anyhow::Context;
use arc_swap::ArcSwap;
use argon2::PasswordVerifier;
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, OriginalUri, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use redis::{AsyncCommands, aio::ConnectionManager};
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::{
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
    time::Duration,
};
use tokio::sync::Mutex;
use tower_http::limit::RequestBodyLimitLayer;
use tracing::warn;
use uuid::Uuid;

const INIT_HTML: &str = include_str!("setup/init.html");

#[derive(RustEmbed)]
#[folder = "../../web-admin/dist/"]
struct AdminWebAssets;

pub enum RuntimeMode {
    Setup,
    Ready {
        db: store::DbPools,
        redis: Option<ConnectionManager>,
        redis_config: RedisConfig,
    },
}

pub struct AppState {
    mode: Arc<ArcSwap<RuntimeMode>>,
    config_path: PathBuf,
    listen: ListenConfig,
    trusted_proxies: Arc<crate::client_ip::TrustedProxies>,
    init_lock: Mutex<()>,
}

#[derive(Deserialize)]
pub struct InitRequest {
    pub postgres: PgConfig,
    pub redis: RedisConfig,
    pub admin_email: String,
    pub admin_password: String,
}

#[derive(Deserialize)]
pub struct InitPgTest {
    pub postgres: PgConfig,
}

#[derive(Deserialize)]
pub struct InitRedisTest {
    pub redis: RedisConfig,
}

#[derive(Serialize)]
struct SetupStatus {
    initialized: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum UserRole {
    Admin,
    User,
}

#[derive(Deserialize)]
struct LoginRequest {
    email: String,
    password: String,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct SessionClaims {
    pub(crate) user_id: Uuid,
    pub(crate) email: String,
    pub(crate) role: UserRole,
    #[serde(default)]
    pub(crate) csrf_token: String,
}

#[derive(Serialize)]
struct LoginResponse {
    user_id: Uuid,
    email: String,
    role: UserRole,
    csrf_token: String,
}

#[derive(Serialize)]
struct ApiMessage {
    message: &'static str,
}

#[derive(Clone)]
pub(crate) struct ApiErrorContext(pub(crate) String);

impl RuntimeMode {
    pub async fn start(config: BoardConfig) -> anyhow::Result<Self> {
        match (&config.postgres, &config.redis) {
            (None, None) => Ok(Self::Setup),
            (Some(pg_config), Some(redis_config)) => {
                let db = store::DbPools::connect(pg_config).await?;
                store::migrate(&db.api()).await?;
                let installed = sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS (SELECT 1 FROM system_settings WHERE key = 'installation_state')",
                )
                .fetch_one(&db.api())
                .await?;
                if !installed {
                    return Ok(Self::Setup);
                }
                let redis = match store::connect_redis(redis_config).await {
                    Ok(redis) => match store::rebuild_port_maps(&db.api(), &redis).await {
                        Ok(()) => Some(redis),
                        Err(_) => {
                            warn!(
                                "Redis port-cache rebuild failed; board starting in degraded mode"
                            );
                            None
                        }
                    },
                    Err(_) => {
                        warn!("Redis unavailable; board starting in degraded mode");
                        None
                    }
                };
                Ok(Self::Ready {
                    db,
                    redis,
                    redis_config: redis_config.clone(),
                })
            }
            _ => anyhow::bail!(
                "both PostgreSQL and Redis settings must be present in board configuration"
            ),
        }
    }
}

impl AppState {
    pub fn new(
        mode: RuntimeMode,
        config_path: PathBuf,
        listen: ListenConfig,
        trusted_proxy_cidrs: Vec<String>,
    ) -> Self {
        Self {
            mode: Arc::new(ArcSwap::from_pointee(mode)),
            config_path,
            listen,
            trusted_proxies: Arc::new(crate::client_ip::TrustedProxies::from_cidrs(
                &trusted_proxy_cidrs,
            )),
            init_lock: Mutex::new(()),
        }
    }

    pub(crate) fn trusted_proxies(&self) -> &crate::client_ip::TrustedProxies {
        &self.trusted_proxies
    }

    pub(crate) async fn database_for(
        &self,
        headers: &HeaderMap,
        role: UserRole,
    ) -> Result<(PgPool, SessionClaims), Response> {
        let claims = authenticate(self, headers, role).await?;
        let mode = self.mode.load_full();
        match &*mode {
            RuntimeMode::Ready { db, .. } => Ok((db.api(), claims)),
            RuntimeMode::Setup => Err(unavailable(
                StatusCode::SERVICE_UNAVAILABLE,
                "board 尚未初始化",
            )),
        }
    }

    pub(crate) fn redis_connection(&self) -> Option<ConnectionManager> {
        match &**self.mode.load() {
            RuntimeMode::Ready { redis, .. } => redis.clone(),
            RuntimeMode::Setup => None,
        }
    }

    pub(crate) fn ready_database(
        &self,
    ) -> Option<(PgPool, Option<ConnectionManager>)> {
        match &**self.mode.load() {
            RuntimeMode::Ready { db, redis, .. } => Some((db.metrics(), redis.clone())),
            RuntimeMode::Setup => None,
        }
    }

    pub(crate) fn postgres(&self) -> Option<PgPool> {
        self.pg_api()
    }

    pub(crate) fn pg_api(&self) -> Option<PgPool> {
        match &*self.mode.load_full() {
            RuntimeMode::Ready { db, .. } => Some(db.api()),
            RuntimeMode::Setup => None,
        }
    }

    pub(crate) fn pg_control(&self) -> Option<PgPool> {
        match &*self.mode.load_full() {
            RuntimeMode::Ready { db, .. } => Some(db.control()),
            RuntimeMode::Setup => None,
        }
    }

    pub(crate) fn disable_redis(&self) {
        if let RuntimeMode::Ready { db, redis_config, .. } = &*self.mode.load_full() {
            self.mode.store(Arc::new(RuntimeMode::Ready {
                db: db.clone(),
                redis: None,
                redis_config: redis_config.clone(),
            }));
        }
    }

    fn set_redis(&self, redis: Option<ConnectionManager>) {
        if let RuntimeMode::Ready { db, redis_config, .. } = &*self.mode.load_full() {
            self.mode.store(Arc::new(RuntimeMode::Ready {
                db: db.clone(),
                redis,
                redis_config: redis_config.clone(),
            }));
        }
    }
}

pub fn start_redis_worker(state: Arc<AppState>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        interval.tick().await;
        loop {
            interval.tick().await;
            let mode = state.mode.load_full();
            let RuntimeMode::Ready {
                db,
                redis,
                redis_config,
            } = &*mode
            else {
                continue;
            };
            let pg = db.api();
            let redis_config = redis_config.clone();
            let was_connected = redis.is_some();
            let connection = match redis.clone() {
                Some(connection) => connection,
                None => match store::connect_redis(&redis_config).await {
                    Ok(connection) => connection,
                    Err(_) => continue,
                },
            };
            let mut ping_connection = connection.clone();
            let ping: Result<String, _> = redis::cmd("PING")
                .query_async(&mut ping_connection)
                .await;
            if ping.is_err() {
                state.disable_redis();
                continue;
            }
            if !was_connected {
                if store::rebuild_port_maps(&pg, &connection).await.is_err() {
                    continue;
                }
                state.set_redis(Some(connection.clone()));
            }
            retry_port_outbox(&pg, &connection).await;
        }
    })
}

async fn retry_port_outbox(pg: &PgPool, redis: &ConnectionManager) {
    let rows = match sqlx::query_as::<_, (i64, serde_json::Value)>(
        "SELECT id, payload FROM redis_outbox WHERE operation = 'rebuild_node_ports' AND next_attempt_at <= now() ORDER BY id LIMIT 32",
    )
    .fetch_all(pg)
    .await
    {
        Ok(rows) => rows,
        Err(_) => return,
    };
    for (outbox_id, payload) in rows {
        let node_id = payload
            .get("node_id")
            .and_then(serde_json::Value::as_str)
            .and_then(|id| Uuid::parse_str(id).ok());
        let Some(node_id) = node_id else {
            let _ = sqlx::query("UPDATE redis_outbox SET attempts = attempts + 1, next_attempt_at = now() + interval '5 minutes' WHERE id = $1")
                .bind(outbox_id)
                .execute(pg)
                .await;
            continue;
        };
        let mut transaction = match pg.begin().await {
            Ok(transaction) => transaction,
            Err(_) => continue,
        };
        let lock_key = format!("tunnel-ports:{node_id}");
        let locked = sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 42))")
            .bind(lock_key)
            .execute(&mut *transaction)
            .await
            .is_ok();
        let rebuilt = locked
            && store::rebuild_node_port_map_tx(&mut transaction, redis, node_id)
                .await
                .is_ok();
        if rebuilt {
            if sqlx::query("DELETE FROM redis_outbox WHERE id = $1")
                .bind(outbox_id)
                .execute(&mut *transaction)
                .await
                .is_ok()
            {
                let _ = transaction.commit().await;
            } else {
                let _ = transaction.rollback().await;
            }
        } else {
            let _ = transaction.rollback().await;
            let _ = sqlx::query("UPDATE redis_outbox SET attempts = attempts + 1, next_attempt_at = now() + interval '30 seconds' WHERE id = $1")
                .bind(outbox_id)
                .execute(pg)
                .await;
        }
    }
}

pub fn admin_router(state: Arc<AppState>) -> Router {
    // admin HTTP + server(node) `/ws` 同端口；WS 单独挂载，避免 body limit 影响升级。
    // 管理端需上传主题 zip（可达数十 MB），body 上限 128MiB；用户端仍用小限制。
    const ADMIN_BODY_LIMIT: usize = 128 * 1024 * 1024;
    let http = Router::new()
        .route("/", get(admin_home))
        .route("/init", get(init_page))
        .route("/api/init/status", get(init_status))
        .route("/api/init/test/postgres", post(test_postgres))
        .route("/api/init/test/redis", post(test_redis))
        .route("/api/init/complete", post(complete_init))
        .route("/api/auth/login", post(admin_login))
        .route("/api/auth/logout", post(admin_logout))
        .route("/api/auth/me", get(admin_me))
        .route("/api/public/branding", get(public_branding))
        .merge(crate::api::admin_router().layer(
            axum::middleware::from_fn_with_state(
                state.clone(),
                crate::api::csrf::require_admin_csrf,
            ),
        ))
        .fallback(admin_static)
        .with_state(state.clone())
        .layer(DefaultBodyLimit::max(ADMIN_BODY_LIMIT))
        .layer(RequestBodyLimitLayer::new(ADMIN_BODY_LIMIT));
    Router::new()
        .merge(crate::ws::node_router(state))
        .merge(http)
}

pub fn user_router(state: Arc<AppState>) -> Router {
    // user HTTP + client(agent) `/ws` 同端口。
    let http = Router::new()
        .route("/api/auth/login", post(user_login))
        .route("/api/auth/logout", post(user_logout))
        .route("/api/auth/me", get(user_me))
        .route("/api/public/branding", get(public_branding))
        .route("/api/public/theme", get(public_theme))
        .route(
            "/themes/{theme_id}/{*rest}",
            get(crate::theme::theme_asset),
        )
        .merge(crate::api::payments::public_router())
        .merge(crate::api::user_router().layer(
            axum::middleware::from_fn_with_state(state.clone(), crate::api::csrf::require_user_csrf),
        ))
        .fallback(user_static)
        .with_state(state.clone())
        .layer(RequestBodyLimitLayer::new(128 * 1024));
    Router::new()
        .merge(crate::ws::agent_router(state))
        .merge(http)
}

async fn admin_home(State(state): State<Arc<AppState>>) -> Response {
    let destination = if matches!(&**state.mode.load(), RuntimeMode::Setup) {
        "/init"
    } else {
        "/dashboard"
    };
    Redirect::temporary(destination).into_response()
}

async fn init_page(State(state): State<Arc<AppState>>) -> Response {
    if !matches!(&**state.mode.load(), RuntimeMode::Setup) {
        return Redirect::temporary("/login").into_response();
    }
    serve_admin_file("/init", true).await
}

async fn admin_static(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
) -> Response {
    serve_admin_file(
        uri.path(),
        matches!(&**state.mode.load(), RuntimeMode::Setup),
    )
    .await
}

async fn user_static(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
) -> Response {
    if matches!(&**state.mode.load(), RuntimeMode::Setup) {
        return unavailable(StatusCode::SERVICE_UNAVAILABLE, "board 尚未初始化");
    }
    let Some(pg) = state.postgres() else {
        return unavailable(StatusCode::SERVICE_UNAVAILABLE, "board 尚未初始化");
    };
    let active = crate::theme::resolve_serve_theme_id(&pg).await;
    crate::theme::serve_user_portal(uri.path(), &active).await
}

async fn serve_admin_file(requested: &str, setup: bool) -> Response {
    if requested == "/init" && setup {
        return match AdminWebAssets::get("index.html") {
            Some(file) => static_response("text/html; charset=utf-8", file.data.into_owned()),
            None => {
                let mut response = Html(INIT_HTML).into_response();
                let _ = response
                    .headers_mut()
                    .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
                response
            }
        };
    }
    if setup && requested == "/" {
        return Redirect::temporary("/init").into_response();
    }
    serve_admin_spa(requested).await
}

async fn serve_admin_spa(requested: &str) -> Response {
    if requested.starts_with("/api/") {
        return unavailable(StatusCode::NOT_FOUND, "接口不存在");
    }
    let relative = requested.trim_start_matches('/');
    if !relative.is_empty() {
        if let Some(file) = AdminWebAssets::get(relative) {
            let content_type = match relative.rsplit('.').next() {
                Some("js") => "text/javascript; charset=utf-8",
                Some("css") => "text/css; charset=utf-8",
                Some("svg") => "image/svg+xml",
                Some("png") => "image/png",
                Some("woff2") => "font/woff2",
                Some("json") => "application/json",
                _ => "application/octet-stream",
            };
            return static_response(content_type, file.data.into_owned());
        }
        if relative
            .rsplit('/')
            .next()
            .is_some_and(|name| name.contains('.'))
        {
            return unavailable(StatusCode::NOT_FOUND, "资源不存在");
        }
    }
    match AdminWebAssets::get("index.html") {
        Some(file) => static_response("text/html; charset=utf-8", file.data.into_owned()),
        None => unavailable(StatusCode::SERVICE_UNAVAILABLE, "管理端前端资源未构建"),
    }
}

fn static_response(content_type: &'static str, content: Vec<u8>) -> Response {
    let mut response = Response::new(Body::from(content));
    let headers = response.headers_mut();
    let _ = headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    let _ = headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    let _ = headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    response
}

async fn init_status(State(state): State<Arc<AppState>>) -> Json<SetupStatus> {
    Json(SetupStatus {
        initialized: matches!(&**state.mode.load(), RuntimeMode::Ready { .. }),
    })
}

async fn test_postgres(
    State(state): State<Arc<AppState>>,
    Json(request): Json<InitPgTest>,
) -> Response {
    if !matches!(&**state.mode.load(), RuntimeMode::Setup) {
        return unavailable(StatusCode::CONFLICT, "board 已经完成初始化");
    }
    match store::connect_postgres(&request.postgres).await {
        Ok(pool) => {
            pool.close().await;
            Json(ApiMessage {
                message: "PostgreSQL 连接成功",
            })
            .into_response()
        }
        Err(_) => {
            warn!("PostgreSQL connection test failed");
            unavailable(
                StatusCode::BAD_GATEWAY,
                "PostgreSQL 连接失败，请检查填写的信息",
            )
        }
    }
}

async fn test_redis(
    State(state): State<Arc<AppState>>,
    Json(request): Json<InitRedisTest>,
) -> Response {
    if !matches!(&**state.mode.load(), RuntimeMode::Setup) {
        return unavailable(StatusCode::CONFLICT, "board 已经完成初始化");
    }
    match store::connect_redis(&request.redis).await {
        Ok(_) => Json(ApiMessage {
            message: "Redis 连接成功",
        })
        .into_response(),
        Err(_) => {
            warn!("Redis connection test failed");
            unavailable(StatusCode::BAD_GATEWAY, "Redis 连接失败，请检查填写的信息")
        }
    }
}

async fn complete_init(
    State(state): State<Arc<AppState>>,
    Json(request): Json<InitRequest>,
) -> Response {
    let _guard = state.init_lock.lock().await;
    if !matches!(&**state.mode.load(), RuntimeMode::Setup) {
        return unavailable(StatusCode::CONFLICT, "board 已经完成初始化");
    }
    if let Err(error) = validate_admin(&request.admin_email, &request.admin_password) {
        return unavailable(StatusCode::BAD_REQUEST, error);
    }
    if let Err(error) = request
        .postgres
        .validate()
        .and_then(|_| request.redis.validate())
    {
        return unavailable(StatusCode::BAD_REQUEST, error.to_string());
    }
    let (db, redis) = match tokio::try_join!(
        store::DbPools::connect(&request.postgres),
        store::connect_redis(&request.redis),
    ) {
        Ok(connections) => connections,
        Err(_) => {
            warn!("initialization dependency check failed");
            return unavailable(
                StatusCode::BAD_GATEWAY,
                "无法连接数据库服务，请检查连接配置",
            );
        }
    };
    let pg = db.api();
    if let Err(error) = store::migrate(&pg).await {
        warn!(?error, "initialization migration failed");
        return unavailable(
            StatusCode::INTERNAL_SERVER_ERROR,
            "数据库迁移失败，请检查 PostgreSQL 权限",
        );
    }
    let admin_email = request.admin_email.trim().to_lowercase();
    if let Err(error) = store::initialize_database(&pg, &admin_email, &request.admin_password).await {
        warn!(?error, "initialization database transaction failed");
        return unavailable(
            StatusCode::CONFLICT,
            "数据库初始化失败；如果此安装已创建管理员，请用相同的邮箱和密码重试",
        );
    }
    let redis_config = request.redis.clone();
    if let Err(error) = BoardConfig::persist_database_config(
        &state.config_path,
        &request.postgres,
        &request.redis,
        true,
    ) {
        warn!(
            ?error,
            path = %state.config_path.display(),
            "failed to persist board configuration"
        );
        return unavailable(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!(
                "管理员和数据库初始化已完成，但无法写入配置文件 {}；请确认目录可写、磁盘未满，然后用相同管理员账户重试",
                state.config_path.display()
            ),
        );
    }
    state.mode.store(Arc::new(RuntimeMode::Ready {
        db,
        redis: Some(redis),
        redis_config,
    }));
    Json(ApiMessage {
        message: "初始化完成",
    })
    .into_response()
}

async fn public_theme(State(state): State<Arc<AppState>>) -> Response {
    let mode = state.mode.load_full();
    let RuntimeMode::Ready { db, .. } = &*mode else {
        return Json(serde_json::json!({
            "short": "",
            "version": "",
            "settings": {},
        }))
        .into_response();
    };
    let info = crate::theme::public_theme_info(&db.api()).await;
    Json(info).into_response()
}

async fn public_branding(State(state): State<Arc<AppState>>) -> Response {
    let mode = state.mode.load_full();
    let RuntimeMode::Ready { db, .. } = &*mode else {
        return Json(serde_json::json!({
            "site_title": "",
            "site_subtitle": "",
            "site_description": "",
            "site_url": ""
        }))
        .into_response();
    };
    let branding = crate::system_settings::site_branding(&db.api()).await;
    Json(serde_json::json!({
        "site_title": branding.site_title,
        "site_subtitle": branding.site_subtitle,
        "site_description": branding.site_description,
        "site_url": branding.site_url,
    }))
    .into_response()
}

async fn admin_login(
    State(state): State<Arc<AppState>>,
    client_ip: crate::client_ip::ClientIp,
    Json(request): Json<LoginRequest>,
) -> Response {
    login(state, client_ip, request, UserRole::Admin).await
}

async fn user_login(
    State(state): State<Arc<AppState>>,
    client_ip: crate::client_ip::ClientIp,
    Json(request): Json<LoginRequest>,
) -> Response {
    // 用户端允许 admin / user 登录（管理员也可使用用户中心）。
    login(state, client_ip, request, UserRole::User).await
}

async fn login(
    state: Arc<AppState>,
    client_ip: crate::client_ip::ClientIp,
    request: LoginRequest,
    portal: UserRole,
) -> Response {
    let mode = state.mode.load_full();
    let RuntimeMode::Ready {
        db,
        redis: Some(redis),
        ..
    } = &*mode
    else {
        return unavailable(StatusCode::SERVICE_UNAVAILABLE, "board 或认证服务暂不可用");
    };
    let pg = db.api();
    if request.password.len() > 1024 || request.email.len() > 254 {
        return unavailable(StatusCode::UNAUTHORIZED, "邮箱或密码错误");
    }
    let email = request.email.trim().to_lowercase();
    let mut redis = redis.clone();
    // 基线：按真实客户端 IP 限流，防扫号（反代/CDN 后不能用 peer=127.0.0.1）
    let ip_fail_key = format!("auth:fail:ip:{}:{}", portal.as_str(), client_ip);
    let ip_attempts: i64 = match redis.incr(&ip_fail_key, 1).await {
        Ok(attempts) => attempts,
        Err(_) => return unavailable(StatusCode::SERVICE_UNAVAILABLE, "认证服务暂不可用"),
    };
    if redis.expire(&ip_fail_key, 900).await.unwrap_or(false) == false {
        return unavailable(StatusCode::SERVICE_UNAVAILABLE, "认证服务暂不可用");
    }
    if ip_attempts > 10 {
        return unavailable(
            StatusCode::TOO_MANY_REQUESTS,
            "登录尝试次数过多，请稍后重试",
        );
    }

    let policy = crate::system_settings::password_attempt_policy(&pg).await;
    let account_fail_key = format!("auth:fail:account:{}:{email}", portal.as_str());
    let account_lock_key = format!("auth:lock:account:{}:{email}", portal.as_str());
    if policy.enabled {
        let locked: bool = redis.exists(&account_lock_key).await.unwrap_or(false);
        if locked {
            return unavailable(
                StatusCode::TOO_MANY_REQUESTS,
                if policy.lock_minutes == 0 {
                    "账户已锁定，请联系管理员解锁"
                } else {
                    "账户已锁定，请稍后重试"
                },
            );
        }
    }

    let account = if portal == UserRole::Admin {
        sqlx::query_as::<_, (Uuid, String, String, String)>(
            "SELECT id, email, password_hash, role FROM users WHERE email = $1 AND role = 'admin' AND status = 'active'",
        )
        .bind(&email)
        .fetch_optional(&pg)
        .await
    } else {
        sqlx::query_as::<_, (Uuid, String, String, String)>(
            "SELECT id, email, password_hash, role FROM users WHERE email = $1 AND role IN ('user', 'admin') AND status = 'active'",
        )
        .bind(&email)
        .fetch_optional(&pg)
        .await
    };
    let verified = match account {
        Ok(Some((user_id, email, hash, role_str))) => {
            let account_role = match role_str.as_str() {
                "admin" => UserRole::Admin,
                "user" => UserRole::User,
                _ => {
                    return unavailable(StatusCode::UNAUTHORIZED, "邮箱或密码错误");
                }
            };
            let valid = argon2::PasswordHash::new(&hash).ok().is_some_and(|hash| {
                argon2::Argon2::default()
                    .verify_password(request.password.as_bytes(), &hash)
                    .is_ok()
            });
            if valid {
                Some((user_id, email, account_role))
            } else {
                None
            }
        }
        Ok(None) => None,
        Err(_) => return unavailable(StatusCode::SERVICE_UNAVAILABLE, "认证服务暂不可用"),
    };
    let Some((user_id, email, role)) = verified else {
        if policy.enabled {
            let attempts: i64 = redis.incr(&account_fail_key, 1).await.unwrap_or(0);
            let window_secs = if policy.lock_minutes == 0 {
                24 * 60 * 60
            } else {
                i64::from(policy.lock_minutes.max(1)) * 60
            };
            let _: Result<bool, _> = redis.expire(&account_fail_key, window_secs).await;
            if attempts >= i64::from(policy.max_attempts) {
                let lock_ttl = if policy.lock_minutes == 0 {
                    30 * 24 * 60 * 60
                } else {
                    i64::from(policy.lock_minutes) * 60
                };
                let _: Result<(), _> = redis
                    .set_ex::<_, _, ()>(&account_lock_key, "1", lock_ttl as u64)
                    .await;
                return unavailable(
                    StatusCode::TOO_MANY_REQUESTS,
                    if policy.lock_minutes == 0 {
                        "密码错误次数过多，账户已锁定，请联系管理员"
                    } else {
                        "密码错误次数过多，账户已暂时锁定"
                    },
                );
            }
        }
        return unavailable(StatusCode::UNAUTHORIZED, "邮箱或密码错误");
    };
    let _: Result<usize, _> = redis.del(&ip_fail_key).await;
    let _: Result<usize, _> = redis.del(&account_fail_key).await;
    let _: Result<usize, _> = redis.del(&account_lock_key).await;
    let mut token_bytes = [0u8; 32];
    if getrandom::fill(&mut token_bytes).is_err() {
        return unavailable(StatusCode::INTERNAL_SERVER_ERROR, "无法建立登录会话");
    }
    let token = token_bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let key = session_key(&token);
    let csrf_token = new_csrf_token();
    let claims = SessionClaims {
        user_id,
        email: email.clone(),
        role,
        csrf_token: csrf_token.clone(),
    };
    let payload = match serde_json::to_string(&claims) {
        Ok(payload) => payload,
        Err(_) => return unavailable(StatusCode::INTERNAL_SERVER_ERROR, "无法建立登录会话"),
    };
    if redis
        .set_ex::<_, _, ()>(&key, payload, 12 * 60 * 60)
        .await
        .is_err()
    {
        return unavailable(StatusCode::SERVICE_UNAVAILABLE, "认证服务暂不可用");
    }
    let _: Result<(), _> = sqlx::query("UPDATE users SET last_login_at = now() WHERE id = $1")
        .bind(user_id)
        .execute(&pg)
        .await
        .map(|_| ());
    let _: Result<(), _> = sqlx::query(
        "INSERT INTO audit_logs (actor_id, action, target_type, target_id, remote_ip) \
         VALUES ($1, 'auth.login', 'user', $2, $3::inet)",
    )
    .bind(user_id)
    .bind(user_id.to_string())
    .bind(client_ip.as_str())
    .execute(&pg)
    .await
    .map(|_| ());
    let cookie_name = portal.cookie_name();
    let cookie = format!("{cookie_name}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age=43200");
    let mut response = Json(LoginResponse {
        user_id,
        email,
        role,
        csrf_token,
    })
    .into_response();
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    response
}

async fn admin_me(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    match session_claims_for_me(&state, &headers, UserRole::Admin).await {
        Ok(claims) => Json(claims).into_response(),
        Err(response) => response,
    }
}

async fn user_me(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    match session_claims_for_me(&state, &headers, UserRole::User).await {
        Ok(claims) => Json(claims).into_response(),
        Err(response) => response,
    }
}

async fn admin_logout(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    logout(&state, &headers, UserRole::Admin).await
}

async fn user_logout(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    logout(&state, &headers, UserRole::User).await
}

async fn logout(state: &AppState, headers: &HeaderMap, role: UserRole) -> Response {
    if let Some(token) = read_cookie(headers, role.cookie_name()) {
        let mode = state.mode.load_full();
        if let RuntimeMode::Ready {
            redis: Some(redis), ..
        } = &*mode
        {
            let mut redis = redis.clone();
            let _: Result<usize, _> = redis.del(session_key(&token)).await;
        }
    }
    let cookie = format!(
        "{}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0",
        role.cookie_name()
    );
    let mut response = StatusCode::NO_CONTENT.into_response();
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    response
}

pub(crate) async fn authenticate(
    state: &AppState,
    headers: &HeaderMap,
    role: UserRole,
) -> Result<SessionClaims, Response> {
    let Some(token) = read_cookie(headers, role.cookie_name()) else {
        return Err(unavailable(StatusCode::UNAUTHORIZED, "请先登录"));
    };
    if token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(unavailable(StatusCode::UNAUTHORIZED, "登录会话无效"));
    }
    let mode = state.mode.load_full();
    let RuntimeMode::Ready {
        db,
        redis: Some(redis),
        ..
    } = &*mode
    else {
        return Err(unavailable(
            StatusCode::SERVICE_UNAVAILABLE,
            "board 或认证服务暂不可用",
        ));
    };
    let pg = db.api();
    let mut redis = redis.clone();
    let payload: Option<String> = redis
        .get(session_key(&token))
        .await
        .map_err(|_| unavailable(StatusCode::SERVICE_UNAVAILABLE, "认证服务暂不可用"))?;
    let Some(payload) = payload else {
        return Err(unavailable(StatusCode::UNAUTHORIZED, "登录会话已过期"));
    };
    let claims: SessionClaims = serde_json::from_str(&payload)
        .map_err(|_| unavailable(StatusCode::UNAUTHORIZED, "登录会话无效"))?;
    // 管理端仅 admin；用户端允许 admin / user（管理员可用用户中心）。
    let portal_ok = match role {
        UserRole::Admin => claims.role == UserRole::Admin,
        UserRole::User => {
            claims.role == UserRole::User || claims.role == UserRole::Admin
        }
    };
    if !portal_ok {
        return Err(unavailable(StatusCode::FORBIDDEN, "无权访问"));
    }
    let active: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM users WHERE id = $1 AND role = $2 AND status = 'active')",
    )
    .bind(claims.user_id)
    .bind(claims.role.as_str())
    .fetch_one(&pg)
    .await
    .map_err(|_| unavailable(StatusCode::SERVICE_UNAVAILABLE, "认证服务暂不可用"))?;
    if !active {
        let _: Result<usize, _> = redis.del(session_key(&token)).await;
        return Err(unavailable(StatusCode::UNAUTHORIZED, "账户已停用"));
    }
    Ok(claims)
}

fn new_csrf_token() -> String {
    let mut bytes = [0_u8; 32];
    if getrandom::fill(&mut bytes).is_err() {
        return String::new();
    }
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// 换绑邮箱后写回当前用户会话，避免刷新后仍显示旧邮箱。
pub(crate) async fn update_session_email(
    state: &AppState,
    headers: &HeaderMap,
    role: UserRole,
    new_email: &str,
) -> Result<(), Response> {
    let Some(session_token) = read_cookie(headers, role.cookie_name()) else {
        return Ok(());
    };
    let mode = state.mode.load_full();
    let RuntimeMode::Ready {
        redis: Some(redis),
        ..
    } = &*mode
    else {
        return Ok(());
    };
    let mut redis = redis.clone();
    let key = session_key(&session_token);
    let payload: Option<String> = redis.get(&key).await.unwrap_or(None);
    let Some(payload) = payload else {
        return Ok(());
    };
    let Ok(mut claims) = serde_json::from_str::<SessionClaims>(&payload) else {
        return Ok(());
    };
    claims.email = new_email.to_owned();
    let Ok(updated) = serde_json::to_string(&claims) else {
        return Ok(());
    };
    let _: Result<(), _> = redis
        .set_ex::<_, _, ()>(key, updated, 12 * 60 * 60)
        .await;
    Ok(())
}

async fn session_claims_for_me(
    state: &AppState,
    headers: &HeaderMap,
    role: UserRole,
) -> Result<SessionClaims, Response> {
    let Some(session_token) = read_cookie(headers, role.cookie_name()) else {
        return Err(unavailable(StatusCode::UNAUTHORIZED, "请先登录"));
    };
    let mut claims = authenticate(state, headers, role).await?;
    if !claims.csrf_token.is_empty() {
        return Ok(claims);
    }
    claims.csrf_token = new_csrf_token();
    if claims.csrf_token.is_empty() {
        return Err(unavailable(
            StatusCode::INTERNAL_SERVER_ERROR,
            "无法刷新会话",
        ));
    }
    let mode = state.mode.load_full();
    let RuntimeMode::Ready {
        redis: Some(redis),
        ..
    } = &*mode
    else {
        return Err(unavailable(
            StatusCode::SERVICE_UNAVAILABLE,
            "board 或认证服务暂不可用",
        ));
    };
    let payload = serde_json::to_string(&claims)
        .map_err(|_| unavailable(StatusCode::INTERNAL_SERVER_ERROR, "无法刷新会话"))?;
    let mut redis = redis.clone();
    if redis
        .set_ex::<_, _, ()>(
            session_key(&session_token),
            payload,
            12 * 60 * 60,
        )
        .await
        .is_err()
    {
        return Err(unavailable(
            StatusCode::SERVICE_UNAVAILABLE,
            "认证服务暂不可用",
        ));
    }
    Ok(claims)
}

fn read_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|part| {
            let (key, value) = part.trim().split_once('=')?;
            (key == name).then(|| value.to_owned())
        })
}

fn session_key(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    let encoded = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("session:{encoded}")
}

impl UserRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::User => "user",
        }
    }

    fn cookie_name(self) -> &'static str {
        match self {
            Self::Admin => "tz_admin_session",
            Self::User => "tz_user_session",
        }
    }
}

async fn user_unavailable(State(state): State<Arc<AppState>>) -> Response {
    match &**state.mode.load() {
        RuntimeMode::Setup => unavailable(StatusCode::SERVICE_UNAVAILABLE, "board 尚未初始化"),
        RuntimeMode::Ready { .. } => unavailable(StatusCode::NOT_IMPLEMENTED, "用户接口尚未接入"),
    }
}

async fn control_not_ready(State(state): State<Arc<AppState>>) -> Response {
    match &**state.mode.load() {
        RuntimeMode::Setup => unavailable(StatusCode::SERVICE_UNAVAILABLE, "board 尚未初始化"),
        RuntimeMode::Ready { .. } => {
            unavailable(StatusCode::NOT_IMPLEMENTED, "控制 WebSocket 尚未接入")
        }
    }
}

pub(crate) fn unavailable(status: StatusCode, message: impl Into<String>) -> Response {
    let message = message.into();
    let mut response = (
        status,
        Json(serde_json::json!({ "error": message.clone() })),
    )
        .into_response();
    response.extensions_mut().insert(ApiErrorContext(message));
    response
}

fn validate_admin(email: &str, password: &str) -> Result<(), &'static str> {
    let email = email.trim();
    let Some((local, domain)) = email.split_once('@') else {
        return Err("请输入有效的管理员邮箱");
    };
    if email.len() > 254
        || local.is_empty()
        || domain.is_empty()
        || !domain.contains('.')
        || email.matches('@').count() != 1
        || email.chars().any(char::is_whitespace)
    {
        return Err("请输入有效的管理员邮箱");
    }
    store::validate_password(password)?;
    Ok(())
}

