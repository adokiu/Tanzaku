use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tz_proto::TunnelTrafficSample;
use uuid::Uuid;

/// 阻塞到下一个对齐周期边界，返回刚结束的区间 `[start, end)`（Unix 秒）。
pub async fn wait_closed_period(interval_secs: u64) -> (u64, u64) {
    let interval_secs = interval_secs.max(1);
    let interval_ms = interval_secs * 1000;
    let now_ms = unix_millis_now();
    let period_end_ms = (now_ms / interval_ms + 1) * interval_ms;
    loop {
        let now_ms = unix_millis_now();
        if now_ms >= period_end_ms {
            break;
        }
        tokio::time::sleep(Duration::from_millis(period_end_ms - now_ms)).await;
    }
    let period_end = period_end_ms / 1000;
    (period_end - interval_secs, period_end)
}

fn unix_millis_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

pub fn merge_traffic_samples(samples: Vec<TunnelTrafficSample>) -> Vec<TunnelTrafficSample> {
    let mut merged: std::collections::HashMap<Uuid, TunnelTrafficSample> =
        std::collections::HashMap::new();
    for sample in samples {
        merged
            .entry(sample.tunnel_id)
            .and_modify(|existing| merge_sample(existing, &sample))
            .or_insert(sample);
    }
    merged.into_values().collect()
}

fn merge_sample(target: &mut TunnelTrafficSample, source: &TunnelTrafficSample) {
    target.bytes_in = target.bytes_in.saturating_add(source.bytes_in);
    target.bytes_out = target.bytes_out.saturating_add(source.bytes_out);
    target.rejects_quota = target.rejects_quota.saturating_add(source.rejects_quota);
    target.rejects_guard = target.rejects_guard.saturating_add(source.rejects_guard);
    target.active_conns = target.active_conns.max(source.active_conns);
}
