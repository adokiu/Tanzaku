//! 从 **board 视角的入站连接** 解析真实客户端 IP（与 YibinServer 一致）：
//! 读反代/CDN 转发头，没有再退回 TCP peer。
//! Client 不上报 IP；只看连到 board 的这一跳请求。

use axum::{
    extract::{ConnectInfo, FromRequestParts},
    http::{HeaderMap, request::Parts, StatusCode},
};
use ipnet::IpNet;
use std::{
    net::{IpAddr, SocketAddr},
    str::FromStr,
    sync::Arc,
};

use crate::setup::AppState;

/// 国内 CDN / 反代常见真实 IP 头（单值头按优先级）。
const SINGLE_IP_HEADERS: &[&str] = &[
    "cf-connecting-ip",
    "true-client-ip",
    "ali-cdn-real-ip",
    "x-tencent-cdn-real-ip",
    "x-tencent-cdn-source-ip",
    "cdn-src-ip",
    "x-cdn-src-ip",
    "x-azure-clientip",
    "fastly-client-ip",
    "x-real-ip",
    "x-client-ip",
    "x-original-forwarded-for",
    "x-cluster-client-ip",
    "wl-proxy-client-ip",
    "x-true-ip",
];

#[derive(Debug, Clone)]
pub struct TrustedProxies {
    nets: Vec<IpNet>,
}

impl TrustedProxies {
    pub fn from_cidrs(cidrs: &[String]) -> Self {
        let mut nets = Vec::new();
        for raw in cidrs {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Ok(net) = IpNet::from_str(trimmed) {
                nets.push(net);
                continue;
            }
            if let Ok(ip) = IpAddr::from_str(trimmed) {
                nets.push(IpNet::from(ip));
            }
        }
        if nets.is_empty() {
            Self::default_list()
        } else {
            Self { nets }
        }
    }

    pub fn default_list() -> Self {
        Self::from_cidrs(&default_trusted_proxy_cidrs())
    }
}

pub fn default_trusted_proxy_cidrs() -> Vec<String> {
    vec![
        "127.0.0.0/8".into(),
        "::1/128".into(),
        "10.0.0.0/8".into(),
        "172.16.0.0/12".into(),
        "192.168.0.0/16".into(),
        "fc00::/7".into(),
        "fe80::/10".into(),
    ]
}

/// 解析真实客户端 IP：转发头优先，否则 peer。
pub fn resolve(peer: SocketAddr, headers: &HeaderMap, _trusted: &TrustedProxies) -> IpAddr {
    if let Some(ip) = extract_from_headers(headers) {
        return ip;
    }
    peer.ip()
}

fn extract_from_headers(headers: &HeaderMap) -> Option<IpAddr> {
    for name in SINGLE_IP_HEADERS {
        if let Some(ip) = header_joined(headers, name).and_then(|value| parse_ip_token(&value)) {
            return Some(ip);
        }
    }
    // X-Forwarded-For：合并所有同名头，自左向右取第一个公网 IP（跳过私网/回环）
    if let Some(value) = header_joined(headers, "x-forwarded-for") {
        if let Some(ip) = first_public_forwarded_for(&value) {
            return Some(ip);
        }
        if let Some(ip) = first_forwarded_for(&value) {
            return Some(ip);
        }
    }
    if let Some(value) = header_joined(headers, "forwarded") {
        if let Some(ip) = parse_forwarded_for(&value) {
            return Some(ip);
        }
    }
    None
}

fn header_joined(headers: &HeaderMap, name: &str) -> Option<String> {
    let mut parts = Vec::new();
    for value in headers.get_all(name) {
        if let Ok(text) = value.to_str() {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                parts.push(trimmed);
            }
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(","))
    }
}

fn first_forwarded_for(value: &str) -> Option<IpAddr> {
    for part in value.split(',') {
        if let Some(ip) = parse_ip_token(part) {
            return Some(ip);
        }
    }
    None
}

