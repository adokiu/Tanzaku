use chrono::Utc;
use dashmap::DashMap;
use sqlx::PgPool;
use std::sync::{Arc, LazyLock, OnceLock};
use tokio::sync::{Mutex, mpsc};
use tracing::warn;
use tz_proto::{StatsReport, TunnelTrafficSample};
use uuid::Uuid;

const STATS_INGRESS_CAP: usize = 16_384;
/// 未落 PG 的隧道流量在 Redis 聚合，最多间隔该秒数批量写入 PG。
const PG_SETTLE_INTERVAL_SECS: u64 = 60;

const REDIS_SETTLE_TUNNELS: &str = "stats:settle:tunnel_ids";
const REDIS_SETTLE_BYTES_IN: &str = "stats:settle:bytes_in";
const REDIS_SETTLE_BYTES_OUT: &str = "stats:settle:bytes_out";

/// 快照当前待落库增量（不删除；PG 成功后再 confirm）。
const REDIS_SNAPSHOT_SETTLE_LUA: &str = r#"
local ids = redis.call('SMEMBERS', KEYS[1])
local out = {}
for _, tid in ipairs(ids) do
  local inn = redis.call('HGET', KEYS[2], tid)
  local ouu = redis.call('HGET', KEYS[3], tid)
  if inn or ouu then
    out[#out + 1] = tid
    out[#out + 1] = inn or '0'
    out[#out + 1] = ouu or '0'
  end
end
return out
"#;

const REDIS_CONFIRM_SETTLE_LUA: &str = r#"
local n = #ARGV
for i = 1, n, 3 do
  local tid = ARGV[i]
  local inn = tonumber(ARGV[i + 1]) or 0
  local ouu = tonumber(ARGV[i + 2]) or 0
  if inn ~= 0 then
    redis.call('HINCRBY', KEYS[1], tid, -inn)
  end
  if ouu ~= 0 then
    redis.call('HINCRBY', KEYS[2], tid, -ouu)
  end
  local left_in = tonumber(redis.call('HGET', KEYS[1], tid)) or 0
  local left_out = tonumber(redis.call('HGET', KEYS[2], tid)) or 0
  if left_in <= 0 and left_out <= 0 then
    redis.call('HDEL', KEYS[1], tid)
    redis.call('HDEL', KEYS[2], tid)
    redis.call('SREM', KEYS[3], tid)
  end
end
return n / 3
"#;

struct StatsIngressJob {
    node_id: Uuid,
    report: StatsReport,
}

static STATS_INGRESS_TX: OnceLock<mpsc::Sender<StatsIngressJob>> = OnceLock::new();

static PENDING: LazyLock<DashMap<Uuid, (i64, i64)>> = LazyLock::new(DashMap::new);
static PENDING_ACK: LazyLock<DashMap<Uuid, u64>> = LazyLock::new(DashMap::new);
static COMMITTED_SEQ: LazyLock<DashMap<Uuid, i64>> = LazyLock::new(DashMap::new);
static INGESTED_SEQ: LazyLock<DashMap<Uuid, u64>> = LazyLock::new(DashMap::new);
static NODE_LAST_PERIOD_END: LazyLock<DashMap<Uuid, u64>> = LazyLock::new(DashMap::new);
static SETTLE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

pub(crate) async fn settle_lock() -> tokio::sync::MutexGuard<'static, ()> {
    SETTLE_LOCK.lock().await
}

/// 指定隧道在 Redis 中尚未落 PG 的待结算字节（in, out）合计。
pub(crate) async fn redis_pending_settle(
    redis: &mut redis::aio::ConnectionManager,
    tunnel_ids: &[Uuid],
) -> (i64, i64) {
    redis_pending_settle_each(redis, tunnel_ids)
        .await
        .into_iter()
        .fold((0, 0), |(total_in, total_out), (bytes_in, bytes_out)| {
            (total_in + bytes_in, total_out + bytes_out)
        })
}

/// 与 `tunnel_ids` 一一对应的待结算字节（in, out）。
pub(crate) async fn redis_pending_settle_each(
    redis: &mut redis::aio::ConnectionManager,
    tunnel_ids: &[Uuid],
) -> Vec<(i64, i64)> {
    if tunnel_ids.is_empty() {
        return Vec::new();
    }
    let fields: Vec<String> = tunnel_ids.iter().map(Uuid::to_string).collect();
    let bytes_in: Vec<Option<i64>> = redis::cmd("HMGET")
        .arg(REDIS_SETTLE_BYTES_IN)
        .arg(&fields)
        .query_async(redis)
        .await
        .unwrap_or_default();
    let bytes_out: Vec<Option<i64>> = redis::cmd("HMGET")
        .arg(REDIS_SETTLE_BYTES_OUT)
        .arg(&fields)
        .query_async(redis)
        .await
        .unwrap_or_default();
    let positive = |value: Option<&Option<i64>>| value.copied().flatten().unwrap_or(0).max(0);
    (0..tunnel_ids.len())
        .map(|index| (positive(bytes_in.get(index)), positive(bytes_out.get(index))))
        .collect()
}

/// 管理员覆盖已用流量时丢弃这些隧道尚未落库的增量（调用方需持有 `settle_lock`）。
pub(crate) async fn discard_pending_settle(
    redis: Option<&mut redis::aio::ConnectionManager>,
    tunnel_ids: &[Uuid],
) {
    if tunnel_ids.is_empty() {
        return;
    }
    clear_pending_tunnels(tunnel_ids);
    let Some(redis) = redis else {
        return;
    };
    let fields: Vec<String> = tunnel_ids.iter().map(Uuid::to_string).collect();
    let result: Result<(), _> = redis::pipe()
        .cmd("HDEL")
        .arg(REDIS_SETTLE_BYTES_IN)
        .arg(&fields)
        .ignore()
        .cmd("HDEL")
        .arg(REDIS_SETTLE_BYTES_OUT)
        .arg(&fields)
        .ignore()
        .cmd("SREM")
        .arg(REDIS_SETTLE_TUNNELS)
        .arg(&fields)
        .ignore()
        .query_async(redis)
        .await;
    if let Err(error) = result {
        warn!(%error, "redis discard pending settle failed");
    }
}

pub fn seed_committed_seq(node_id: Uuid, seq: i64) {
    COMMITTED_SEQ.insert(node_id, seq);
}

/// 节点重连后清空本连接内的 seq 去重，避免旧游标挡住新序号。
pub fn reset_ingress_session(node_id: Uuid) {
    INGESTED_SEQ.remove(&node_id);
    PENDING_ACK.remove(&node_id);
    NODE_LAST_PERIOD_END.remove(&node_id);
}

pub fn committed_seq_cached(node_id: Uuid) -> i64 {
    COMMITTED_SEQ.get(&node_id).map(|entry| *entry).unwrap_or(0)
}

pub fn schedule_fast_stats_ack(node_id: Uuid, seq: u64) {
    let mut should_ack = false;
    COMMITTED_SEQ
        .entry(node_id)
        .and_modify(|current| {
            if seq as i64 > *current {
                *current = seq as i64;
                should_ack = true;
            }
        })
        .or_insert_with(|| {
            should_ack = true;
            seq as i64
        });
    if should_ack {
        tokio::spawn(async move {
            crate::ws::notify_stats_ack(node_id, seq).await;
        });
    }
}

pub fn ingest(node_id: Uuid, report: &StatsReport, committed_seq: i64) -> bool {
    let watermark = committed_seq_cached(node_id).max(committed_seq);
    if report.seq <= watermark as u64 {
        return false;
    }
    if INGESTED_SEQ
        .get(&node_id)
        .is_some_and(|seen| report.seq <= *seen)
    {
        return false;
    }
    INGESTED_SEQ.insert(node_id, report.seq);
    PENDING_ACK
        .entry(node_id)
        .and_modify(|seq| *seq = (*seq).max(report.seq))
        .or_insert(report.seq);
    for sample in &report.tunnels {
        PENDING
            .entry(sample.tunnel_id)
            .and_modify(|(in_bytes, out_bytes)| {
                *in_bytes += i64::try_from(sample.bytes_in).unwrap_or(i64::MAX);
                *out_bytes += i64::try_from(sample.bytes_out).unwrap_or(i64::MAX);
            })
            .or_insert((
                i64::try_from(sample.bytes_in).unwrap_or(i64::MAX),
                i64::try_from(sample.bytes_out).unwrap_or(i64::MAX),
            ));
    }
    true
}

pub async fn committed_seq(pg: &PgPool, node_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT committed_seq FROM node_stats_cursors WHERE node_id = $1",
    )
    .bind(node_id)
    .fetch_optional(pg)
    .await
    .ok()
    .flatten()
    .unwrap_or(0)
}

pub async fn ensure_cursor_row(pg: &PgPool, node_id: Uuid) {
    let _ = sqlx::query(
        "INSERT INTO node_stats_cursors (node_id, committed_seq) VALUES ($1, 0) ON CONFLICT (node_id) DO NOTHING",
    )
    .bind(node_id)
    .execute(pg)
    .await;
}

pub async fn redis_incr(
    redis: &mut redis::aio::ConnectionManager,
    tunnel_id: Uuid,
    sample: &TunnelTrafficSample,
) {
    let key = format!("tunnel:{tunnel_id}:live");
    let _: Result<(), _> = redis::AsyncCommands::hincr(
        redis,
        &key,
        "bytes_in",
        i64::try_from(sample.bytes_in).unwrap_or(0),
    )
    .await;
    let _: Result<(), _> = redis::AsyncCommands::hincr(
        redis,
        &key,
        "bytes_out",
        i64::try_from(sample.bytes_out).unwrap_or(0),
    )
    .await;
    let _: Result<(), _> = redis::AsyncCommands::hset(
        redis,
        &key,
        "active_conns",
        i64::from(sample.active_conns),
    )
    .await;
    let _: Result<(), _> = redis::AsyncCommands::hincr(
        redis,
        &key,
        "rejects_quota",
        i64::from(sample.rejects_quota),
    )
    .await;
    let _: Result<(), _> = redis::AsyncCommands::hincr(
        redis,
        &key,
        "rejects_guard",
        i64::from(sample.rejects_guard),
    )
    .await;
    let _: Result<(), _> = redis::AsyncCommands::expire(redis, &key, 120).await;
}

async fn redis_accrue_settle(
    redis: &mut redis::aio::ConnectionManager,
    sample: &TunnelTrafficSample,
) {
    let tunnel_key = sample.tunnel_id.to_string();
    let bytes_in = i64::try_from(sample.bytes_in).unwrap_or(0);
    let bytes_out = i64::try_from(sample.bytes_out).unwrap_or(0);
    if bytes_in == 0 && bytes_out == 0 {
        return;
    }
    let _: Result<(), _> = redis::AsyncCommands::hincr(redis, REDIS_SETTLE_BYTES_IN, &tunnel_key, bytes_in).await;
    let _: Result<(), _> = redis::AsyncCommands::hincr(redis, REDIS_SETTLE_BYTES_OUT, &tunnel_key, bytes_out).await;
    let _: Result<(), _> = redis::AsyncCommands::sadd(redis, REDIS_SETTLE_TUNNELS, &tunnel_key).await;
}

fn parse_settle_snapshot(raw: Vec<String>) -> Vec<(Uuid, i64, i64)> {
    let mut snapshot = Vec::new();
    for chunk in raw.chunks_exact(3) {
        let Ok(tunnel_id) = Uuid::parse_str(&chunk[0]) else {
            continue;
        };
        let bytes_in = chunk[1].parse::<i64>().unwrap_or(0);
        let bytes_out = chunk[2].parse::<i64>().unwrap_or(0);
        if bytes_in == 0 && bytes_out == 0 {
            continue;
        }
        snapshot.push((tunnel_id, bytes_in, bytes_out));
    }
    snapshot
}

async fn redis_snapshot_settle(
    redis: &mut redis::aio::ConnectionManager,
) -> Vec<(Uuid, i64, i64)> {
    let raw: Result<Vec<String>, _> = redis::Script::new(REDIS_SNAPSHOT_SETTLE_LUA)
        .key(REDIS_SETTLE_TUNNELS)
        .key(REDIS_SETTLE_BYTES_IN)
        .key(REDIS_SETTLE_BYTES_OUT)
        .invoke_async(redis)
        .await;
    raw.map(parse_settle_snapshot).unwrap_or_default()
}

async fn redis_confirm_settle(
    redis: &mut redis::aio::ConnectionManager,
    snapshot: &[(Uuid, i64, i64)],
) {
    if snapshot.is_empty() {
        return;
    }
    let script = redis::Script::new(REDIS_CONFIRM_SETTLE_LUA);
    let mut invocation = script.prepare_invoke();
    invocation
        .key(REDIS_SETTLE_BYTES_IN)
        .key(REDIS_SETTLE_BYTES_OUT)
        .key(REDIS_SETTLE_TUNNELS);
    for (tunnel_id, bytes_in, bytes_out) in snapshot {
        invocation
            .arg(tunnel_id.to_string())
            .arg(*bytes_in)
            .arg(*bytes_out);
    }
    if let Err(error) = invocation.invoke_async::<i64>(redis).await {
        warn!(%error, "redis settle confirm failed; pending bytes may be settled twice");
    }
}

pub fn pending_traffic_bytes(tunnel_id: Uuid) -> (i64, i64) {
    PENDING
        .get(&tunnel_id)
        .map(|entry| (entry.value().0, entry.value().1))
        .unwrap_or((0, 0))
}

pub fn pending_traffic_total() -> (i64, i64) {
    let mut bytes_in = 0i64;
    let mut bytes_out = 0i64;
    for entry in PENDING.iter() {
        bytes_in = bytes_in.saturating_add(entry.value().0);
        bytes_out = bytes_out.saturating_add(entry.value().1);
    }
    (bytes_in, bytes_out)
}

pub fn spawn_ingress_worker(state: Arc<crate::setup::AppState>) {
    let (tx, mut rx) = mpsc::channel(STATS_INGRESS_CAP);
    let _ = STATS_INGRESS_TX.set(tx);
    tokio::spawn(async move {
        while let Some(job) = rx.recv().await {
            process_stats_report(state.clone(), job.node_id, &job.report).await;
            while let Ok(next) = rx.try_recv() {
                process_stats_report(state.clone(), next.node_id, &next.report).await;
            }
        }
    });
}

pub fn enqueue_stats_report(node_id: Uuid, report: StatsReport) {
    let Some(tx) = STATS_INGRESS_TX.get() else {
        return;
    };
    let job = StatsIngressJob { node_id, report };
    match tx.try_send(job) {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Full(job)) => {
            warn!(%node_id, seq = job.report.seq, "stats ingress queue full; skipping redis live counters");
            let cursor = committed_seq_cached(job.node_id);
            if ingest(job.node_id, &job.report, cursor) {
                schedule_fast_stats_ack(job.node_id, job.report.seq);
            }
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {}
    }
}

fn note_period_continuity(node_id: Uuid, report: &StatsReport) {
    if report.period_end_unix == 0 {
        return;
    }
    if let Some(last_end) = NODE_LAST_PERIOD_END.get(&node_id) {
        if report.period_start_unix != *last_end && *last_end != 0 {
            warn!(
                %node_id,
                seq = report.seq,
                expected_period_start = *last_end,
                actual_period_start = report.period_start_unix,
                period_end = report.period_end_unix,
                "node stats period gap (bytes may be missing or merged on node)"
            );
        }
    }
    NODE_LAST_PERIOD_END.insert(node_id, report.period_end_unix);
}

async fn process_stats_report(
    state: Arc<crate::setup::AppState>,
    node_id: Uuid,
    report: &StatsReport,
) {
    let cursor = committed_seq_cached(node_id);
    if !ingest(node_id, report, cursor) {
        let watermark = u64::try_from(cursor.max(0)).unwrap_or(0);
        if report.seq <= watermark {
            warn!(
                %node_id,
                seq = report.seq,
                watermark,
                "stats report below committed watermark; dropped (node seq behind board)"
            );
            tokio::spawn(async move {
                crate::ws::notify_stats_ack(node_id, watermark).await;
            });
        }
        return;
    }
    note_period_continuity(node_id, report);
    schedule_fast_stats_ack(node_id, report.seq);
    let database = state.ready_database();
    let report_interval_secs = if report.interval_secs > 0 {
        report.interval_secs
    } else if let Some((pg, _)) = &database {
        crate::system_settings::host_metrics_interval_secs(pg).await
    } else {
        1
    };
    for sample in &report.tunnels {
        crate::tunnel_traffic::ingest(
            sample.tunnel_id,
            sample.bytes_in,
            sample.bytes_out,
            report_interval_secs,
        );
    }
    if let Some((pg, redis)) = state.ready_database() {
        crate::quota::observe(&pg, redis.as_ref(), &report.tunnels).await;
    }
    if let Some(mut redis) = state.redis_connection() {
        for sample in &report.tunnels {
            redis_incr(&mut redis, sample.tunnel_id, sample).await;
            redis_accrue_settle(&mut redis, sample).await;
        }
    }
}

fn clear_pending_tunnels(tunnel_ids: &[Uuid]) {
    for tunnel_id in tunnel_ids {
        PENDING.remove(tunnel_id);
    }
}

pub fn spawn_settlement_task(state: Arc<crate::setup::AppState>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let Some((pg, redis)) = state.ready_database() else {
                continue;
            };
            let pg = Arc::new(pg.clone());
            let redis = redis.clone();
            crate::quota::sweep(&pg, redis.clone()).await;
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(PG_SETTLE_INTERVAL_SECS));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            interval.tick().await;
            loop {
                interval.tick().await;
                if let Err(error) = settle_once(&pg, redis.clone()).await {
                    warn!(?error, "stats settlement failed");
                }
            }
        }
    });
}

