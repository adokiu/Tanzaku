//! 订阅流量配额：Board 收到 node 流量上报即在内存累计，超额立即下发控制面命令停用隧道。
//! 数据面不做配额判断，允许少量超跑（一个上报周期 + 控制面下发延迟）。
//!
//! 内存基线 = PG `traffic_usage` 已结算量 + Redis 待结算量；每次 60s 结算后整体失效重载。

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use sqlx::PgPool;
use std::sync::{
    Arc, LazyLock,
    atomic::{AtomicBool, AtomicI64, Ordering},
};
use tracing::{info, warn};
use tz_proto::TunnelTrafficSample;
use uuid::Uuid;

/// 因配额耗尽被停用的隧道写入该 `last_error`，额度恢复后据此自动重新启用。
pub const QUOTA_SUSPEND_REASON: &str = "流量配额已用尽";

#[derive(Clone, Copy, Debug)]
pub enum CountMode {
    Sum,
    Inbound,
    Outbound,
    Max,
}

impl CountMode {
    pub fn parse(value: &str) -> Self {
        match value {
            "inbound" => Self::Inbound,
            "outbound" => Self::Outbound,
            "max" => Self::Max,
            _ => Self::Sum,
        }
    }

    pub fn used(self, bytes_in: i64, bytes_out: i64) -> i64 {
        match self {
            Self::Sum => bytes_in.saturating_add(bytes_out),
            Self::Inbound => bytes_in,
            Self::Outbound => bytes_out,
            Self::Max => bytes_in.max(bytes_out),
        }
    }

    /// 管理员直接设置「已用流量」时，按计费口径落到 in/out 两列。
    pub fn split_used(self, used: i64) -> (i64, i64) {
        match self {
            Self::Outbound => (0, used),
            _ => (used, 0),
        }
    }
}

struct LiveSub {
    id: Uuid,
    user_id: Uuid,
    quota: i64,
    mode: CountMode,
    period_start: DateTime<Utc>,
    bytes_in: AtomicI64,
    bytes_out: AtomicI64,
    tripped: AtomicBool,
}

/// tunnel → 有配额的有效订阅（`None` 表示无配额，不追踪）。
static TUNNEL_SUB: LazyLock<DashMap<Uuid, Option<Uuid>>> = LazyLock::new(DashMap::new);
static SUBS: LazyLock<DashMap<Uuid, Arc<LiveSub>>> = LazyLock::new(DashMap::new);

pub fn invalidate() {
    TUNNEL_SUB.clear();
    SUBS.clear();
}

#[derive(sqlx::FromRow)]
pub(crate) struct SubRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub traffic_quota_bytes: Option<i64>,
    pub traffic_count_mode: String,
    pub traffic_period: String,
    pub starts_at: DateTime<Utc>,
    pub period_anchor: i16,
    pub exhausted_period_start: Option<DateTime<Utc>>,
}

const SUB_COLUMNS: &str = "s.id, s.user_id, s.traffic_quota_bytes, s.traffic_count_mode, s.traffic_period, s.starts_at, s.period_anchor, s.exhausted_period_start";

impl SubRow {
    pub fn period_start(&self, now: DateTime<Utc>) -> DateTime<Utc> {
        crate::subscription_period::subscription_period_start(
            &self.traffic_period,
            self.starts_at,
            self.period_anchor,
            now,
        )
    }
}

async fn settled_usage(pg: &PgPool, sub_id: Uuid, period_start: DateTime<Utc>) -> (i64, i64) {
    sqlx::query_as::<_, (i64, i64)>(
        "SELECT bytes_in, bytes_out FROM traffic_usage WHERE subscription_id = $1 AND period_start = $2",
    )
    .bind(sub_id)
    .bind(period_start)
    .fetch_optional(pg)
    .await
    .ok()
    .flatten()
    .unwrap_or((0, 0))
}

