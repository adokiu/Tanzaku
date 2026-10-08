use dashmap::DashMap;
use redis::AsyncCommands;
use std::sync::LazyLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tz_proto::HostMetricsReport;
use uuid::Uuid;

const LIVE_TTL_SECS: u64 = 120;

#[derive(Debug, Clone, serde::Serialize)]
pub struct HostMetricsSnapshot {
    pub cpu_usage_percent: u8,
    pub cpu_cores: u16,
    pub memory_used_bytes: u64,
    pub memory_total_bytes: u64,
    pub net_up_bps: u64,
    pub net_down_bps: u64,
    pub net_rx_bytes_total: u64,
    pub net_tx_bytes_total: u64,
    pub connections_tcp: u32,
    pub connections_udp: u32,
    pub reported_at_ms: u64,
}

struct StoredMetrics {
    snapshot: HostMetricsSnapshot,
    #[allow(dead_code)]
    received: Instant,
}

static LIVE: LazyLock<DashMap<Uuid, StoredMetrics>> = LazyLock::new(DashMap::new);

pub fn ingest(node_id: Uuid, report: &HostMetricsReport) {
    let snapshot = snapshot_from_report(report);
    LIVE.insert(
        node_id,
        StoredMetrics {
            snapshot,
            received: Instant::now(),
        },
    );
}

/// 覆盖 `node:{id}:live`。连接数是瞬时值，禁止 HINCRBY。
pub async fn persist_redis(redis: &mut redis::aio::ConnectionManager, node_id: Uuid, report: &HostMetricsReport) {
    let snapshot = snapshot_from_report(report);
    let key = format!("node:{node_id}:live");
    let _: Result<(), _> = redis::cmd("HSET")
        .arg(&key)
        .arg("cpu_usage_percent")
        .arg(snapshot.cpu_usage_percent)
        .arg("cpu_cores")
        .arg(snapshot.cpu_cores)
        .arg("memory_used_bytes")
        .arg(snapshot.memory_used_bytes)
        .arg("memory_total_bytes")
        .arg(snapshot.memory_total_bytes)
        .arg("net_up_bps")
        .arg(snapshot.net_up_bps)
        .arg("net_down_bps")
        .arg(snapshot.net_down_bps)
        .arg("connections_tcp")
        .arg(snapshot.connections_tcp)
        .arg("connections_udp")
        .arg(snapshot.connections_udp)
        .query_async::<()>(redis)
        .await;
    let _: Result<(), _> = redis::AsyncCommands::expire(redis, &key, LIVE_TTL_SECS as i64).await;
}

pub async fn clear_redis(redis: &mut redis::aio::ConnectionManager, node_id: Uuid) {
    let key = format!("node:{node_id}:live");
    let _: Result<(), _> = redis::AsyncCommands::del(redis, &key).await;
}

fn snapshot_from_report(report: &HostMetricsReport) -> HostMetricsSnapshot {
    let reported_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    HostMetricsSnapshot {
        cpu_usage_percent: report.cpu_usage_percent.min(100),
        cpu_cores: report.cpu_cores.max(1),
        memory_used_bytes: report.memory_used_bytes,
        memory_total_bytes: report.memory_total_bytes,
        net_up_bps: report.net_up_bps,
        net_down_bps: report.net_down_bps,
        net_rx_bytes_total: report.net_rx_bytes_total,
        net_tx_bytes_total: report.net_tx_bytes_total,
        connections_tcp: report.connections_tcp,
        connections_udp: report.connections_udp,
        reported_at_ms,
    }
}

pub fn snapshot(node_id: Uuid) -> Option<HostMetricsSnapshot> {
    LIVE.get(&node_id).map(|entry| entry.snapshot.clone())
}

pub fn clear(node_id: Uuid) {
    LIVE.remove(&node_id);
}