async fn settle_once(
    pg: &PgPool,
    mut redis: Option<redis::aio::ConnectionManager>,
) -> Result<(), sqlx::Error> {
    let _guard = SETTLE_LOCK.lock().await;
    let mut snapshot: Vec<(Uuid, i64, i64)> = Vec::new();
    let mut from_redis = false;
    if let Some(mut connection) = redis.clone() {
        snapshot = redis_snapshot_settle(&mut connection).await;
        from_redis = !snapshot.is_empty();
    }
    if snapshot.is_empty() && !PENDING.is_empty() {
        snapshot = PENDING
            .iter()
            .map(|entry| (*entry.key(), entry.value().0, entry.value().1))
            .collect();
    }
    if snapshot.is_empty() {
        return Ok(());
    }
    let flushed_ids: Vec<Uuid> = snapshot.iter().map(|(id, _, _)| *id).collect();
    let mut transaction = pg.begin().await?;
    for (tunnel_id, bytes_in, bytes_out) in &snapshot {
        if *bytes_in == 0 && *bytes_out == 0 {
            continue;
        }
        sqlx::query(
            "INSERT INTO tunnel_traffic_hourly (tunnel_id, hour_start, bytes_in, bytes_out) VALUES ($1, date_trunc('hour', now()), $2, $3) ON CONFLICT (tunnel_id, hour_start) DO UPDATE SET bytes_in = tunnel_traffic_hourly.bytes_in + EXCLUDED.bytes_in, bytes_out = tunnel_traffic_hourly.bytes_out + EXCLUDED.bytes_out",
        )
        .bind(tunnel_id)
        .bind(bytes_in)
        .bind(bytes_out)
        .execute(&mut *transaction)
        .await?;
        accrue_subscription_traffic(&mut transaction, *tunnel_id, *bytes_in, *bytes_out)
            .await?;
    }
    transaction.commit().await?;
    if from_redis {
        if let Some(ref mut connection) = redis {
            redis_confirm_settle(connection, &snapshot).await;
        }
    }
    clear_pending_tunnels(&flushed_ids);
    crate::quota::invalidate();
    let acks: Vec<(Uuid, u64)> = PENDING_ACK
        .iter()
        .map(|entry| (*entry.key(), *entry.value()))
        .collect();
    for (node_id, seq) in acks {
        ack_seq(pg, node_id, seq).await;
        seed_committed_seq(node_id, seq as i64);
        INGESTED_SEQ.remove(&node_id);
        PENDING_ACK.remove(&node_id);
    }
    drop(_guard);
    let pg = pg.clone();
    tokio::spawn(async move {
        crate::quota::sweep(&pg, redis).await;
    });
    Ok(())
}

