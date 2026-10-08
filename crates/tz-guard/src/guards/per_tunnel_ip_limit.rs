use crate::{
    pipeline::{GuardFactory, GuardModule},
    types::{AuthFailCtx, ConnCtx, HttpCtx, PktCtx, Reason, Verdict},
};
use dashmap::DashMap;
use serde_json::Value;
use std::{
    net::IpAddr,
    sync::Arc,
    time::{Duration, Instant},
};
use uuid::Uuid;

inventory::submit! {
    GuardFactory {
        name: "per_tunnel_ip_limit",
        build: build,
    }
}

const IDLE_SECS: u64 = 300;
const PRUNE_THRESHOLD: usize = 65_536;

fn build(config: &Value) -> Option<Arc<dyn GuardModule>> {
    let max_distinct_ips = config
        .get("max_distinct_ips")
        .and_then(Value::as_u64)
        .unwrap_or(64)
        .min(u64::from(u32::MAX)) as usize;
    if max_distinct_ips == 0 {
        return None;
    }
    Some(Arc::new(PerTunnelIpLimit {
        max_distinct_ips,
        tunnels: DashMap::new(),
    }))
}

struct TunnelIps {
    ips: DashMap<IpAddr, Instant>,
    last_active: Instant,
}

struct PerTunnelIpLimit {
    max_distinct_ips: usize,
    tunnels: DashMap<Uuid, TunnelIps>,
}

impl PerTunnelIpLimit {
    fn admit(&self, tunnel_id: Uuid, peer: IpAddr) -> Verdict {
        if self.tunnels.len() > PRUNE_THRESHOLD {
            let cutoff = Instant::now() - Duration::from_secs(IDLE_SECS);
            self.tunnels
                .retain(|_, entry| entry.last_active >= cutoff);
        }
        let mut entry = self.tunnels.entry(tunnel_id).or_insert_with(|| TunnelIps {
            ips: DashMap::new(),
            last_active: Instant::now(),
        });
        entry.last_active = Instant::now();
        if entry.ips.contains_key(&peer) {
            entry.ips.insert(peer, Instant::now());
            return Verdict::Continue;
        }
        if entry.ips.len() >= self.max_distinct_ips {
            return Verdict::Deny(Reason::PerTunnelIpLimit);
        }
        entry.ips.insert(peer, Instant::now());
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

    fn on_auth_failure(&self, _: &AuthFailCtx) -> Verdict {
        Verdict::Continue
    }
}
