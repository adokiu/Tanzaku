use crate::types::{AuthFailCtx, ConnCtx, HttpCtx, PktCtx, Reason, Verdict};
use arc_swap::ArcSwap;
use serde_json::Value;
use std::{net::IpAddr, sync::Arc};
use uuid::Uuid;

pub struct GuardFactory {
    pub name: &'static str,
    pub build: fn(&Value) -> Option<Arc<dyn GuardModule>>,
}

inventory::collect!(GuardFactory);

pub trait GuardModule: Send + Sync {
    fn name(&self) -> &'static str;
    fn on_conn(&self, ctx: &ConnCtx) -> Verdict;
    fn on_udp_packet(&self, ctx: &PktCtx) -> Verdict;
    fn on_http_request(&self, ctx: &HttpCtx) -> Verdict;
    fn on_auth_failure(&self, ctx: &AuthFailCtx) -> Verdict;
    fn on_tls_handshake(&self) -> Verdict {
        Verdict::Continue
    }
    fn release_tls_handshake(&self) {}
    fn reconfigure(&self, _config: &Value) -> bool {
        false
    }
}

pub struct TlsHandshakeGuard {
    modules: Vec<Arc<dyn GuardModule>>,
}

impl Drop for TlsHandshakeGuard {
    fn drop(&mut self) {
        for module in &self.modules {
            module.release_tls_handshake();
        }
    }
}

/// 防护触发回调（节点侧聚合后上报 Board）。
pub type GuardEventHook = Arc<dyn Fn(GuardEvent) + Send + Sync>;

#[derive(Debug, Clone)]
pub struct GuardEvent {
    pub rule: String,
    pub peer: Option<String>,
    pub tunnel_id: Option<Uuid>,
    pub detail: String,
}

pub struct GuardPipeline {
    inner: ArcSwap<Vec<Arc<dyn GuardModule>>>,
    policy: ArcSwap<Value>,
    event_hook: ArcSwap<Option<GuardEventHook>>,
}

impl GuardPipeline {
    pub fn from_policy(policy: &Value) -> Self {
        let mut modules = Vec::new();
        if let Some(object) = policy.as_object() {
            for (name, config) in object {
                if config.get("enabled").and_then(Value::as_bool) == Some(false) {
                    continue;
                }
                if let Some(factory) = inventory::iter::<GuardFactory>().find(|f| f.name == name.as_str())
                {
                    if let Some(module) = (factory.build)(config) {
                        modules.push(module);
                    }
                }
            }
        }
        // IP ACL 最先执行，行为接近防火墙前置过滤。
        modules.sort_by_key(|module| if module.name() == "ip_acl" { 0 } else { 1 });
        Self {
            inner: ArcSwap::from_pointee(modules),
            policy: ArcSwap::from_pointee(policy.clone()),
            event_hook: ArcSwap::from_pointee(None),
        }
    }

    pub fn policy_snapshot(&self) -> Arc<Value> {
        self.policy.load_full()
    }

    pub fn reload(&self, policy: &Value) {
        let previous_policy = self.policy.load_full();
        if previous_policy.as_ref() == policy {
            return;
        }
        let previous = self.inner.load_full();
        let rebuilt = Self::from_policy(policy);
        let modules = rebuilt.inner.load().iter().map(|module| {
            previous.iter().find(|old| old.name() == module.name())
                .filter(|old| {
                    previous_policy.get(old.name()) == policy.get(old.name())
                        || old.reconfigure(&policy[old.name()])
                })
                .cloned()
                .unwrap_or_else(|| module.clone())
        }).collect();
        self.inner.store(Arc::new(modules));
        self.policy.store(rebuilt.policy.load_full());
        // 保留已注册的事件钩子。
    }

    pub fn set_event_hook(&self, hook: Option<GuardEventHook>) {
        self.event_hook.store(Arc::new(hook));
    }

    /// TCP/UDP 隧道上探测到 HTTP 时拦截并返回提示页。
    pub fn block_http_on_l4(&self) -> bool {
        self.policy
            .load()
            .get("block_http_on_l4")
            .and_then(|cfg| cfg.get("enabled"))
            .and_then(Value::as_bool)
            == Some(true)
    }

    pub fn report_deny(
        &self,
        reason: &Reason,
        peer: IpAddr,
        tunnel_id: Option<Uuid>,
        detail: impl Into<String>,
    ) {
        let Some(hook) = self.event_hook.load().as_ref().clone() else {
            return;
        };
        hook(GuardEvent {
            rule: reason.rule_name().to_string(),
            peer: Some(peer.to_string()),
            tunnel_id,
            detail: detail.into(),
        });
    }

