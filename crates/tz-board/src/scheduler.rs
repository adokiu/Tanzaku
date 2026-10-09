use sqlx::PgPool;
use std::sync::Arc;
use tracing::{info, warn};
use uuid::Uuid;

pub fn spawn(state: Arc<crate::setup::AppState>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        let mut reconcile = tokio::time::interval(std::time::Duration::from_secs(600));
        let mut recover = tokio::time::interval(std::time::Duration::from_secs(AUTO_RECOVER_TICK_SECS));
        recover.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        interval.tick().await;
        reconcile.tick().await;
        recover.tick().await;
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let Some(pg) = state.postgres() else { continue; };
                    if let Err(error) = tick(&pg).await {
                        warn!(?error, "scheduler tick failed");
                    }
                }
                _ = recover.tick() => {
                    let Some(pg) = state.postgres() else { continue; };
                    if let Err(error) = auto_recover_tunnels(&pg).await {
                        warn!(?error, "tunnel auto-recover tick failed");
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

const AUTO_RECOVER_TICK_SECS: u64 = 10;
/// 重建后保持运行中达到该时长视为恢复成功；不足即再次异常计为一次失败。
const AUTO_RECOVER_STABLE_SECS: i32 = 30;
const AUTO_RECOVER_MAX_ATTEMPTS: i32 = 3;
/// 关闭到重新下发之间的间隔，确保 node 停掉监听、client 断开旧 carrier。
const AUTO_RECOVER_REOPEN_DELAY_SECS: i32 = 3;
const AUTO_RECOVER_BATCH: i64 = 50;
const AUTO_RECOVER_GAVE_UP_MARK: &str = "[自动重建连续失败 3 次，已停止重试]";

/// 异常隧道自动重建：先完全关闭（node/client 移除），下一轮再重新下发。
/// 上次运行中时长 ≥ 30 秒视为曾经恢复，失败计数从头开始；否则累加，达到 3 次后停止。
/// 手动启用/停用/编辑会清零计数。
async fn auto_recover_tunnels(pg: &PgPool) -> Result<(), sqlx::Error> {
    // 关闭期间被暂停/删除等外部操作接管的，不再重新开启。
    sqlx::query(
        "UPDATE tunnels SET auto_recover_closing = FALSE WHERE auto_recover_closing AND (status IN ('deleted', 'suspended', 'pending_review') OR enabled = TRUE)",
    )
    .execute(pg)
    .await?;

    let reopened: Vec<(Uuid, Uuid, Uuid, i32)> = sqlx::query_as(
        "UPDATE tunnels SET enabled = TRUE, auto_recover_closing = FALSE, status = 'provisioning', revision = revision + 1, updated_at = now()
         WHERE auto_recover_closing AND enabled = FALSE AND status NOT IN ('deleted', 'suspended', 'pending_review')
           AND auto_recover_at <= now() - make_interval(secs => $1::int)
         RETURNING id, node_id, client_id, auto_recover_attempts",
    )
    .bind(AUTO_RECOVER_REOPEN_DELAY_SECS)
    .fetch_all(pg)
    .await?;
    for (tunnel_id, node_id, client_id, attempt) in reopened {
        info!(%tunnel_id, attempt, "tunnel auto-recover: reopening");
        crate::ws::sync_tunnel_online(pg, node_id, client_id, tunnel_id).await;
    }

    let closed: Vec<(Uuid, Uuid, Uuid, i32)> = sqlx::query_as(
        "WITH picked AS (
             SELECT id FROM tunnels
             WHERE status = 'error' AND enabled = TRUE AND NOT auto_recover_closing
               AND (auto_recover_at IS NULL OR auto_recover_at <= now() - make_interval(secs => $1::int))
               AND (CASE WHEN COALESCE(last_active_secs, 0) >= $1::int THEN 0 ELSE auto_recover_attempts END) < $2::int
             ORDER BY updated_at
             LIMIT $3
             FOR UPDATE SKIP LOCKED
         )
         UPDATE tunnels t SET
             auto_recover_attempts = CASE WHEN COALESCE(t.last_active_secs, 0) >= $1::int THEN 1 ELSE t.auto_recover_attempts + 1 END,
             auto_recover_at = now(),
             auto_recover_closing = TRUE,
             last_active_secs = NULL,
             enabled = FALSE,
             status = 'provisioning',
             revision = t.revision + 1,
             updated_at = now()
         FROM picked
         WHERE t.id = picked.id
         RETURNING t.id, t.node_id, t.client_id, t.auto_recover_attempts",
    )
    .bind(AUTO_RECOVER_STABLE_SECS)
    .bind(AUTO_RECOVER_MAX_ATTEMPTS)
    .bind(AUTO_RECOVER_BATCH)
    .fetch_all(pg)
    .await?;
    for (tunnel_id, node_id, client_id, attempt) in closed {
        info!(%tunnel_id, attempt, max = AUTO_RECOVER_MAX_ATTEMPTS, "tunnel auto-recover: closing errored tunnel");
        crate::ws::sync_tunnel_offline(pg, node_id, client_id, tunnel_id).await;
    }

    sqlx::query(
        "UPDATE tunnels SET last_error = CONCAT_WS(' ', NULLIF(last_error, ''), $3::text)
         WHERE status = 'error' AND enabled = TRUE AND NOT auto_recover_closing
           AND auto_recover_attempts >= $2::int
           AND COALESCE(last_active_secs, 0) < $1::int
           AND auto_recover_at <= now() - make_interval(secs => $1::int)
           AND POSITION($3::text IN COALESCE(last_error, '')) = 0",
    )
    .bind(AUTO_RECOVER_STABLE_SECS)
    .bind(AUTO_RECOVER_MAX_ATTEMPTS)
    .bind(AUTO_RECOVER_GAVE_UP_MARK)
    .execute(pg)
    .await?;
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
