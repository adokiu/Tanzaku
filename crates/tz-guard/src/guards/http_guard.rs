use crate::{
    pipeline::{GuardFactory, GuardModule},
    types::{AuthFailCtx, ConnCtx, HttpCtx, PktCtx, Reason, Verdict},
};
use serde_json::Value;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

inventory::submit! {
    GuardFactory {
        name: "http_guard",
        build: build,
    }
}

fn build(config: &Value) -> Option<Arc<dyn GuardModule>> {
    Some(Arc::new(HttpGuard {
        max_header_bytes: AtomicUsize::new(header_bytes_limit(config)),
        max_headers: AtomicUsize::new(header_count_limit(config)),
    }))
}

fn header_bytes_limit(config: &Value) -> usize {
    config
        .get("max_header_bytes")
        .and_then(Value::as_u64)
        .unwrap_or(16_384)
        .clamp(1, u64::from(u32::MAX)) as usize
}

fn header_count_limit(config: &Value) -> usize {
    config
        .get("max_headers")
        .and_then(Value::as_u64)
        .unwrap_or(100)
        .clamp(1, u64::from(u32::MAX)) as usize
}

struct HttpGuard {
    max_header_bytes: AtomicUsize,
    max_headers: AtomicUsize,
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
        if ctx.header_bytes > self.max_header_bytes.load(Ordering::Relaxed)
            || ctx.header_count > self.max_headers.load(Ordering::Relaxed)
        {
            return Verdict::Deny(Reason::HttpPolicy);
        }
        Verdict::Continue
    }

    fn reconfigure(&self, config: &Value) -> bool {
        self.max_header_bytes
            .store(header_bytes_limit(config), Ordering::Relaxed);
        self.max_headers
            .store(header_count_limit(config), Ordering::Relaxed);
        true
    }

    fn on_auth_failure(&self, ctx: &AuthFailCtx) -> Verdict {
        if ctx.reason.contains("http") {
            return Verdict::Ban(Reason::HttpPolicy, std::time::Duration::from_secs(600));
        }
        Verdict::Continue
    }
}