    /// 若为 Deny/Ban 则上报并返回 true。
    pub fn record_if_denied(
        &self,
        verdict: &Verdict,
        peer: IpAddr,
        tunnel_id: Option<Uuid>,
    ) -> bool {
        match verdict {
            Verdict::Deny(reason) | Verdict::Ban(reason, _) => {
                self.report_deny(reason, peer, tunnel_id, "");
                true
            }
            _ => false,
        }
    }

    pub fn check_conn(&self, ctx: &ConnCtx) -> Verdict {
        self.evaluate(ctx, GuardHook::Conn)
    }

    pub fn acquire_tls_handshake(&self) -> Result<TlsHandshakeGuard, Verdict> {
        let mut permit = TlsHandshakeGuard { modules: Vec::new() };
        for module in self.inner.load().iter() {
            match module.on_tls_handshake() {
                Verdict::Continue | Verdict::Trust => permit.modules.push(module.clone()),
                verdict => return Err(verdict),
            }
        }
        Ok(permit)
    }

    pub fn check_ip_acl(&self, ctx: &ConnCtx) -> Verdict {
        self.inner.load().iter()
            .find(|module| module.name() == "ip_acl")
            .map_or(Verdict::Continue, |module| module.on_conn(ctx))
    }

    pub fn check_udp(&self, ctx: &PktCtx) -> Verdict {
        for module in self.inner.load().iter() {
            match module.on_udp_packet(ctx) {
                Verdict::Continue => {}
                Verdict::Trust => return Verdict::Trust,
                verdict => return verdict,
            }
        }
        Verdict::Continue
    }

    pub fn check_http(&self, ctx: &HttpCtx) -> Verdict {
        for module in self.inner.load().iter() {
            match module.on_http_request(ctx) {
                Verdict::Continue => {}
                Verdict::Trust => return Verdict::Trust,
                verdict => return verdict,
            }
        }
        Verdict::Continue
    }

    fn evaluate(&self, ctx: &ConnCtx, hook: GuardHook) -> Verdict {
        for module in self.inner.load().iter() {
            let verdict = match hook {
                GuardHook::Conn => module.on_conn(ctx),
                GuardHook::Auth => Verdict::Continue,
            };
            match verdict {
                Verdict::Continue => {}
                Verdict::Trust => return Verdict::Trust,
                verdict => return verdict,
            }
        }
        Verdict::Continue
    }

    pub fn check_auth_failure(&self, ctx: &AuthFailCtx) -> Verdict {
        for module in self.inner.load().iter() {
            match module.on_auth_failure(ctx) {
                Verdict::Continue => {}
                Verdict::Trust => return Verdict::Trust,
                verdict => return verdict,
            }
        }
        Verdict::Continue
    }
}

enum GuardHook {
    Conn,
    Auth,
}

pub fn registered_modules() -> Vec<&'static str> {
    inventory::iter::<GuardFactory>()
        .map(|factory| factory.name)
        .collect()
}

struct NullGuard;

impl GuardModule for NullGuard {
    fn name(&self) -> &'static str {
        "null"
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
        Verdict::Continue
    }
}

pub fn null_module() -> Arc<dyn GuardModule> {
    Arc::new(NullGuard)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn soft_reload_keeps_tls_handshake_counters() {
        let pipeline = GuardPipeline::from_policy(&json!({
            "tls_guard": {"enabled": true, "max_handshakes": 2}
        }));
        let first = pipeline.acquire_tls_handshake().expect("slot");
        let second = pipeline.acquire_tls_handshake().expect("slot");
        assert!(pipeline.acquire_tls_handshake().is_err());
        pipeline.reload(&json!({
            "tls_guard": {"enabled": true, "max_handshakes": 2},
            "http_guard": {"enabled": true}
        }));
        assert!(pipeline.acquire_tls_handshake().is_err());
        drop(first);
        assert!(pipeline.acquire_tls_handshake().is_ok());
        drop(second);
    }

    #[test]
    fn unrelated_module_config_change_updates_limit_without_reset_when_reconfigurable() {
        let pipeline = GuardPipeline::from_policy(&json!({
            "tls_guard": {"enabled": true, "max_handshakes": 1}
        }));
        let _held = pipeline.acquire_tls_handshake().expect("slot");
        pipeline.reload(&json!({
            "tls_guard": {"enabled": true, "max_handshakes": 3}
        }));
        assert!(pipeline.acquire_tls_handshake().is_ok());
        assert!(pipeline.acquire_tls_handshake().is_ok());
        assert!(pipeline.acquire_tls_handshake().is_err());
    }
}