fn first_public_forwarded_for(value: &str) -> Option<IpAddr> {
    for part in value.split(',') {
        if let Some(ip) = parse_ip_token(part).filter(|ip| is_global_ip(*ip)) {
            return Some(ip);
        }
    }
    None
}

fn is_global_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            !(v4.is_unspecified()
                || v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_multicast())
        }
        IpAddr::V6(v6) => {
            !(v6.is_unspecified()
                || v6.is_loopback()
                || v6.is_multicast()
                || (v6.segments()[0] & 0xffc0) == 0xfe80
                || (v6.segments()[0] & 0xfe00) == 0xfc00)
        }
    }
}

fn parse_forwarded_for(value: &str) -> Option<IpAddr> {
    for segment in value.split(',') {
        for pair in segment.split(';') {
            let pair = pair.trim();
            let Some(rest) = pair
                .strip_prefix("for=")
                .or_else(|| pair.strip_prefix("For="))
                .or_else(|| pair.strip_prefix("FOR="))
            else {
                continue;
            };
            if let Some(ip) = parse_ip_token(rest).filter(|ip| is_global_ip(*ip)) {
                return Some(ip);
            }
            if let Some(ip) = parse_ip_token(rest) {
                return Some(ip);
            }
        }
    }
    None
}

fn parse_ip_token(raw: &str) -> Option<IpAddr> {
    let mut token = raw.trim().trim_matches('"').trim();
    if token.eq_ignore_ascii_case("unknown") || token.is_empty() {
        return None;
    }
    if let Some(inner) = token.strip_prefix('[').and_then(|value| {
        let end = value.find(']')?;
        Some(&value[..end])
    }) {
        return IpAddr::from_str(inner).ok();
    }
    if let Ok(addr) = SocketAddr::from_str(token) {
        return Some(addr.ip());
    }
    if let Some((host, port)) = token.rsplit_once(':') {
        if port.chars().all(|ch| ch.is_ascii_digit())
            && host.chars().all(|ch| ch.is_ascii_digit() || ch == '.')
        {
            token = host;
        }
    }
    IpAddr::from_str(token).ok()
}

#[derive(Debug, Clone, Copy)]
pub struct ClientIp(pub IpAddr);

impl ClientIp {
    pub fn as_str(&self) -> String {
        self.0.to_string()
    }
}

impl std::fmt::Display for ClientIp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromRequestParts<Arc<AppState>> for ClientIp {
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|info| info.0)
            .ok_or((StatusCode::INTERNAL_SERVER_ERROR, "missing connect info"))?;
        Ok(ClientIp(resolve(
            peer,
            &parts.headers,
            state.trusted_proxies(),
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.append(
                axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        map
    }

    #[test]
    fn prefers_forwarded_public_ip_from_cdn_peer() {
        let trusted = TrustedProxies::default_list();
        let peer = "203.0.113.50:443".parse().unwrap();
        let hdrs = headers(&[("x-forwarded-for", "198.51.100.7, 203.0.113.50")]);
        assert_eq!(
            resolve(peer, &hdrs, &trusted),
            "198.51.100.7".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn skips_private_prefix_in_xff() {
        let trusted = TrustedProxies::default_list();
        let peer = "203.0.113.50:443".parse().unwrap();
        let hdrs = headers(&[("x-forwarded-for", "10.0.0.8, 198.51.100.7, 203.0.113.50")]);
        assert_eq!(
            resolve(peer, &hdrs, &trusted),
            "198.51.100.7".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn uses_ali_cdn_header() {
        let trusted = TrustedProxies::default_list();
        let peer = "203.0.113.50:80".parse().unwrap();
        let hdrs = headers(&[
            ("ali-cdn-real-ip", "198.51.100.9"),
            ("x-forwarded-for", "203.0.113.50"),
        ]);
        assert_eq!(
            resolve(peer, &hdrs, &trusted),
            "198.51.100.9".parse::<IpAddr>().unwrap()
        );
    }
}
