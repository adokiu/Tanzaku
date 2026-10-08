use crate::setup::{self, unavailable, AppState, UserRole};
use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, Method, Request, StatusCode},
    middleware::Next,
    response::Response,
};
use std::sync::Arc;

/// 管理端独立端口，仅校验 `tz_admin_session` Cookie。
pub async fn require_admin_csrf(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    request: Request<Body>,
    next: Next,
) -> Result<Response, Response> {
    guard(UserRole::Admin, state, headers, request, next).await
}

/// 用户端独立端口，仅校验 `tz_user_session` Cookie。
pub async fn require_user_csrf(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    request: Request<Body>,
    next: Next,
) -> Result<Response, Response> {
    guard(UserRole::User, state, headers, request, next).await
}

async fn guard(
    role: UserRole,
    state: Arc<AppState>,
    headers: HeaderMap,
    request: Request<Body>,
    next: Next,
) -> Result<Response, Response> {
    if matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    ) {
        return Ok(next.run(request).await);
    }
    let path = request.uri().path();
    if path.starts_with("/api/auth/") || path.starts_with("/api/init/") {
        return Ok(next.run(request).await);
    }
    let claims = setup::authenticate(&state, &headers, role).await?;
    let Some(header_token) = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
    else {
        return Err(unavailable(
            StatusCode::FORBIDDEN,
            "请刷新页面后重试",
        ));
    };
    if header_token.len() < 16
        || header_token.len() > 128
        || claims.csrf_token.is_empty()
        || header_token != claims.csrf_token
    {
        return Err(unavailable(
            StatusCode::FORBIDDEN,
            "请重新登录后重试",
        ));
    }
    Ok(next.run(request).await)
}
