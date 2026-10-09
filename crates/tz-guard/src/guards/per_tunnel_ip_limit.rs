use crate::{
    pipeline::{GuardFactory, GuardModule},
    types::{AuthFailCtx, ConnCtx, HttpCtx, PktCtx, Reason, Verdict},
};
use dashmap::DashMap;
use serde_json::Value;
use std::{
    net::IpAddr,
    sync::{
        atomic::{AtomicU32, AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use uuid::Uuid;

inventory::submit! {
    GuardFactory {
        name: "per_tunnel_ip_limit",
        build: build,
    }
}

const PRUNE_THRESHOLD: usize = 65_536;
const MAX_WINDOW_SECS: u32 = 86_400;
const DEFAULT_WINDOW_SECS: u32 = 300;

fn parse_config(config: &Value) -> (usize, u32) {
    let max_distinct_ips = config
        .get("max_distinct_ips")
        .and_then(Value::as_u64)
        .unwrap_or(64)
        .min(u64::from(u32::MAX)) as usize;
    let window_secs = config
        .get("window_secs")
        .and_then(Value::as_u64)
        .unwrap_or(u64::from(DEFAULT_WINDOW_SECS))
        .clamp(1, u64::from(MAX_WINDOW_SECS)) as u32;
    (max_distinct_ips, window_secs)
}

fn build(config: &Value) -> Option<Arc<dyn GuardModule>> {
    let (max_distinct_ips, window_secs) = parse_config(config);
    if max_distinct_ips == 0 {
        return None;
    }
    Some(Arc::new(PerTunnelIpLimit {
        max_distinct_ips: AtomicUsize::new(max_distinct_ips),
        window_secs: AtomicU32::new(window_secs),
        tunnels: DashMap::new(),
    }))
}

struct TunnelIps {
    ips: DashMap<IpAddr, Instant>,
    last_active: Instant,
}

/// 单隧道滑动窗口来源 IP 数限制：`window_secs` 秒内允许的不同来源 IP 最多 `max_distinct_ips` 个。
/// IP 的最近活跃时间会随连接刷新，窗口内不重复计数；窗口外自动过期，
/// 避免旧实现“每秒 1 个 IP、累积 64 个后永久封禁”的误杀。
struct PerTunnelIpLimit {
    max_distinct_ips: AtomicUsize,
    window_secs: AtomicU32,
    tunnels: DashMap<Uuid, TunnelIps>,
}

impl PerTunnelIpLimit {
    fn admit(&self, tunnel_id: Uuid, peer: IpAddr) -> Verdict {
        let window_secs = self.window_secs.load(Ordering::Relaxed);
        let window = Duration::from_secs(u64::from(window_secs));
        if self.tunnels.len() > PRUNE_THRESHOLD {
            let cutoff = Instant::now() - window;
            self.tunnels.retain(|_, entry| entry.last_active >= cutoff);
        }
        let mut entry = self.tunnels.entry(tunnel_id).or_insert_with(|| TunnelIps {
            ips: DashMap::new(),
            last_active: Instant::now(),
        });
        entry.last_active = Instant::now();
        let now = Instant::now();
        if !entry.ips.contains_key(&peer) {
            // 先剔除窗口外 IP，再判断是否超限。
            entry.ips.retain(|_, seen| now.duration_since(*seen) < window);
            if entry.ips.len() >= self.max_distinct_ips.load(Ordering::Relaxed) {
                return Verdict::Deny(Reason::PerTunnelIpLimit);
            }
        }
        entry.ips.insert(peer, now);
        Verdict::Continue
    }
}

impl GuardModule for PerTunnelIpLimit {
    fn name(&self) -> &'static str {
        "per_tunnel_ip_limit"
    }

    fn on_conn(&self, ctx: &ConnCtx) -> Verdict {
        let Some(tunnel_id) = ctx.tunnel_id else {
            return Verdict::Continue;
        };
        self.admit(tunnel_id, ctx.peer)
    }

    fn on_udp_packet(&self, ctx: &PktCtx) -> Verdict {
        if !ctx.new_flow {
            return Verdict::Continue;
        }
        let Some(tunnel_id) = ctx.tunnel_id else {
            return Verdict::Continue;
        };
        self.admit(tunnel_id, ctx.peer)
    }

    fn on_http_request(&self, _: &HttpCtx) -> Verdict {
        Verdict::Continue
    }

    fn reconfigure(&self, config: &Value) -> bool {
        let (max_distinct_ips, window_secs) = parse_config(config);
        self.max_distinct_ips.store(max_distinct_ips, Ordering::Relaxed);
        self.window_secs.store(window_secs, Ordering::Relaxed);
        true
    }

    fn on_auth_failure(&self, _: &AuthFailCtx) -> Verdict {
        Verdict::Continue
    }
}