use serde_json::{Value, json};
use sqlx::PgPool;

/// 同时驱动 Node 宿主机指标与隧道 Stats（实时速率/待结算流量）上报间隔（秒）。
const HOST_METRICS_INTERVAL_KEY: &str = "node_host_metrics_interval_secs";
const CLIENT_IP_GEO_PROVIDER_KEY: &str = "client_ip_geo_provider";

/// 管理端列表不展示的内部运行时键（装机状态 / session / 主题）。
pub const HIDDEN_SETTING_KEYS: &[&str] = &["installation_state", "session_key", "user_theme"];

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

fn is_string(value: &Value) -> bool {
    value.is_string()
}

fn is_bool(value: &Value) -> bool {
    value.is_boolean()
}

fn is_u64_range(value: &Value, min: u64, max: u64) -> bool {
    value.as_u64().is_some_and(|n| (min..=max).contains(&n))
}

fn is_string_array(value: &Value) -> bool {
    value
        .as_array()
        .is_some_and(|items| items.iter().all(|item| item.is_string()))
}

fn is_optional_uuid_string(value: &Value) -> bool {
    if value.is_null() {
        return true;
    }
    value
        .as_str()
        .is_some_and(|raw| raw.is_empty() || uuid::Uuid::parse_str(raw).is_ok())
}

fn is_mail_encryption(value: &Value) -> bool {
    matches!(
        value.as_str().map(str::to_ascii_lowercase).as_deref(),
        Some("none" | "starttls" | "ssl" | "ssl/tls" | "tls")
    )
}

/// 校验管理端可写设置；未知 key 返回 false。
pub fn validate_writable_setting(key: &str, value: &Value) -> bool {
    match key {
        "registration_enabled" => is_bool(value),
        key if is_host_metrics_interval_key(key) => validate_host_metrics_interval(value),
        key if is_client_ip_geo_provider_key(key) => validate_client_ip_geo_provider(value),
        "site_title" | "site_subtitle" | "site_description" | "site_url" => is_string(value),
        "trial_plan_id" => is_optional_uuid_string(value),
        "trial_duration_days" => is_u64_range(value, 0, 3650),
        "traffic_reset_mode" => value
            .as_str()
            .is_some_and(crate::subscription_period::is_system_reset_mode),
        "security_email_verification"
        | "security_safe_mode"
        | "security_email_suffix_whitelist_enabled"
        | "security_captcha_enabled"
        | "security_ip_register_limit_enabled"
        | "security_password_attempt_limit_enabled"
        | "mail_notify_enabled" => is_bool(value),
        "security_email_suffix_whitelist" => is_string_array(value),
        "security_ip_register_max_count" => is_u64_range(value, 1, 10_000),
        "security_ip_register_window_minutes" => is_u64_range(value, 1, 525_600),
        "security_password_attempt_max" => is_u64_range(value, 1, 100),
        "security_password_lock_minutes" => is_u64_range(value, 0, 525_600),
        "mail_smtp_host" | "mail_smtp_username" | "mail_smtp_password" | "mail_from_address" => {
            is_string(value)
        }
        "mail_smtp_port" => is_u64_range(value, 1, 65535),
        "mail_smtp_encryption" => is_mail_encryption(value),
        _ => false,
    }
}

pub fn is_hidden_setting_key(key: &str) -> bool {
    HIDDEN_SETTING_KEYS.contains(&key)
}

/// 列表脱敏：SMTP 密码非空时只返回占位标记，不回传明文。
pub fn redact_setting_value(key: &str, value: Value) -> Value {
    if key == "mail_smtp_password" {
        let set = value.as_str().is_some_and(|s| !s.is_empty());
        return json!({ "set": set });
    }
    value
}

#[derive(Debug, Clone)]
pub struct SiteBranding {
    pub site_title: String,
    pub site_subtitle: String,
    pub site_description: String,
    pub site_url: String,
}

#[derive(Debug, Clone, Copy)]
pub struct PasswordAttemptPolicy {
    pub enabled: bool,
    pub max_attempts: u32,
    pub lock_minutes: u32,
}

async fn setting_json(pg: &PgPool, key: &str) -> Option<Value> {
    sqlx::query_scalar("SELECT value FROM system_settings WHERE key = $1")
        .bind(key)
        .fetch_optional(pg)
        .await
        .ok()
        .flatten()
}

fn json_string(value: Option<Value>) -> String {
    value
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn json_bool(value: Option<Value>, default: bool) -> bool {
    value.and_then(|v| v.as_bool()).unwrap_or(default)
}

fn json_u32(value: Option<Value>, default: u32) -> u32 {
    value
        .and_then(|v| v.as_u64())
        .map(|n| n.min(u64::from(u32::MAX)) as u32)
        .unwrap_or(default)
}

pub async fn traffic_reset_mode(pg: &PgPool) -> String {
    parse_traffic_reset_mode(setting_json(pg, "traffic_reset_mode").await)
}

pub fn parse_traffic_reset_mode(value: Option<Value>) -> String {
    value
        .and_then(|v| v.as_str().map(str::to_owned))
        .filter(|raw| crate::subscription_period::is_system_reset_mode(raw))
        .unwrap_or_else(|| "month_purchase".into())
}

pub async fn site_branding(pg: &PgPool) -> SiteBranding {
    SiteBranding {
        site_title: json_string(setting_json(pg, "site_title").await),
        site_subtitle: json_string(setting_json(pg, "site_subtitle").await),
        site_description: json_string(setting_json(pg, "site_description").await),
        site_url: json_string(setting_json(pg, "site_url").await),
    }
}

pub async fn password_attempt_policy(pg: &PgPool) -> PasswordAttemptPolicy {
    PasswordAttemptPolicy {
        enabled: json_bool(
            setting_json(pg, "security_password_attempt_limit_enabled").await,
            false,
        ),
        max_attempts: json_u32(setting_json(pg, "security_password_attempt_max").await, 5).max(1),
        lock_minutes: json_u32(setting_json(pg, "security_password_lock_minutes").await, 0),
    }
}
