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
        name: "udp_amplify",
        build: build,
    }
}

/// 计数器条目超过该数量时清理已过窗口的 IP。
const PRUNE_THRESHOLD: usize = 65_536;
const MAX_WINDOW_SECS: u32 = 86_400;

/// 兼容旧键 `max_*_per_sec`（隐含 1 秒窗口）。
fn parse_config(config: &Value) -> (u32, u32, u32) {
    let number = |key: &str| {
        config
            .get(key)
            .and_then(|value| value.as_u64().or_else(|| value.as_f64().map(|n| n as u64)))
    };
    let max_new_flows = number("max_new_flows")
        .or_else(|| number("max_new_flows_per_sec"))
        .unwrap_or(128)
        .min(u64::from(u32::MAX)) as u32;
    let max_packets = number("max_packets")
        .or_else(|| number("max_packets_per_sec"))
        .unwrap_or(200_000)
        .min(u64::from(u32::MAX)) as u32;
    let window_secs = number("window_secs")
        .unwrap_or(1)
        .clamp(1, u64::from(MAX_WINDOW_SECS)) as u32;
    (max_new_flows, max_packets, window_secs)
}

/// UDP 防护：按来源/访客 IP 限制滑动窗口内新 flow 数与包数（上/下行都计），0 表示不限。
/// 默认 `window_secs = 1` 与旧版“每秒”语义一致；可配置更长窗口实现
/// “x 秒内最多 N 个包 / N 条新流”。
fn build(config: &Value) -> Option<Arc<dyn GuardModule>> {
    let (max_new_flows, max_packets, window_secs) = parse_config(config);
    Some(Arc::new(UdpFlood {
        sources: DashMap::new(),
        limits: Mutex::new((max_new_flows, max_packets, window_secs)),
    }))
}

struct SourceWindow {
    packets: TokenBucket,
    new_flows: TokenBucket,
}

struct UdpFlood {
    sources: DashMap<IpAddr, SourceWindow>,
    /// (max_new_flows, max_packets, window_secs)，新条目按当前值建桶。
    limits: Mutex<(u32, u32, u32)>,
}

impl UdpFlood {
    fn admit(&self, ctx: &PktCtx) -> Verdict {
        let (max_new_flows, max_packets, window_secs) = *self.limits.lock().expect("udp_amplify config");
        if self.sources.len() > PRUNE_THRESHOLD {
            let cutoff = Duration::from_secs(u64::from(window_secs) * 4);
            self.sources
                .retain(|_, window| window.packets.idle() < cutoff && window.new_flows.idle() < cutoff);
        }
        let window = self.sources.entry(ctx.peer).or_insert_with(|| SourceWindow {
            packets: TokenBucket::new(max_packets, window_secs),
            new_flows: TokenBucket::new(max_new_flows, window_secs),
        });
        if !window.packets.allow() {
            return Verdict::Deny(Reason::UdpAmplify);
        }
        if ctx.new_flow && !window.new_flows.allow() {
            return Verdict::Deny(Reason::UdpAmplify);
        }
        Verdict::Continue
    }
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
        self.admit(ctx)
    }

    fn on_http_request(&self, _: &HttpCtx) -> Verdict {
        Verdict::Continue
    }

    fn reconfigure(&self, config: &Value) -> bool {
        let (max_new_flows, max_packets, window_secs) = parse_config(config);
        {
            let mut limits = self.limits.lock().expect("udp_amplify config");
            *limits = (max_new_flows, max_packets, window_secs);
        }
        for entry in self.sources.iter() {
            entry.value().packets.reconfigure(max_packets, window_secs);
            entry.value().new_flows.reconfigure(max_new_flows, window_secs);
        }
        true
    }

    fn on_auth_failure(&self, _: &AuthFailCtx) -> Verdict {
        Verdict::Continue
    }
}