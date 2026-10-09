use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const PROTOCOL_MAJOR: u16 = 1;
pub const PROTOCOL_MINOR: u16 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolVersion {
    pub major: u16,
    pub minor: u16,
}

impl ProtocolVersion {
    pub const CURRENT: Self = Self {
        major: PROTOCOL_MAJOR,
        minor: PROTOCOL_MINOR,
    };

    pub const fn is_compatible_with(self, other: Self) -> bool {
        self.major == other.major
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    Request,
    Response,
    Event,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope<T> {
    pub version: ProtocolVersion,
    pub id: u64,
    pub kind: MessageKind,
    pub payload: T,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorPayload {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ControlOp {
    Auth(AuthMessage),
    Hello(HelloMessage),
    CsrRequest(CsrRequest),
    CertIssued(CertIssued),
    NodeConfig(NodeConfig),
    ClientConfig(ClientConfig),
    TunnelUpsert(TunnelSpec),
    TunnelRemove { tunnel_id: Uuid },
    TunnelSuspend { tunnel_id: Uuid },
    TunnelResume { tunnel_id: Uuid },
    StatsAck { seq: u64 },
    StatsReport(StatsReport),
    HostMetricsReport(HostMetricsReport),
    /// Client 进程内各隧道转发累计/区间流量（非整机网卡）。
    ClientTrafficReport(ClientTrafficReport),
    TunnelReady {
        tunnel_id: Uuid,
        revision: i64,
    },
    TunnelFailed {
        tunnel_id: Uuid,
        revision: i64,
        reason: String,
    },
    PortBindFailed {
        l4: String,
        port: u16,
        reason: String,
    },
    TunnelState(TunnelStateReport),
    /// 节点防护触发上报（限流 / ACL / 禁 HTTP over L4 等）。
    GuardEventReport(GuardEventReport),
    Error(ErrorPayload),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardEventReport {
    /// 攻击/防护类型，如 `per_ip_limit` / `udp_amplify`。
    pub rule: String,
    /// 关联隧道（控制面状态：用于自动暂停等处置）；无则仅记节点级事件。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tunnel_id: Option<Uuid>,
    /// 本窗口攻击强度（拒绝次数）。控制面不携带来源 IP / 载荷等业务原始数据。
    #[serde(default = "default_guard_intensity", alias = "hit_count")]
    pub intensity: u32,
}

const fn default_guard_intensity() -> u32 {
    1
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelStateReport {
    pub tunnel_id: Uuid,
    /// 端到端已转发或已验证上游（非仅 carrier TLS 建连）。
    pub reachable: bool,
    pub backend_tls_error: bool,
    /// client 与 node 的 carrier 会话已建立（QUIC/TCP 相同语义）。
    #[serde(default)]
    pub carrier_connected: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostMetricsReport {
    pub cpu_usage_percent: u8,
    pub cpu_cores: u16,
    pub memory_used_bytes: u64,
    pub memory_total_bytes: u64,
    pub net_up_bps: u64,
    pub net_down_bps: u64,
    /// 各网卡累计接收字节（Komari Down / 入口）。
    #[serde(default)]
    pub net_rx_bytes_total: u64,
    /// 各网卡累计发送字节（Komari Up / 出口）。
    #[serde(default)]
    pub net_tx_bytes_total: u64,
    /// 主机当前 ESTABLISHED TCP 连接数（含 tcp6）。每次上报覆盖，不含 TIME_WAIT。
    #[serde(default)]
    pub connections_tcp: u32,
    /// 主机 UDP 套接字数（含 udp6；Komari `connections_udp`）。
    #[serde(default)]
    pub connections_udp: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientTrafficReport {
    /// 自 Client 启动以来，经隧道从节点侧进入转发的累计字节（carrier → upstream）。
    pub bytes_in: u64,
    /// 自 Client 启动以来，经隧道发回节点侧的累计字节（upstream → carrier）。
    pub bytes_out: u64,
    /// 上一上报周期内入口增量（用于实时网速）。
    pub bytes_in_delta: u64,
    /// 上一上报周期内出口增量。
    pub bytes_out_delta: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatsReport {
    pub seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq_from: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq_to: Option<u64>,
    /// 本包统计区间起点（Unix 秒，含）。
    #[serde(default)]
    pub period_start_unix: u64,
    /// 本包统计区间终点（Unix 秒，不含），与起点相差 `interval_secs`。
    #[serde(default)]
    pub period_end_unix: u64,
    #[serde(default = "default_stats_interval_secs")]
    pub interval_secs: u32,
    #[serde(default)]
    pub tunnels: Vec<TunnelTrafficSample>,
}

const fn default_stats_interval_secs() -> u32 {
    1
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelTrafficSample {
    pub tunnel_id: Uuid,
    pub bytes_in: u64,
    pub bytes_out: u64,
    #[serde(default)]
    pub active_conns: u32,
    #[serde(default)]
    pub rejects_quota: u32,
    #[serde(default)]
    pub rejects_guard: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthMessage {
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloMessage {
    pub revision: i64,
    pub capabilities: AgentCapabilities,
    pub version: Option<String>,
    pub os: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_ipv4: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_ipv6: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub running_tunnels: Vec<RunningTunnel>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunningTunnel {
    pub tunnel_id: Uuid,
    pub revision: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentCapabilities {
    #[serde(default)]
    pub carriers: Vec<String>,
    #[serde(default)]
    pub ingress: Vec<String>,
    #[serde(default)]
    pub guards: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CsrRequest {
    pub csr_pem: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CertIssued {
    pub certificate_pem: String,
    pub fingerprint: String,
    pub not_after: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeConfig {
    pub node_id: Uuid,
    pub revision: i64,
    pub bind_addr: String,
    pub public_host: String,
    pub carrier_ports: serde_json::Value,
    pub tcp_port_ranges: serde_json::Value,
    pub udp_port_ranges: serde_json::Value,
    pub port_exclude: serde_json::Value,
    pub http_shared_port: i32,
    pub https_shared_port: i32,
    pub guard_policy: serde_json::Value,
    /// 中国大陆节点且全局策略开启：未过白域名禁止 HTTP 隧道，访问时返回备案页。
    #[serde(default)]
    pub cn_http_filing: bool,
    /// 中国大陆节点且全局策略开启：仅允许境内 Client 建立 carrier 连接（按对端 IP 地理校验）。
    #[serde(default)]
    pub cn_residency: bool,
    /// 备案过白域名（apex 及其子域）。
    #[serde(default)]
    pub domain_whitelist: Vec<String>,
    pub trusted_proxies: Vec<String>,
    #[serde(default)]
    pub board_ca_pem: String,
    /// 当前在该节点上有隧道的 client 证书指纹（board 集中维护，CSR 签发后即时更新）。
    #[serde(default)]
    pub authorized_client_fingerprints: Vec<String>,
    /// 节点 agent 证书指纹（与 client 侧 node_cert_fingerprint 对应）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certificate_fingerprint: Option<String>,
    pub tunnels: Vec<TunnelSpec>,
    #[serde(default)]
    pub http_domain_routes: Vec<HttpDomainRoute>,
    #[serde(default)]
    pub tls_certificates: Vec<NodeTlsCertificate>,
    /// 宿主机指标上报间隔（秒），由 board 系统设置下发。
    #[serde(default = "default_host_metrics_interval_secs")]
    pub host_metrics_interval_secs: u32,
}

const fn default_host_metrics_interval_secs() -> u32 {
    1
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeTlsCertificate {
    pub domains: Vec<String>,
    pub certificate_pem: String,
    pub private_key_pem: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpDomainRoute {
    #[serde(default)]
    pub https_enabled: bool,
    pub domain: String,
    pub tunnel_id: Uuid,
    pub client_fingerprint: Option<String>,
    pub speed_limit_mbps: i64,
    pub max_conns: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientConfig {
    pub client_id: Uuid,
    pub board_ca_pem: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certificate_fingerprint: Option<String>,
    pub tunnels: Vec<ClientTunnelAssign>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelSpec {
    #[serde(default)]
    pub https_enabled: bool,
    pub tunnel_id: Uuid,
    pub revision: i64,
    pub protocol: String,
    pub carrier: String,
    pub remote_port: Option<i32>,
    pub target_host: Option<String>,
    pub target_port: Option<i32>,
    pub target_url: Option<String>,
    #[serde(default)]
    pub client_id: Option<Uuid>,
    pub client_fingerprint: Option<String>,
    pub speed_limit_mbps: i64,
    pub max_conns: i32,
    pub max_new_conns_per_sec: i32,
    /// 独立端口 HTTP 隧道已审核通过的域名（小写）；节点据此校验 Host。
    #[serde(default)]
    pub domains: Vec<String>,
    /// TCP carrier 加密共享密钥（hex），与 client 侧 `ClientTunnelAssign.carrier_secret` 相同。
    #[serde(default)]
    pub carrier_secret: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientTunnelAssign {
    pub tunnel_id: Uuid,
    pub node_id: Uuid,
    pub node_public_host: String,
    pub carrier: String,
    pub carrier_port: u16,
    pub protocol: String,
    pub target_host: Option<String>,
    pub target_port: Option<i32>,
    pub target_url: Option<String>,
    pub host_rewrite: Option<String>,
    pub backend_tls_insecure: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_cert_fingerprint: Option<String>,
    /// TCP carrier 加密共享密钥（hex），与 node 侧 `TunnelSpec.carrier_secret` 相同。
    #[serde(default)]
    pub carrier_secret: String,
}

pub fn encode_envelope(payload: &ControlOp) -> Result<Vec<u8>, rmp_serde::encode::Error> {
    let envelope = Envelope {
        version: ProtocolVersion::CURRENT,
        id: 0,
        kind: MessageKind::Event,
        payload,
    };
    rmp_serde::to_vec_named(&envelope)
}

pub fn decode_envelope(bytes: &[u8]) -> Result<Envelope<ControlOp>, rmp_serde::decode::Error> {
    rmp_serde::from_slice(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_compatibility_requires_same_major_version() {
        assert!(ProtocolVersion::CURRENT.is_compatible_with(ProtocolVersion {
            major: 1,
            minor: 99
        }));
        assert!(
            !ProtocolVersion::CURRENT.is_compatible_with(ProtocolVersion { major: 2, minor: 0 })
        );
    }

    #[test]
    fn roundtrip_auth_envelope() {
        let payload = ControlOp::Auth(AuthMessage {
            token: "secret".into(),
        });
        let bytes = encode_envelope(&payload).expect("encode");
        let decoded = decode_envelope(&bytes).expect("decode");
        assert!(matches!(decoded.payload, ControlOp::Auth(_)));
    }
}
