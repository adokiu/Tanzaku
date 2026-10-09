use crate::{
    pipeline::{GuardFactory, GuardModule},
    token_bucket::TokenBucket,
    types::{AuthFailCtx, ConnCtx, HttpCtx, PktCtx, Reason, Verdict},
};
use dashmap::DashMap;
use serde_json::Value;
use std::{
    net::IpAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

inventory::submit! {
    GuardFactory {
        name: "per_ip_limit",
        build: build,
    }
}

/// 计数器条目超过该数量时清理已过窗口的 IP。
const PRUNE_THRESHOLD: usize = 65_536;
/// 窗口上限，防止误配置为超长窗口导致记忆耗尽。
const MAX_WINDOW_SECS: u32 = 86_400;

/// 兼容旧键 `max_new_per_sec`（隐含 1 秒窗口）。
fn parse_limit(config: &Value) -> (u32, u32) {
    let max_new = config
        .get("max_new")
        .and_then(Value::as_u64)
        .or_else(|| config.get("max_new_per_sec").and_then(Value::as_u64))
        .unwrap_or(64)
        .clamp(0, u64::from(u32::MAX)) as u32;
    let window_secs = config
        .get("window_secs")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .clamp(1, u64::from(MAX_WINDOW_SECS)) as u32;
    (max_new, window_secs)
}

fn build(config: &Value) -> Option<Arc<dyn GuardModule>> {
    let (max_new, window_secs) = parse_limit(config);
    Some(Arc::new(PerIpLimit {
        buckets: DashMap::new(),
        started: Mutex::new((max_new, window_secs)),
    }))
}

/// 按来源 IP 限制滑动窗口内的新建 TCP 连接 / HTTP 请求数（UDP 只由 `udp_amplify` 防泛洪）。
/// 默认 `window_secs = 1` 与旧版“每秒”语义一致；可配置更长窗口实现
/// “x 秒内最多 N 个事件”，避免低速率持续连接被逐秒放行后仍触发封禁。
/// 并发连接数由隧道级 `max_conns` 控制；这里没有连接关闭回调，无法按 IP 统计并发。
struct PerIpLimit {
    buckets: DashMap<IpAddr, TokenBucket>,
    /// (burst, window_secs)，新条目按当前值建桶。
    started: Mutex<(u32, u32)>,
}

impl PerIpLimit {
    fn admit_new(&self, peer: IpAddr) -> Verdict {
        let (burst, window_secs) = *self.started.lock().expect("per_ip_limit config");
        if self.buckets.len() > PRUNE_THRESHOLD {
            let cutoff = Duration::from_secs(u64::from(window_secs) * 4);
            self.buckets.retain(|_, bucket| bucket.idle() < cutoff);
        }
        let bucket = self.buckets.entry(peer).or_insert_with(|| TokenBucket::new(burst, window_secs));
        if bucket.allow() {
            Verdict::Continue
        } else {
            Verdict::Deny(Reason::RateLimit)
        }
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

    /// HTTP 入口已在连接/请求入口调用 `check_conn`，此处不再重复计数。
    fn on_http_request(&self, _: &HttpCtx) -> Verdict {
        Verdict::Continue
    }

    fn reconfigure(&self, config: &Value) -> bool {
        let (max_new, window_secs) = parse_limit(config);
        {
            let mut started = self.started.lock().expect("per_ip_limit config");
            *started = (max_new, window_secs);
        }
        for entry in self.buckets.iter() {
            entry.value().reconfigure(max_new, window_secs);
        }
        true
    }

    fn on_auth_failure(&self, _: &AuthFailCtx) -> Verdict {
        Verdict::Continue
    }
}