use crate::{
    pipeline::{GuardFactory, GuardModule},
    types::{AuthFailCtx, ConnCtx, HttpCtx, PktCtx, Reason, Verdict},
};
use ipnet::IpNet;
use serde_json::Value;
use std::{net::IpAddr, sync::Arc};

inventory::submit! {
    GuardFactory {
        name: "ip_acl",
        build: build,
    }
}

fn build(config: &Value) -> Option<Arc<dyn GuardModule>> {
    let deny = config.get("deny").map(parse_list).unwrap_or_default();
    let allow = config.get("allow").map(parse_list).unwrap_or_default();
    Some(Arc::new(IpAcl { allow, deny }))
}

fn parse_list(value: &Value) -> Vec<IpNet> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.as_str())
        .filter_map(|cidr| cidr.parse().ok())
        .collect()
}

struct IpAcl {
    allow: Vec<IpNet>,
    deny: Vec<IpNet>,
}

impl GuardModule for IpAcl {
    fn name(&self) -> &'static str {
        "ip_acl"
    }

    fn on_conn(&self, ctx: &ConnCtx) -> Verdict {
        evaluate(self, ctx.peer)
    }

    fn on_udp_packet(&self, ctx: &PktCtx) -> Verdict {
        // 与防火墙一致：UDP 无握手，违规包直接丢弃。
        evaluate(self, ctx.peer)
    }

    fn on_http_request(&self, ctx: &HttpCtx) -> Verdict {
        evaluate(self, ctx.peer)
    }

    fn on_auth_failure(&self, _: &AuthFailCtx) -> Verdict {
        Verdict::Continue
    }
}

/// deny 优先；若配置了 allow 白名单，则仅白名单放行，其余拒绝（同 iptables 默认 DROP）。
fn evaluate(acl: &IpAcl, peer: IpAddr) -> Verdict {
    if acl.deny.iter().any(|net| net.contains(&peer)) {
        return Verdict::Deny(Reason::IpAcl);
    }
    if acl.allow.is_empty() {
        return Verdict::Continue;
    }
    if acl.allow.iter().any(|net| net.contains(&peer)) {
        return Verdict::Trust;
    }
    Verdict::Deny(Reason::IpAcl)
}
