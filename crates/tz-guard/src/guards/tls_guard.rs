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
        max_handshakes: AtomicU32::new(handshake_limit(config)),
        active: AtomicU32::new(0),
    }))
}

fn handshake_limit(config: &Value) -> u32 {
    config.get("max_handshakes").and_then(Value::as_u64)
        .unwrap_or(256).clamp(1, u64::from(u32::MAX)) as u32
}

struct TlsGuard {
    max_handshakes: AtomicU32,
    active: AtomicU32,
}

impl GuardModule for TlsGuard {
    fn name(&self) -> &'static str {
        "tls_guard"
    }

    fn on_conn(&self, _: &ConnCtx) -> Verdict {
        Verdict::Continue
    }

    fn on_tls_handshake(&self) -> Verdict {
        let limit = self.max_handshakes.load(Ordering::Relaxed);
        match self.active.fetch_update(Ordering::AcqRel, Ordering::Relaxed, |active| {
            (active < limit).then(|| active + 1)
        }) {
            Ok(_) => Verdict::Continue,
            Err(_) => Verdict::Deny(Reason::TlsPolicy),
        }
    }

    fn release_tls_handshake(&self) {
        let _ = self.active.fetch_update(Ordering::AcqRel, Ordering::Relaxed, |active| {
            (active > 0).then_some(active - 1)
        });
    }

    fn reconfigure(&self, config: &Value) -> bool {
        self.max_handshakes.store(handshake_limit(config), Ordering::Relaxed);
        true
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn handshake_slots_are_released_and_not_cumulative() {
        let guard = build(&json!({"max_handshakes": 2})).expect("tls_guard");
        assert!(matches!(guard.on_tls_handshake(), Verdict::Continue));
        assert!(matches!(guard.on_tls_handshake(), Verdict::Continue));
        assert!(matches!(
            guard.on_tls_handshake(),
            Verdict::Deny(Reason::TlsPolicy)
        ));
        guard.release_tls_handshake();
        assert!(matches!(guard.on_tls_handshake(), Verdict::Continue));
        for _ in 0..8 {
            guard.release_tls_handshake();
        }
        assert!(matches!(guard.on_tls_handshake(), Verdict::Continue));
        guard.release_tls_handshake();
    }

    #[test]
    fn plain_http_hooks_do_not_consume_handshake_slots() {
        let guard = build(&json!({"max_handshakes": 1})).expect("tls_guard");
        for _ in 0..8 {
            assert!(matches!(
                guard.on_conn(&ConnCtx {
                    peer: "127.0.0.1".parse().unwrap(),
                    trust: crate::types::SourceTrust::Tcp,
                    trusted_proxy: false,
                    tunnel_id: None,
                }),
                Verdict::Continue
            ));
            assert!(matches!(
                guard.on_http_request(&HttpCtx {
                    peer: "127.0.0.1".parse().unwrap(),
                    trust: crate::types::SourceTrust::Tcp,
                    header_bytes: 64,
                    header_count: 2,
                    request_rate_slot: 0,
                }),
                Verdict::Continue
            ));
        }
        assert!(matches!(guard.on_tls_handshake(), Verdict::Continue));
    }
}
