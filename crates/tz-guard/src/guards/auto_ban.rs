use crate::{
    pipeline::{GuardFactory, GuardModule},
    types::{AuthFailCtx, ConnCtx, HttpCtx, PktCtx, Reason, SourceTrust, Verdict},
};
use dashmap::DashMap;
use serde_json::Value;
use std::{
    net::IpAddr,
    sync::Arc,
    time::{Duration, Instant},
};

inventory::submit! {
    GuardFactory {
        name: "auto_ban",
        build: build,
    }
}

fn build(_config: &Value) -> Option<Arc<dyn GuardModule>> {
    Some(Arc::new(AutoBan {
        bans: DashMap::new(),
        base_duration: Duration::from_secs(600),
    }))
}

struct AutoBan {
    bans: DashMap<IpAddr, Instant>,
    base_duration: Duration,
}

impl GuardModule for AutoBan {
    fn name(&self) -> &'static str {
        "auto_ban"
    }

    fn on_conn(&self, ctx: &ConnCtx) -> Verdict {
        if let Some(until) = self.bans.get(&ctx.peer) {
            if until.value() > &Instant::now() {
                return Verdict::Deny(Reason::Ban);
            }
            self.bans.remove(&ctx.peer);
        }
        Verdict::Continue
    }

    fn on_udp_packet(&self, _: &PktCtx) -> Verdict {
        Verdict::Continue
    }

    fn on_http_request(&self, ctx: &HttpCtx) -> Verdict {
        self.on_conn(&ConnCtx {
            peer: ctx.peer,
            trust: ctx.trust,
            trusted_proxy: false,
            tunnel_id: None,
        })
    }

    fn on_auth_failure(&self, ctx: &AuthFailCtx) -> Verdict {
        if ctx.trust != SourceTrust::Tcp {
            return Verdict::Continue;
        }
        self.bans
            .insert(ctx.peer, Instant::now() + self.base_duration);
        Verdict::Ban(Reason::Ban, self.base_duration)
    }
}
