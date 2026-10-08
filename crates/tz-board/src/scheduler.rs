use sqlx::PgPool;
use std::sync::Arc;
use tracing::{info, warn};
use uuid::Uuid;

pub fn spawn(state: Arc<crate::setup::AppState>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        let mut reconcile = tokio::time::interval(std::time::Duration::from_secs(600));
        interval.tick().await;
        reconcile.tick().await;
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let Some(pg) = state.postgres() else { continue; };
                    if let Err(error) = tick(&pg).await {
                        warn!(?error, "scheduler tick failed");
                    }
                }
                _ = reconcile.tick() => {
                    let Some(pg) = state.postgres() else { continue; };
                    let Some(redis) = state.redis_connection() else { continue; };
                    reconcile_redis_epoch(&pg, &redis).await;
                }
            }
        }
    });
}

async fn tick(pg: &PgPool) -> Result<(), sqlx::Error> {
    expire_subscriptions(pg).await?;
    mark_stale_clients_offline(pg).await?;
    mark_stale_nodes_offline(pg).await?;
    crate::guard_events::resume_expired_guard_pauses(pg).await;
    Ok(())
}

pub async fn reconcile_redis_epoch(pg: &PgPool, redis: &redis::aio::ConnectionManager) {
    let mut connection = redis.clone();
    let epoch: Option<String> = redis::AsyncCommands::get(&mut connection, "meta:epoch").await.ok();
    if epoch.is_some() {
        return;
    }
    let _ = crate::store::rebuild_port_maps(pg, redis).await;
}

async fn expire_subscriptions(pg: &PgPool) -> Result<(), sqlx::Error> {
    let expired: Vec<Uuid> = sqlx::query_scalar(
        "UPDATE user_subscriptions SET status = 'expired', updated_at = now() WHERE status = 'active' AND expires_at IS NOT NULL AND expires_at <= now() RETURNING id",
    )
    .fetch_all(pg)
    .await?;
    for subscription_id in expired {
        let user_id: Option<Uuid> = sqlx::query_scalar(
            "SELECT user_id FROM user_subscriptions WHERE id = $1",
        )
        .bind(subscription_id)
        .fetch_optional(pg)
        .await?;
        let Some(user_id) = user_id else {
            continue;
        };
        let tunnels: Vec<(Uuid, Uuid, Uuid)> = sqlx::query_as(
            "UPDATE tunnels SET status = 'suspended', enabled = FALSE, revision = revision + 1, updated_at = now() WHERE user_id = $1 AND status NOT IN ('deleted', 'suspended') RETURNING id, node_id, client_id",
        )
        .bind(user_id)
        .fetch_all(pg)
        .await?;
        for (tunnel_id, node_id, client_id) in tunnels {
            crate::ws::on_tunnel_removed(pg, node_id, client_id, tunnel_id).await;
        }
        info!(%subscription_id, %user_id, "subscription expired; tunnels suspended");
    }
    Ok(())
}

async fn mark_stale_clients_offline(pg: &PgPool) -> Result<(), sqlx::Error> {
    let stale: Vec<Uuid> = sqlx::query_scalar(
        "UPDATE clients SET online = FALSE, updated_at = now() WHERE online = TRUE AND last_seen_at < now() - interval '90 seconds' RETURNING id",
    )
    .fetch_all(pg)
    .await?;
    for client_id in stale {
        sqlx::query(
            "UPDATE tunnels SET status = 'client_offline', last_error = 'client heartbeat timeout', updated_at = now() WHERE client_id = $1 AND enabled = TRUE AND status NOT IN ('deleted', 'suspended', 'pending_review')",
        )
        .bind(client_id)
        .execute(pg)
        .await?;
    }
    Ok(())
}

async fn mark_stale_nodes_offline(pg: &PgPool) -> Result<(), sqlx::Error> {
    let stale: Vec<Uuid> = sqlx::query_scalar(
        "UPDATE nodes SET online = FALSE, updated_at = now() WHERE online = TRUE AND last_seen_at < now() - interval '90 seconds' RETURNING id",
    )
    .fetch_all(pg)
    .await?;
    for node_id in stale {
        sqlx::query(
            "UPDATE tunnels SET status = 'node_offline', last_error = 'node heartbeat timeout', updated_at = now() WHERE node_id = $1 AND enabled = TRUE AND status NOT IN ('deleted', 'suspended', 'pending_review')",
        )
        .bind(node_id)
        .execute(pg)
        .await?;
    }
    Ok(())
}
