//! 中国大陆节点 residency：按 carrier 对端 IP 地理校验，非 CN 直接拒绝。

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use dashmap::DashMap;
use reqwest::Client;
use serde::Deserialize;
use tracing::warn;

/// 查询成功的结果缓存时长。
const CACHE_TTL: Duration = Duration::from_secs(3600);
/// 查询失败（超时 / 限流 / 解析失败）只短暂缓存，避免一次抖动拒绝该 IP 一小时。
const FAILURE_TTL: Duration = Duration::from_secs(30);
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(5);

static HTTP: OnceLock<Client> = OnceLock::new();
static CACHE: OnceLock<DashMap<IpAddr, CacheEntry>> = OnceLock::new();
static INFLIGHT: OnceLock<DashMap<IpAddr, Arc<tokio::sync::Mutex<()>>>> = OnceLock::new();

#[derive(Clone)]
struct CacheEntry {
    /// 最近一次成功查到的国家码；查询失败时保留旧值，过期后仍可兜底。
    country: Option<String>,
    resolved_at: Option<Instant>,
    failed_at: Option<Instant>,
}

impl CacheEntry {
    fn fresh_country(&self) -> Option<Option<String>> {
        if let Some(at) = self.resolved_at {
            if at.elapsed() < CACHE_TTL {
                return Some(self.country.clone());
            }
        }
        if let Some(at) = self.failed_at {
            if at.elapsed() < FAILURE_TTL {
                return Some(self.country.clone());
            }
        }
        None
    }
}

fn http_client() -> &'static Client {
    HTTP.get_or_init(|| {
        Client::builder()
            .timeout(LOOKUP_TIMEOUT)
            .user_agent("tanzaku-carrier/0.1")
            .build()
            .expect("reqwest client")
    })
}

fn cache() -> &'static DashMap<IpAddr, CacheEntry> {
    CACHE.get_or_init(DashMap::new)
}

fn inflight_lock(ip: IpAddr) -> Arc<tokio::sync::Mutex<()>> {
    INFLIGHT
        .get_or_init(DashMap::new)
        .entry(ip)
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

fn is_lookup_eligible(ip: IpAddr) -> bool {
    !ip.is_unspecified() && !ip.is_loopback() && !ip.is_multicast()
}

fn normalize_country_code(raw: &str) -> Option<String> {
    let code = raw.trim();
    if code.len() != 2 || !code.is_ascii() {
        return None;
    }
    Some(code.to_ascii_uppercase())
}

fn canonical_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        IpAddr::V4(_) => ip,
    }
}

async fn lookup_ip_sb(ip: IpAddr) -> Option<String> {
    #[derive(Deserialize)]
    struct IpSbResponse {
        country_code: Option<String>,
    }
    let url = format!("https://api.ip.sb/geoip/{ip}");
    let response = http_client().get(url).send().await.ok()?.error_for_status().ok()?;
    let body: IpSbResponse = response.json().await.ok()?;
    normalize_country_code(&body.country_code.unwrap_or_default())
}

async fn country_for_ip(ip: IpAddr) -> Option<String> {
    let ip = canonical_ip(ip);
    if !is_lookup_eligible(ip) {
        return None;
    }
    if let Some(country) = cache().get(&ip).and_then(|entry| entry.fresh_country()) {
        return country;
    }
    // 同一 IP 的并发链路（TCP carrier 每隧道 8+1 条）只发一次查询，其余等待结果。
    let lock = inflight_lock(ip);
    let _guard = lock.lock().await;
    if let Some(country) = cache().get(&ip).and_then(|entry| entry.fresh_country()) {
        return country;
    }
    let looked_up = tokio::time::timeout(LOOKUP_TIMEOUT, lookup_ip_sb(ip))
        .await
        .ok()
        .flatten();
    let now = Instant::now();
    let mut entry = cache().entry(ip).or_insert_with(|| CacheEntry {
        country: None,
        resolved_at: None,
        failed_at: None,
    });
    match looked_up {
        Some(country) => {
            entry.country = Some(country);
            entry.resolved_at = Some(now);
            entry.failed_at = None;
        }
        None => {
            if entry.country.is_some() {
                warn!(%ip, "cn residency: geo lookup failed; using last known result");
            }
            entry.failed_at = Some(now);
        }
    }
    entry.country.clone()
}

/// `cn_residency` 开启时校验对端 IP 是否为中国大陆；不符合或无法判定则拒绝。
pub async fn enforce_peer_cn(
    enabled: bool,
    peer: SocketAddr,
) -> Result<(), std::io::Error> {
    if !enabled {
        return Ok(());
    }
    let ip = peer.ip();
    match country_for_ip(ip).await {
        Some(code) if code.eq_ignore_ascii_case("CN") => Ok(()),
        Some(code) => {
            warn!(%peer, %code, "cn residency: reject non-CN carrier peer");
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "cn residency: peer IP is not in China mainland",
            ))
        }
        None => {
            warn!(%peer, "cn residency: reject carrier peer with unknown geo");
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "cn residency: peer IP geo unavailable or not China mainland",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_keeps_last_known_country_briefly() {
        let entry = CacheEntry {
            country: Some("CN".into()),
            resolved_at: Some(Instant::now() - CACHE_TTL - Duration::from_secs(1)),
            failed_at: Some(Instant::now()),
        };
        assert_eq!(entry.fresh_country(), Some(Some("CN".into())));
    }

    #[test]
    fn expired_failure_triggers_new_lookup() {
        let entry = CacheEntry {
            country: None,
            resolved_at: None,
            failed_at: Some(Instant::now() - FAILURE_TTL - Duration::from_secs(1)),
        };
        assert_eq!(entry.fresh_country(), None);
    }

    #[test]
    fn mapped_ipv6_is_canonicalized() {
        let mapped: IpAddr = "::ffff:8.137.116.139".parse().unwrap();
        assert_eq!(canonical_ip(mapped), "8.137.116.139".parse::<IpAddr>().unwrap());
    }
}
