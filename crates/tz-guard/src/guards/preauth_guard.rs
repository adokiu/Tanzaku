use crate::{
    pipeline::{GuardFactory, GuardModule},
    types::{AuthFailCtx, ConnCtx, HttpCtx, PktCtx, Reason, Verdict},
};
use serde_json::Value;
use std::sync::Arc;

inventory::submit! {
    GuardFactory {
        name: "preauth_guard",
        build: build,
    }
}

fn build(config: &Value) -> Option<Arc<dyn GuardModule>> {
    Some(Arc::new(PreauthGuard {
        max_unauthenticated: config
            .get("max_unauthenticated")
            .and_then(Value::as_u64)
            .unwrap_or(128) as u32,
    }))
}

struct PreauthGuard {
    max_unauthenticated: u32,
}

impl GuardModule for PreauthGuard {
    fn name(&self) -> &'static str {
        "preauth_guard"
    }

    fn on_conn(&self, _: &ConnCtx) -> Verdict {
        Verdict::Continue
    }

    fn on_udp_packet(&self, _: &PktCtx) -> Verdict {
        Verdict::Continue
    }

    fn on_http_request(&self, _: &HttpCtx) -> Verdict {
        Verdict::Continue
    }

    fn on_auth_failure(&self, _: &AuthFailCtx) -> Verdict {
        Verdict::Deny(Reason::Preauth)
    }
}
