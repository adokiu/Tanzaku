use dashmap::DashMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

#[derive(Debug, Clone, serde::Serialize)]
pub struct TunnelTrafficSnapshot {
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub in_bps: u64,
    pub out_bps: u64,
    pub reported_at_ms: u64,
}

struct StoredTraffic {
    snapshot: TunnelTrafficSnapshot,
    received: Instant,
    interval_secs: u32,
}

static LIVE: LazyLock<DashMap<Uuid, StoredTraffic>> = LazyLock::new(DashMap::new);

/// Node Stats 周期增量；`report_interval_secs` 与 `node_host_metrics_interval_secs` 一致，用于换算 bps。
pub fn ingest(tunnel_id: Uuid, bytes_in: u64, bytes_out: u64, report_interval_secs: u32) {
    let reported_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    let secs = u64::from(report_interval_secs.max(1));
    LIVE.insert(
        tunnel_id,
        StoredTraffic {
            snapshot: TunnelTrafficSnapshot {
                bytes_in,
                bytes_out,
                in_bps: bytes_in.saturating_mul(8) / secs,
                out_bps: bytes_out.saturating_mul(8) / secs,
                reported_at_ms,
            },
            received: Instant::now(),
            interval_secs: report_interval_secs.max(1),
        },
    );
}

/// 节点空闲时不上报全零样本，超过两个周期未收到样本即视为带宽归零。
pub fn live_totals() -> (u64, u64) {
    let mut in_bps = 0u64;
    let mut out_bps = 0u64;
    for entry in LIVE.iter() {
        let stale_after = Duration::from_secs(u64::from(entry.interval_secs) * 2 + 1);
        if entry.received.elapsed() > stale_after {
            continue;
        }
        in_bps = in_bps.saturating_add(entry.snapshot.in_bps);
        out_bps = out_bps.saturating_add(entry.snapshot.out_bps);
    }
    (in_bps, out_bps)
}

pub fn snapshot(tunnel_id: Uuid) -> Option<TunnelTrafficSnapshot> {
    let entry = LIVE.get(&tunnel_id)?;
    let stale_after = Duration::from_secs(u64::from(entry.interval_secs) * 2 + 1);
    let mut snapshot = entry.snapshot.clone();
    if entry.received.elapsed() > stale_after {
        snapshot.in_bps = 0;
        snapshot.out_bps = 0;
    }
    Some(snapshot)
}