/// 已结算 + Redis 待结算（尚未落 PG 的上报）。
async fn current_usage(
    pg: &PgPool,
    redis: Option<&redis::aio::ConnectionManager>,
    row: &SubRow,
    period_start: DateTime<Utc>,
) -> (i64, i64) {
    let (mut bytes_in, mut bytes_out) = settled_usage(pg, row.id, period_start).await;
    if let Some(redis) = redis {
        let tunnel_ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM tunnels WHERE user_id = $1")
            .bind(row.user_id)
            .fetch_all(pg)
            .await
            .unwrap_or_default();
        let (pending_in, pending_out) =
            crate::stats::redis_pending_settle(&mut redis.clone(), &tunnel_ids).await;
        bytes_in = bytes_in.saturating_add(pending_in);
        bytes_out = bytes_out.saturating_add(pending_out);
    }
    (bytes_in, bytes_out)
}

/// stats 入站路径调用：在写 Redis 待结算量之前调用，避免重载基线时重复计入本次上报。
pub async fn observe(
    pg: &PgPool,
    redis: Option<&redis::aio::ConnectionManager>,
    samples: &[TunnelTrafficSample],
) {
    let mut missing: Vec<Uuid> = samples
        .iter()
        .filter(|sample| sample.bytes_in > 0 || sample.bytes_out > 0)
        .map(|sample| sample.tunnel_id)
        .filter(|id| !TUNNEL_SUB.contains_key(id))
        .collect();
    missing.sort_unstable();
    missing.dedup();
    if !missing.is_empty() {
        load(pg, redis, &missing).await;
    }
    for sample in samples {
        if sample.bytes_in == 0 && sample.bytes_out == 0 {
            continue;
        }
        let Some(Some(sub_id)) = TUNNEL_SUB.get(&sample.tunnel_id).map(|entry| *entry) else {
            continue;
        };
        let Some(sub) = SUBS.get(&sub_id).map(|entry| entry.clone()) else {
            continue;
        };
        let bytes_in = i64::try_from(sample.bytes_in).unwrap_or(i64::MAX);
        let bytes_out = i64::try_from(sample.bytes_out).unwrap_or(i64::MAX);
        sub.bytes_in.fetch_add(bytes_in, Ordering::Relaxed);
        sub.bytes_out.fetch_add(bytes_out, Ordering::Relaxed);
        check(pg, redis, &sub);
    }
}

fn check(pg: &PgPool, redis: Option<&redis::aio::ConnectionManager>, sub: &Arc<LiveSub>) {
    let used = sub.mode.used(
        sub.bytes_in.load(Ordering::Relaxed),
        sub.bytes_out.load(Ordering::Relaxed),
    );
    if used < sub.quota || sub.tripped.swap(true, Ordering::SeqCst) {
        return;
    }
    info!(
        subscription_id = %sub.id,
        user_id = %sub.user_id,
        used,
        quota = sub.quota,
        "traffic quota exhausted; suspending user tunnels"
    );
    let pg = pg.clone();
    let redis = redis.cloned();
    let (sub_id, user_id, period_start) = (sub.id, sub.user_id, sub.period_start);
    tokio::spawn(async move {
        exhaust(&pg, redis, sub_id, user_id, period_start).await;
    });
}

async fn load(pg: &PgPool, redis: Option<&redis::aio::ConnectionManager>, tunnel_ids: &[Uuid]) {
    let _guard = crate::stats::settle_lock().await;
    let rows = match sqlx::query_as::<_, (Uuid, Option<Uuid>, Option<i64>)>(
        "SELECT t.id, s.id, s.traffic_quota_bytes FROM tunnels t LEFT JOIN LATERAL (SELECT s.id, s.traffic_quota_bytes FROM user_subscriptions s WHERE s.user_id = t.user_id AND s.status = 'active' AND s.starts_at <= now() AND (s.expires_at IS NULL OR s.expires_at > now()) AND s.exhausted_period_start IS NULL ORDER BY s.starts_at DESC LIMIT 1) s ON TRUE WHERE t.id = ANY($1)",
    )
    .bind(tunnel_ids)
    .fetch_all(pg)
    .await
    {
        Ok(rows) => rows,
        Err(error) => {
            warn!(%error, "quota: load tunnel subscriptions failed");
            return;
        }
    };
    let now = Utc::now();
    for (tunnel_id, sub_id, quota) in rows {
        let tracked = match (sub_id, quota) {
            (Some(sub_id), Some(quota)) if quota > 0 => Some(sub_id),
            _ => None,
        };
        if let Some(sub_id) = tracked {
            if !SUBS.contains_key(&sub_id) {
                if let Some(live) = load_sub(pg, redis, sub_id, now).await {
                    SUBS.insert(sub_id, live);
                }
            }
        }
        TUNNEL_SUB.insert(tunnel_id, tracked);
    }
}

