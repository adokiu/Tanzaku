use crate::{
    pipeline::{GuardFactory, GuardModule},
    types::{AuthFailCtx, ConnCtx, HttpCtx, PktCtx, Reason, Verdict},
};
use serde_json::Value;
use std::{
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
};

inventory::submit! {
    GuardFactory {
        name: "tls_guard",
        build: build,
    }
}

fn build(config: &Value) -> Option<Arc<dyn GuardModule>> {
    Some(Arc::new(TlsGuard {
        max_handshakes: config
            .get("max_handshakes")
            .and_then(Value::as_u64)
            .unwrap_or(256) as u32,
        active: AtomicU32::new(0),
    }))
}

struct TlsGuard {
    max_handshakes: u32,
    active: AtomicU32,
}

impl GuardModule for TlsGuard {
    fn name(&self) -> &'static str {
        "tls_guard"
    }

    fn on_conn(&self, _: &ConnCtx) -> Verdict {
        let active = self.active.fetch_add(1, Ordering::Relaxed);
        if active >= self.max_handshakes {
            return Verdict::Deny(Reason::TlsPolicy);
        }
        Verdict::Continue
    }

    fn on_udp_packet(&self, _: &PktCtx) -> Verdict {
        Verdict::Continue
    }

    fn on_http_request(&self, _: &HttpCtx) -> Verdict {
        Verdict::Continue
    }

    fn on_auth_failure(&self, ctx: &AuthFailCtx) -> Verdict {
        if ctx.reason.contains("tls") {
            return Verdict::Ban(Reason::TlsPolicy, std::time::Duration::from_secs(600));
        }
        Verdict::Continue
    }
}
