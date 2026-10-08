use crate::setup::{AppState, UserRole};
use axum::{
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::Response,
    routing::get,
};
use futures_util::{SinkExt, StreamExt};
use redis::AsyncCommands;
use std::sync::Arc;

pub fn router() -> axum::Router<Arc<AppState>> {
    axum::Router::new().route("/api/v1/live", get(live_upgrade))
}

async fn live_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<Response, Response> {
    let (pg, claims) = state
        .database_for(&headers, UserRole::User)
        .await
        .map_err(|response| response)?;
    Ok(ws.on_upgrade(move |socket| handle_live(socket, state, pg, claims.user_id)))
}

async fn handle_live(
    socket: WebSocket,
    state: Arc<AppState>,
    pg: sqlx::PgPool,
    user_id: uuid::Uuid,
) {
    let (mut sender, mut receiver) = socket.split();
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(1));
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let payload = build_snapshot(&state, &pg, user_id).await;
                if sender.send(Message::Text(payload.into())).await.is_err() {
                    break;
                }
            }
            message = receiver.next() => {
                if message.is_none() {
                    break;
                }
            }
        }
    }
}

async fn build_snapshot(state: &AppState, pg: &sqlx::PgPool, user_id: uuid::Uuid) -> String {
    let tunnels: Vec<(uuid::Uuid, String)> = match sqlx::query_as(
        "SELECT id, status FROM tunnels WHERE user_id = $1 AND status <> 'deleted'",
    )
    .bind(user_id)
    .fetch_all(pg)
    .await
    {
        Ok(rows) => rows,
        Err(_) => return r#"{"ok":false}"#.into(),
    };
    let mut live = serde_json::Map::new();
    live.insert("ok".into(), true.into());
    if let Some(mut redis) = state.redis_connection() {
        let mut tunnel_stats = serde_json::Map::new();
        for (tunnel_id, status) in tunnels {
            let key = format!("tunnel:{tunnel_id}:live");
            let bytes_in: i64 = redis.hget(&key, "bytes_in").await.unwrap_or(0);
            let bytes_out: i64 = redis.hget(&key, "bytes_out").await.unwrap_or(0);
            let active_conns: i64 = redis.hget(&key, "active_conns").await.unwrap_or(0);
            let rejects_quota: i64 = redis.hget(&key, "rejects_quota").await.unwrap_or(0);
            let rejects_guard: i64 = redis.hget(&key, "rejects_guard").await.unwrap_or(0);
            tunnel_stats.insert(
                tunnel_id.to_string(),
                serde_json::json!({
                    "status": status,
                    "bytes_in": bytes_in,
                    "bytes_out": bytes_out,
                    "active_conns": active_conns,
                    "rejects_quota": rejects_quota,
                    "rejects_guard": rejects_guard,
                }),
            );
        }
        live.insert("tunnels".into(), tunnel_stats.into());
    } else {
        live.insert("degraded".into(), true.into());
    }
    serde_json::Value::Object(live).to_string()
}
