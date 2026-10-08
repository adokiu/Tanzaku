use std::collections::VecDeque;

use tz_proto::{StatsReport, TunnelTrafficSample};
use uuid::Uuid;

/// 断连期间按 `interval_secs` 积压在内存，重连后按 seq 顺序补发。
const MAX_PENDING: usize = 3600;

pub struct StatsOutbox {
    pending: VecDeque<StatsReport>,
    last_ack: u64,
}

impl StatsOutbox {
    pub fn new() -> Self {
        Self {
            pending: VecDeque::new(),
            last_ack: 0,
        }
    }

    pub fn acknowledge(&mut self, seq: u64) {
        if seq > self.last_ack {
            self.last_ack = seq;
        }
        while self
            .pending
            .front()
            .is_some_and(|report| report.seq <= self.last_ack)
        {
            self.pending.pop_front();
        }
    }

    pub fn enqueue(&mut self, report: StatsReport) {
        self.pending.push_back(report);
        while self.pending.len() > MAX_PENDING {
            self.compact_oldest_pair();
        }
    }

    pub fn pending_reports(&self) -> impl Iterator<Item = &StatsReport> {
        self.pending.iter()
    }

    fn compact_oldest_pair(&mut self) {
        if self.pending.len() < 2 {
            return;
        }
        let first = self.pending.pop_front().expect("len checked");
        let second = self.pending.pop_front().expect("len checked");
        self.pending.push_front(merge_reports(first, second));
    }
}

fn merge_reports(left: StatsReport, right: StatsReport) -> StatsReport {
    let seq_from = left.seq_from.unwrap_or(left.seq).min(right.seq_from.unwrap_or(right.seq));
    let seq_to = right.seq_to.unwrap_or(right.seq).max(left.seq_to.unwrap_or(left.seq));
    let mut merged: std::collections::HashMap<Uuid, TunnelTrafficSample> =
        std::collections::HashMap::new();
    for sample in left.tunnels.into_iter().chain(right.tunnels) {
        merged
            .entry(sample.tunnel_id)
            .and_modify(|existing| merge_sample(existing, &sample))
            .or_insert(sample);
    }
    StatsReport {
        seq: right.seq,
        seq_from: Some(seq_from),
        seq_to: Some(seq_to),
        period_start_unix: left.period_start_unix,
        period_end_unix: right.period_end_unix,
        interval_secs: left.interval_secs.max(right.interval_secs),
        tunnels: merged.into_values().collect(),
    }
}

fn merge_sample(target: &mut TunnelTrafficSample, source: &TunnelTrafficSample) {
    target.bytes_in = target.bytes_in.saturating_add(source.bytes_in);
    target.bytes_out = target.bytes_out.saturating_add(source.bytes_out);
    target.rejects_quota = target.rejects_quota.saturating_add(source.rejects_quota);
    target.rejects_guard = target.rejects_guard.saturating_add(source.rejects_guard);
    target.active_conns = target.active_conns.max(source.active_conns);
}
