use crate::{
    pipeline::{GuardFactory, GuardModule},
    types::{AuthFailCtx, ConnCtx, HttpCtx, PktCtx, Reason, Verdict},
};
use dashmap::DashMap;
use serde_json::Value;
use std::{
    net::IpAddr,
    sync::{
        atomic::{AtomicU32, AtomicU64, Ordering},
        Arc,
    },
    time::Instant,
};
inventory::submit! {
    GuardFactory {
        name: "per_ip_limit",
        build: build,
    }
}

/// 计数器条目超过该数量时清理已过窗口的 IP。
const PRUNE_THRESHOLD: usize = 65_536;

fn build(config: &Value) -> Option<Arc<dyn GuardModule>> {
    Some(Arc::new(PerIpLimit {
        max_new_per_sec: config
            .get("max_new_per_sec")
            .and_then(Value::as_u64)
            .unwrap_or(64) as u32,
        started: Instant::now(),
        counters: DashMap::new(),
    }))
}

struct Counter {
    window_sec: AtomicU64,
    new_in_window: AtomicU32,
}

/// 按来源 IP 限制每秒新建 TCP 连接 / HTTP 请求数（UDP 只由 `udp_amplify` 防泛洪）。
/// 并发连接数由隧道级 `max_conns` 控制；这里没有连接关闭回调，无法按 IP 统计并发。
struct PerIpLimit {
    max_new_per_sec: u32,
    started: Instant,
    counters: DashMap<IpAddr, Counter>,
}

impl PerIpLimit {
    fn now_sec(&self) -> u64 {
        self.started.elapsed().as_secs()
    }

    fn admit_new(&self, peer: IpAddr) -> Verdict {
        let now = self.now_sec();
        if self.counters.len() > PRUNE_THRESHOLD {
            self.counters
                .retain(|_, counter| counter.window_sec.load(Ordering::Relaxed) >= now);
        }
        let counter = self.counters.entry(peer).or_insert_with(|| Counter {
            window_sec: AtomicU64::new(now),
            new_in_window: AtomicU32::new(0),
        });
        if counter.window_sec.swap(now, Ordering::Relaxed) != now {
            counter.new_in_window.store(0, Ordering::Relaxed);
        }
        if counter.new_in_window.fetch_add(1, Ordering::Relaxed) >= self.max_new_per_sec {
            return Verdict::Deny(Reason::RateLimit);
        }
        Verdict::Continue
    }
}

impl GuardModule for PerIpLimit {
    fn name(&self) -> &'static str {
        "per_ip_limit"
    }

    fn on_conn(&self, ctx: &ConnCtx) -> Verdict {
        self.admit_new(ctx.peer)
    }

    fn on_udp_packet(&self, _: &PktCtx) -> Verdict {
        Verdict::Continue
    }

    fn on_http_request(&self, ctx: &HttpCtx) -> Verdict {
        self.admit_new(ctx.peer)
    }

    fn on_auth_failure(&self, _: &AuthFailCtx) -> Verdict {
        Verdict::Continue
    }
}
