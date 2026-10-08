use crate::{
    pipeline::{GuardFactory, GuardModule},
    types::{AuthFailCtx, ConnCtx, HttpCtx, PktCtx, Reason, Verdict},
};
use dashmap::DashMap;
use serde_json::Value;
use std::{net::IpAddr, sync::Arc, time::Instant};

inventory::submit! {
    GuardFactory {
        name: "udp_amplify",
        build: build,
    }
}

/// 计数器条目超过该数量时清理已过窗口的 IP。
const PRUNE_THRESHOLD: usize = 65_536;

/// UDP 防护：按来源/访客 IP 限制每秒新 flow 与每秒包数（上/下行都计），0 表示不限。
fn build(config: &Value) -> Option<Arc<dyn GuardModule>> {
    let limit = |key: &str, default: u64| {
        config
            .get(key)
            .and_then(|value| value.as_u64().or_else(|| value.as_f64().map(|n| n as u64)))
            .unwrap_or(default)
            .min(u64::from(u32::MAX)) as u32
    };
    Some(Arc::new(UdpFlood {
        max_new_flows_per_sec: limit("max_new_flows_per_sec", 128),
        max_packets_per_sec: limit("max_packets_per_sec", 200_000),
        started: Instant::now(),
        sources: DashMap::new(),
    }))
}

struct SourceWindow {
    second: u64,
    new_flows: u32,
    packets: u32,
}

struct UdpFlood {
    max_new_flows_per_sec: u32,
    max_packets_per_sec: u32,
    started: Instant,
    sources: DashMap<IpAddr, SourceWindow>,
}

impl GuardModule for UdpFlood {
    fn name(&self) -> &'static str {
        "udp_amplify"
    }

    fn on_conn(&self, _: &ConnCtx) -> Verdict {
        Verdict::Continue
    }

    fn on_udp_packet(&self, ctx: &PktCtx) -> Verdict {
        if ctx.new_flow && ctx.bytes == 0 {
            return Verdict::Deny(Reason::UdpAmplify);
        }
        let now = self.started.elapsed().as_secs();
        if self.sources.len() > PRUNE_THRESHOLD {
            self.sources.retain(|_, window| window.second >= now);
        }

        // DashMap entry 持有分片锁，避免上/下行并发下计数被打飞。
        let mut window = self.sources.entry(ctx.peer).or_insert_with(|| SourceWindow {
            second: now,
            new_flows: 0,
            packets: 0,
        });
        if window.second != now {
            window.second = now;
            window.new_flows = 0;
            window.packets = 0;
        }

        if self.max_packets_per_sec > 0 {
            if window.packets >= self.max_packets_per_sec {
                return Verdict::Deny(Reason::UdpAmplify);
            }
            window.packets += 1;
        }

        if ctx.new_flow && self.max_new_flows_per_sec > 0 {
            if window.new_flows >= self.max_new_flows_per_sec {
                return Verdict::Deny(Reason::UdpAmplify);
            }
            window.new_flows += 1;
        }

        Verdict::Continue
    }

    fn on_http_request(&self, _: &HttpCtx) -> Verdict {
        Verdict::Continue
    }

    fn on_auth_failure(&self, _: &AuthFailCtx) -> Verdict {
        Verdict::Continue
    }
}
