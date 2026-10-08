use dashmap::DashMap;
use std::sync::LazyLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tz_proto::ClientTrafficReport;
use uuid::Uuid;

#[derive(Debug, Clone, serde::Serialize)]
pub struct ClientTrafficSnapshot {
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub in_bps: u64,
    pub out_bps: u64,
    pub reported_at_ms: u64,
}

struct StoredTraffic {
    snapshot: ClientTrafficSnapshot,
    #[allow(dead_code)]
    received: Instant,
}

static LIVE: LazyLock<DashMap<Uuid, StoredTraffic>> = LazyLock::new(DashMap::new);

/// Client 自报流量，仅供管理端 Client 列表展示；计费与配额以 Node Stats 为准。
pub fn ingest(client_id: Uuid, report: &ClientTrafficReport) {
    let reported_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    LIVE.insert(
        client_id,
        StoredTraffic {
            snapshot: ClientTrafficSnapshot {
                bytes_in: report.bytes_in,
                bytes_out: report.bytes_out,
                in_bps: report.bytes_in_delta,
                out_bps: report.bytes_out_delta,
                reported_at_ms,
            },
            received: Instant::now(),
        },
    );
}

pub fn snapshot(client_id: Uuid) -> Option<ClientTrafficSnapshot> {
    LIVE.get(&client_id).map(|entry| entry.snapshot.clone())
}

pub fn clear(client_id: Uuid) {
    LIVE.remove(&client_id);
}
