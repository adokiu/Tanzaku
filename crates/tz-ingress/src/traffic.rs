use dashmap::DashMap;
use std::collections::HashSet;
use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    Arc,
};

use tz_net::meter::TrafficMeter;
use tz_proto::TunnelTrafficSample;
use uuid::Uuid;

#[derive(Debug)]
pub struct TunnelTrafficAccounting {
    pub meter: TrafficMeter,
    rejects_quota: AtomicU32,
    rejects_guard: AtomicU32,
    pub active_conns: Arc<AtomicU32>,
    closed: AtomicBool,
    closed_notify: tokio::sync::Notify,
}

impl TunnelTrafficAccounting {
    pub fn new_shared() -> Arc<Self> {
        Arc::new(Self {
            meter: TrafficMeter::default(),
            rejects_quota: AtomicU32::new(0),
            rejects_guard: AtomicU32::new(0),
            active_conns: Arc::new(AtomicU32::new(0)),
            closed: AtomicBool::new(false),
            closed_notify: tokio::sync::Notify::new(),
        })
    }

    /// 控制面摘除隧道（暂停 / 配额耗尽 / 删除）时调用，通知该隧道所有存量连接断开。
    pub fn close_connections(&self) {
        if !self.closed.swap(true, Ordering::SeqCst) {
            self.closed_notify.notify_waiters();
        }
    }

    /// 踢掉存量连接（例如 L4 拦截刚开启），但不永久关闭隧道；新连接可继续建立。
    pub fn kick_connections(&self) {
        self.closed_notify.notify_waiters();
    }

    /// 在连接任务里与转发 `select!`，隧道被摘除或被踢时完成。
    pub async fn connections_closed(&self) {
        let notified = self.closed_notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        notified.await;
    }

    pub fn record_reject_quota(&self) {
        self.rejects_quota.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_reject_guard(&self) {
        self.rejects_guard.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_datagram_in(&self, bytes: usize) {
        self.meter.left_to_right.add(bytes);
    }

    pub fn record_datagram_out(&self, bytes: usize) {
        self.meter.right_to_left.add(bytes);
    }

    pub fn try_acquire_conn(&self, max_conns: i32) -> bool {
        if max_conns <= 0 {
            return true;
        }
        loop {
            let current = self.active_conns.load(Ordering::Relaxed);
            if current as i32 >= max_conns {
                return false;
            }
            if self
                .active_conns
                .compare_exchange_weak(current, current + 1, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                return true;
            }
        }
    }

    pub fn release_conn(&self) {
        self.active_conns.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn drain_interval_sample(&self, tunnel_id: Uuid) -> TunnelTrafficSample {
        let delta = self.meter.take_delta();
        TunnelTrafficSample {
            tunnel_id,
            bytes_in: delta.left_to_right,
            bytes_out: delta.right_to_left,
            active_conns: self.active_conns.load(Ordering::Relaxed),
            rejects_quota: self.rejects_quota.swap(0, Ordering::Relaxed),
            rejects_guard: self.rejects_guard.swap(0, Ordering::Relaxed),
        }
    }

    pub fn interval_sample_if_active(&self, tunnel_id: Uuid) -> Option<TunnelTrafficSample> {
        let sample = self.drain_interval_sample(tunnel_id);
        if sample.bytes_in == 0
            && sample.bytes_out == 0
            && sample.rejects_quota == 0
            && sample.rejects_guard == 0
            && sample.active_conns == 0
        {
            return None;
        }
        Some(sample)
    }

    /// 强制取本秒增量（含全零），用于已配置隧道的周期上报，避免有连接但无字节时 board 完全收不到样本。
    pub fn interval_sample(&self, tunnel_id: Uuid) -> TunnelTrafficSample {
        self.drain_interval_sample(tunnel_id)
    }
}

pub fn ensure_tunnel_traffic(
    map: &DashMap<Uuid, Arc<TunnelTrafficAccounting>>,
    tunnel_id: Uuid,
) -> Arc<TunnelTrafficAccounting> {
    map.entry(tunnel_id)
        .or_insert_with(TunnelTrafficAccounting::new_shared)
        .value()
        .clone()
}

pub fn collect_interval_samples(
    map: &DashMap<Uuid, Arc<TunnelTrafficAccounting>>,
    tunnel_ids: impl IntoIterator<Item = Uuid>,
) -> Vec<TunnelTrafficSample> {
    let mut ids: HashSet<Uuid> = tunnel_ids.into_iter().collect();
    for entry in map.iter() {
        ids.insert(*entry.key());
    }
    ids.into_iter()
        .map(|tunnel_id| {
            map.get(&tunnel_id).map_or(
                TunnelTrafficSample {
                    tunnel_id,
                    bytes_in: 0,
                    bytes_out: 0,
                    active_conns: 0,
                    rejects_quota: 0,
                    rejects_guard: 0,
                },
                |entry| entry.interval_sample(tunnel_id),
            )
        })
        .filter(|sample| {
            sample.bytes_in > 0
                || sample.bytes_out > 0
                || sample.active_conns > 0
                || sample.rejects_quota > 0
                || sample.rejects_guard > 0
        })
        .collect()
}

pub fn drain_traffic_accounting(
    entry: &Arc<TunnelTrafficAccounting>,
    tunnel_id: Uuid,
) -> TunnelTrafficSample {
    entry.interval_sample(tunnel_id)
}

pub fn sample_has_bytes(sample: &TunnelTrafficSample) -> bool {
    sample.bytes_in > 0
        || sample.bytes_out > 0
        || sample.rejects_quota > 0
        || sample.rejects_guard > 0
        || sample.active_conns > 0
}
