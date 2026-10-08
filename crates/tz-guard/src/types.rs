use std::{net::IpAddr, time::Duration};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceTrust {
    Tcp,
    Udp,
}

#[derive(Debug, Clone)]
pub struct ConnCtx {
    pub peer: IpAddr,
    pub trust: SourceTrust,
    pub trusted_proxy: bool,
    pub tunnel_id: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub struct PktCtx {
    pub peer: IpAddr,
    pub trust: SourceTrust,
    pub bytes: usize,
    pub new_flow: bool,
    pub tunnel_id: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub struct HttpCtx {
    pub peer: IpAddr,
    pub trust: SourceTrust,
    pub header_bytes: usize,
    pub header_count: usize,
    pub request_rate_slot: u64,
}

#[derive(Debug, Clone)]
pub struct AuthFailCtx {
    pub peer: IpAddr,
    pub trust: SourceTrust,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    IpAcl,
    RateLimit,
    Ban,
    UdpAmplify,
    HttpPolicy,
    TlsPolicy,
    Preauth,
    /// TCP/UDP 隧道上检测到 HTTP，应改用 HTTP 隧道。
    BlockHttpOnL4,
    /// 单隧道不同来源 IP 数超限。
    PerTunnelIpLimit,
}

impl Reason {
    pub fn rule_name(&self) -> &'static str {
        match self {
            Self::IpAcl => "ip_acl",
            Self::RateLimit => "per_ip_limit",
            Self::Ban => "auto_ban",
            Self::UdpAmplify => "udp_amplify",
            Self::HttpPolicy => "http_guard",
            Self::TlsPolicy => "tls_guard",
            Self::Preauth => "preauth_guard",
            Self::BlockHttpOnL4 => "block_http_on_l4",
            Self::PerTunnelIpLimit => "per_tunnel_ip_limit",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Continue,
    Trust,
    Deny(Reason),
    Ban(Reason, Duration),
}
