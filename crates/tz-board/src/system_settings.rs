use serde_json::Value;
use sqlx::PgPool;

/// 同时驱动 Node 宿主机指标与隧道 Stats（实时速率/待结算流量）上报间隔（秒）。
const HOST_METRICS_INTERVAL_KEY: &str = "node_host_metrics_interval_secs";
const CLIENT_IP_GEO_PROVIDER_KEY: &str = "client_ip_geo_provider";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientIpGeoProvider {
    IpInfo,
    Ip9,
    IpSb,
}

impl ClientIpGeoProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::IpInfo => "ipinfo",
            Self::Ip9 => "ip9",
            Self::IpSb => "ip_sb",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "ipinfo" | "ipinfo.io" => Some(Self::IpInfo),
            "ip9" | "ip9.com.cn" => Some(Self::Ip9),
            "ip_sb" | "ip.sb" => Some(Self::IpSb),
            _ => None,
        }
    }
}

pub async fn host_metrics_interval_secs(pg: &PgPool) -> u32 {
    let value: Option<Value> =
        sqlx::query_scalar("SELECT value FROM system_settings WHERE key = $1")
            .bind(HOST_METRICS_INTERVAL_KEY)
            .fetch_optional(pg)
            .await
            .ok()
            .flatten();
    value
        .and_then(|value| value.as_u64())
        .map(|seconds| seconds.clamp(1, 3600) as u32)
        .unwrap_or(1)
}

pub async fn client_ip_geo_provider(pg: &PgPool) -> ClientIpGeoProvider {
    let value: Option<Value> =
        sqlx::query_scalar("SELECT value FROM system_settings WHERE key = $1")
            .bind(CLIENT_IP_GEO_PROVIDER_KEY)
            .fetch_optional(pg)
            .await
            .ok()
            .flatten();
    value
        .and_then(|value| value.as_str().map(str::to_owned))
        .and_then(|raw| ClientIpGeoProvider::parse(&raw))
        .unwrap_or(ClientIpGeoProvider::IpInfo)
}

pub fn is_host_metrics_interval_key(key: &str) -> bool {
    key == HOST_METRICS_INTERVAL_KEY
}

pub fn is_client_ip_geo_provider_key(key: &str) -> bool {
    key == CLIENT_IP_GEO_PROVIDER_KEY
}

pub fn validate_host_metrics_interval(value: &Value) -> bool {
    value
        .as_u64()
        .is_some_and(|seconds| (1..=3600).contains(&seconds))
}

pub fn validate_client_ip_geo_provider(value: &Value) -> bool {
    value
        .as_str()
        .is_some_and(|raw| ClientIpGeoProvider::parse(raw).is_some())
}
