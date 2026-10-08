use crate::{
    pipeline::{GuardFactory, GuardModule},
    types::{AuthFailCtx, ConnCtx, HttpCtx, PktCtx, Reason, Verdict},
};
use serde_json::Value;
use std::sync::Arc;

inventory::submit! {
    GuardFactory {
        name: "http_guard",
        build: build,
    }
}

fn build(config: &Value) -> Option<Arc<dyn GuardModule>> {
    Some(Arc::new(HttpGuard {
        max_header_bytes: config
            .get("max_header_bytes")
            .and_then(Value::as_u64)
            .unwrap_or(16_384) as usize,
        max_headers: config
            .get("max_headers")
            .and_then(Value::as_u64)
            .unwrap_or(100) as usize,
    }))
}

struct HttpGuard {
    max_header_bytes: usize,
    max_headers: usize,
}

impl GuardModule for HttpGuard {
    fn name(&self) -> &'static str {
        "http_guard"
    }

    fn on_conn(&self, _: &ConnCtx) -> Verdict {
        Verdict::Continue
    }

    fn on_udp_packet(&self, _: &PktCtx) -> Verdict {
        Verdict::Continue
    }

    fn on_http_request(&self, ctx: &HttpCtx) -> Verdict {
        if ctx.header_bytes > self.max_header_bytes || ctx.header_count > self.max_headers {
            return Verdict::Deny(Reason::HttpPolicy);
        }
        Verdict::Continue
    }

    fn on_auth_failure(&self, ctx: &AuthFailCtx) -> Verdict {
        if ctx.reason.contains("http") {
            return Verdict::Ban(Reason::HttpPolicy, std::time::Duration::from_secs(600));
        }
        Verdict::Continue
    }
}