async fn accrue_subscription_traffic(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tunnel_id: Uuid,
    bytes_in: i64,
    bytes_out: i64,
) -> Result<(), sqlx::Error> {
    if bytes_in == 0 && bytes_out == 0 {
        return Ok(());
    }
    #[derive(sqlx::FromRow)]
    struct SubRow {
        subscription_id: Uuid,
        traffic_period: String,
        starts_at: chrono::DateTime<Utc>,
        period_anchor: i16,
    }
    let sub = sqlx::query_as::<_, SubRow>(
        "SELECT s.id AS subscription_id, s.traffic_period, s.starts_at, s.period_anchor FROM tunnels t JOIN user_subscriptions s ON s.user_id = t.user_id WHERE t.id = $1 AND s.status = 'active' AND s.starts_at <= now() AND (s.expires_at IS NULL OR s.expires_at > now()) ORDER BY s.starts_at DESC LIMIT 1",
    )
    .bind(tunnel_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(sub) = sub else {
        return Ok(());
    };
    let period_start = crate::subscription_period::subscription_period_start(
        &sub.traffic_period,
        sub.starts_at,
        sub.period_anchor,
        Utc::now(),
    );
    sqlx::query(
        "INSERT INTO traffic_usage (subscription_id, period_start, bytes_in, bytes_out, updated_at) VALUES ($1, $2, $3, $4, now()) ON CONFLICT (subscription_id, period_start) DO UPDATE SET bytes_in = traffic_usage.bytes_in + EXCLUDED.bytes_in, bytes_out = traffic_usage.bytes_out + EXCLUDED.bytes_out, updated_at = now()",
    )
    .bind(sub.subscription_id)
    .bind(period_start)
    .bind(bytes_in)
    .bind(bytes_out)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub async fn ack_seq(pg: &PgPool, node_id: Uuid, seq: u64) {
    seed_committed_seq(node_id, seq as i64);
    let _ = sqlx::query(
        "INSERT INTO node_stats_cursors (node_id, committed_seq, updated_at) VALUES ($1, $2, now()) ON CONFLICT (node_id) DO UPDATE SET committed_seq = GREATEST(node_stats_cursors.committed_seq, EXCLUDED.committed_seq), updated_at = now()",
    )
    .bind(node_id)
    .bind(seq as i64)
    .execute(pg)
    .await;
}