async fn load_sub(
    pg: &PgPool,
    redis: Option<&redis::aio::ConnectionManager>,
    sub_id: Uuid,
    now: DateTime<Utc>,
) -> Option<Arc<LiveSub>> {
    let row = sqlx::query_as::<_, SubRow>(&format!(
        "SELECT {SUB_COLUMNS} FROM user_subscriptions s WHERE s.id = $1"
    ))
    .bind(sub_id)
    .fetch_optional(pg)
    .await
    .ok()
    .flatten()?;
    let quota = row.traffic_quota_bytes.filter(|quota| *quota > 0)?;
    let period_start = row.period_start(now);
    let (bytes_in, bytes_out) = current_usage(pg, redis, &row, period_start).await;
    let live = Arc::new(LiveSub {
        id: sub_id,
        user_id: row.user_id,
        quota,
        mode: CountMode::parse(&row.traffic_count_mode),
        period_start,
        bytes_in: AtomicI64::new(bytes_in),
        bytes_out: AtomicI64::new(bytes_out),
        tripped: AtomicBool::new(false),
    });
    check(pg, redis, &live);
    Some(live)
}

/// 标记订阅本周期耗尽，停用该用户全部运行中的隧道并通知 node/client 立即摘除。
pub async fn exhaust(
    pg: &PgPool,
    redis: Option<redis::aio::ConnectionManager>,
    sub_id: Uuid,
    user_id: Uuid,
    period_start: DateTime<Utc>,
) {
    let marked = sqlx::query(
        "UPDATE user_subscriptions SET exhausted_period_start = $2, updated_at = now() WHERE id = $1 AND exhausted_period_start IS NULL",
    )
    .bind(sub_id)
    .bind(period_start)
    .execute(pg)
    .await;
    if !matches!(marked, Ok(ref result) if result.rows_affected() > 0) {
        return;
    }
    invalidate_sub(sub_id);
    suspend_user_tunnels(pg, user_id).await;
    if let Some(mut redis) = redis {
        let _: Result<(), _> = redis::AsyncCommands::publish(
            &mut redis,
            format!("user:{user_id}:events"),
            r#"{"type":"quota_exhausted"}"#,
        )
        .await;
    }
}

/// 停用用户全部运行中的隧道（可重复调用；已停用的不受影响）。
async fn suspend_user_tunnels(pg: &PgPool, user_id: Uuid) {
    let tunnels: Vec<(Uuid, Uuid, Uuid)> = match sqlx::query_as(
        "UPDATE tunnels SET status = 'suspended', enabled = FALSE, last_error = $2, revision = revision + 1, updated_at = now() WHERE user_id = $1 AND enabled = TRUE AND status NOT IN ('deleted', 'suspended', 'pending_review') RETURNING id, node_id, client_id",
    )
    .bind(user_id)
    .bind(QUOTA_SUSPEND_REASON)
    .fetch_all(pg)
    .await
    {
        Ok(rows) => rows,
        Err(error) => {
            warn!(%user_id, %error, "quota: suspend tunnels failed");
            return;
        }
    };
    if !tunnels.is_empty() {
        info!(%user_id, count = tunnels.len(), "traffic quota exhausted; tunnels suspended");
    }
    for (tunnel_id, node_id, client_id) in tunnels {
        crate::ws::push_tunnel_suspend(pg, node_id, client_id, tunnel_id).await;
    }
}

