use super::response_error;
use crate::{
    payment::{self, NotifyInput, NotifyOutcome, PayError},
    setup::{AppState, unavailable},
};
use axum::{
    Router,
    body::Bytes,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::post,
};
use serde_json::Value;
use std::{collections::HashMap, sync::Arc};

pub fn public_router() -> Router<Arc<AppState>> {
    Router::new().route("/api/v1/payments/{kind}/notify", post(public_notify))
}

async fn public_notify(
    State(_state): State<Arc<AppState>>,
    Path(kind): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    body: Bytes,
) -> Result<&'static str, (StatusCode, axum::Json<Value>)> {
    let driver = payment::find(&kind).ok_or_else(|| {
        response_error(unavailable(StatusCode::NOT_FOUND, "未知支付渠道类型"))
    })?;
    let query: Vec<(String, String)> = query.into_iter().collect();
    let body = String::from_utf8_lossy(&body);
    let input = NotifyInput {
        channel_config: &Value::Object(Default::default()),
        query: &query,
        body: body.as_ref(),
    };
    match driver.handle_notify(&input).await {
        Ok(NotifyOutcome::Ignored | NotifyOutcome::Paid { .. }) => Ok("success"),
        Err(PayError::Unavailable(message) | PayError::Message(message)) => Err(response_error(
            unavailable(StatusCode::BAD_REQUEST, message),
        )),
    }
}
