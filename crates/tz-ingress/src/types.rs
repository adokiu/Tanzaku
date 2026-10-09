use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use arc_swap::ArcSwap;
use dashmap::DashMap;
use tz_carrier::registry::CarrierSession;
use tz_guard::pipeline::GuardPipeline;
use tz_net::rate::{RateClock, TokenBucket};
use uuid::Uuid;

use crate::{shutdown::IngressShutdown, traffic::TunnelTrafficAccounting};

#[derive(Clone)]
pub struct TunnelEndpoint {
    pub tunnel_id: Uuid,
    pub bind_addr: String,
    pub port: u16,
    pub target_host: String,
    pub target_port: u16,
    /// L4 拦截页按 SNI 选证书用的解析器（节点共享入口的 SniCertResolver）；None 则全程自签。
    pub l4_block_cert_resolver: Option<Arc<crate::shared_https::SniCertResolver>>,
    /// 按 tunnel_id 索引；每次入站连接取最新 carrier（重连后 client_session 快照会过期）。
    pub live_sessions: Arc<DashMap<Uuid, Arc<dyn CarrierSession>>>,
    pub client_session: Arc<dyn CarrierSession>,
    pub guard: Arc<GuardPipeline>,
    pub speed_limit_mbps: i64,
    pub max_conns: i32,
    pub traffic: Arc<TunnelTrafficAccounting>,
    pub rate_clock: RateClock,
    pub new_conn_bucket: Option<TokenBucket>,
    pub shutdown: IngressShutdown,
    pub bind_failed: Arc<Mutex<Option<(u16, String)>>>,
    /// ingress 监听成功后再上报 TunnelReady（与 TCP/UDP 一致）。
    pub on_ingress_bound: Option<Arc<dyn Fn() + Send + Sync>>,
    /// 独立端口 HTTP 入口允许的 Host；配置变更时原地替换，不重启监听。
    pub http_hosts: Arc<ArcSwap<DedicatedHttpHosts>>,
    /// 中国大陆未备案域名拦截；热更新，不重启监听。
    pub filing: Arc<ArcSwap<crate::filing::FilingGate>>,
    /// 独立端口 HTTP 隧道的公网协议：true 仅 HTTPS（明文请求 308 跳转），false 仅 HTTP。
    pub https_enabled: bool,
}

/// 独立端口 HTTP 入口的 Host 白名单：`节点IP:端口` 与隧道已审核域名。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DedicatedHttpHosts {
    pub ip_host: String,
    pub domains: Vec<String>,
}

impl DedicatedHttpHosts {
    pub fn new(public_host: &str, port: u16, domains: &[String]) -> Self {
        let public_host = public_host.trim().to_ascii_lowercase();
        let ip_host = if public_host.contains(':') && !public_host.starts_with('[') {
            format!("[{public_host}]:{port}")
        } else {
            format!("{public_host}:{port}")
        };
        Self {
            ip_host,
            domains: domains.iter().map(|domain| domain.to_ascii_lowercase()).collect(),
        }
    }

    pub fn allows(&self, host: &str) -> bool {
        let host = host.trim().to_ascii_lowercase();
        if host == self.ip_host {
            return true;
        }
        let name = host_without_port(&host);
        // 部分客户端 Host 不带端口，仍允许匹配节点公网 IP。
        if name == host_without_port(&self.ip_host) {
            return true;
        }
        self.domains.iter().any(|domain| domain == name)
    }
}

fn host_without_port(host: &str) -> &str {
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(host);
    }
    host.split(':').next().unwrap_or(host)
}

impl TunnelEndpoint {
    pub fn carrier_session(&self) -> Arc<dyn CarrierSession> {
        live_carrier_session(&self.live_sessions, self.tunnel_id, &self.client_session)
    }

    pub async fn allow_new_conn(&self) -> bool {
        if let Some(bucket) = &self.new_conn_bucket {
            if bucket.acquire(1).await.is_err() {
                self.traffic.record_reject_quota();
                return false;
            }
        }
        true
    }

    pub fn try_acquire_conn(&self) -> bool {
        if self.traffic.try_acquire_conn(self.max_conns) {
            return true;
        }
        self.traffic.record_reject_quota();
        false
    }

    pub fn release_conn(&self) {
        self.traffic.release_conn();
    }
}

pub fn live_carrier_session(
    live_sessions: &DashMap<Uuid, Arc<dyn CarrierSession>>,
    tunnel_id: Uuid,
    fallback: &Arc<dyn CarrierSession>,
) -> Arc<dyn CarrierSession> {
    live_sessions
        .get(&tunnel_id)
        .map(|session| session.clone())
        .unwrap_or_else(|| fallback.clone())
}

#[derive(Debug, Clone)]
pub struct AcceptContext {
    pub peer: SocketAddr,
    pub tunnel_id: Uuid,
}