/// 额度恢复（新周期 / 管理员调额或改已用量 / 换套餐）后，重新启用因配额被停用的隧道。
async fn restore_quota_suspended(pg: &PgPool, user_id: Uuid) {
    let tunnels: Vec<(Uuid, Uuid, Uuid)> = match sqlx::query_as(
        "UPDATE tunnels SET status = 'provisioning', enabled = TRUE, last_error = NULL, revision = revision + 1, updated_at = now() WHERE user_id = $1 AND status = 'suspended' AND last_error = $2 RETURNING id, node_id, client_id",
    )
    .bind(user_id)
    .bind(QUOTA_SUSPEND_REASON)
    .fetch_all(pg)
    .await
    {
        Ok(rows) => rows,
        Err(error) => {
            warn!(%user_id, %error, "quota: restore tunnels failed");
            return;
        }
    };
    if !tunnels.is_empty() {
        info!(%user_id, count = tunnels.len(), "traffic quota available again; tunnels restored");
    }
    for (tunnel_id, node_id, client_id) in tunnels {
        crate::ws::sync_tunnel_online(pg, node_id, client_id, tunnel_id).await;
    }
}

/// 按已结算 + 待结算用量重新判定用户当前订阅：跨周期 / 调额后解除耗尽，或补判超额。
pub async fn reevaluate_user(
    pg: &PgPool,
    redis: Option<redis::aio::ConnectionManager>,
    user_id: Uuid,
) {
    let row = sqlx::query_as::<_, SubRow>(&format!(
        "SELECT {SUB_COLUMNS} FROM user_subscriptions s WHERE s.user_id = $1 AND s.status = 'active' AND s.starts_at <= now() AND (s.expires_at IS NULL OR s.expires_at > now()) ORDER BY s.starts_at DESC LIMIT 1"
    ))
    .bind(user_id)
    .fetch_optional(pg)
    .await;
    let Ok(Some(row)) = row else {
        return;
    };
    invalidate_sub(row.id);
    let period_start = row.period_start(Utc::now());
    let (bytes_in, bytes_out) = {
        let _guard = crate::stats::settle_lock().await;
        current_usage(pg, redis.as_ref(), &row, period_start).await
    };
    let used = CountMode::parse(&row.traffic_count_mode).used(bytes_in, bytes_out);
    let over = row.traffic_quota_bytes.is_some_and(|quota| quota > 0 && used >= quota);
    let still_exhausted = row
        .exhausted_period_start
        .is_some_and(|exhausted_at| exhausted_at >= period_start && over);
    if still_exhausted {
        suspend_user_tunnels(pg, user_id).await;
        return;
    }
    if row.exhausted_period_start.is_some() {
        let _ = sqlx::query(
            "UPDATE user_subscriptions SET exhausted_period_start = NULL, updated_at = now() WHERE id = $1",
        )
        .bind(row.id)
        .execute(pg)
        .await;
    }
    if over {
        exhaust(pg, redis, row.id, user_id, period_start).await;
    } else {
        restore_quota_suspended(pg, user_id).await;
    }
}

fn invalidate_sub(sub_id: Uuid) {
    SUBS.remove(&sub_id);
    TUNNEL_SUB.retain(|_, tracked| *tracked != Some(sub_id));
}

/// 60s 结算后的兜底巡检：跨周期解除、补判漏网的超额。
pub async fn sweep(pg: &PgPool, redis: Option<redis::aio::ConnectionManager>) {
    let users: Vec<Uuid> = match sqlx::query_scalar(
        "SELECT DISTINCT user_id FROM user_subscriptions WHERE status = 'active' AND (traffic_quota_bytes IS NOT NULL OR exhausted_period_start IS NOT NULL)",
    )
    .fetch_all(pg)
    .await
    {
        Ok(users) => users,
        Err(_) => return,
    };
    for user_id in users {
        reevaluate_user(pg, redis.clone(), user_id).await;
    }
}

/// 用户当前订阅本周期流量已耗尽（启动 / 新建隧道前校验）。
pub async fn user_quota_exhausted(pg: &PgPool, user_id: Uuid) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT COALESCE((SELECT exhausted_period_start IS NOT NULL FROM user_subscriptions WHERE user_id = $1 AND status = 'active' AND starts_at <= now() AND (expires_at IS NULL OR expires_at > now()) ORDER BY starts_at DESC LIMIT 1), FALSE)",
    )
    .bind(user_id)
    .fetch_one(pg)
    .await
}
