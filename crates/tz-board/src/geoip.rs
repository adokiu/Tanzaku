use std::net::IpAddr;
use std::sync::OnceLock;
use std::time::Duration;

use reqwest::Client;
use serde::Deserialize;
use sqlx::PgPool;
use tracing::warn;

use crate::system_settings::{ClientIpGeoProvider, client_ip_geo_provider};

static HTTP: OnceLock<Client> = OnceLock::new();

fn http_client() -> &'static Client {
    HTTP.get_or_init(|| {
        Client::builder()
            .timeout(Duration::from_secs(8))
            .user_agent("tanzaku-board/0.1")
            .build()
            .expect("reqwest client")
    })
}

/// ISO 3166-1 二字码；由 board 按系统设置调用在线 API。
pub async fn country_code_for_ip(pg: &PgPool, ip: &str) -> Option<String> {
    let ip = ip.trim();
    if !is_lookup_eligible(ip) {
        return None;
    }
    let provider = client_ip_geo_provider(pg).await;
    let lookup = tokio::time::timeout(Duration::from_secs(8), async {
        match provider {
            ClientIpGeoProvider::IpInfo => lookup_ipinfo(ip).await,
            ClientIpGeoProvider::Ip9 => lookup_ip9(ip).await,
            ClientIpGeoProvider::IpSb => lookup_ip_sb(ip).await,
        }
    })
    .await
    .ok()
    .and_then(|result| result.ok());
    lookup.and_then(|code| normalize_country_code(&code))
}

fn is_lookup_eligible(ip: &str) -> bool {
    let Ok(addr) = ip.parse::<IpAddr>() else {
        return false;
    };
    !addr.is_unspecified() && !addr.is_loopback() && !addr.is_multicast()
}

fn normalize_country_code(raw: &str) -> Option<String> {
    let code = raw.trim();
    if code.len() != 2 || !code.is_ascii() {
        return None;
    }
    Some(code.to_ascii_uppercase())
}

async fn lookup_ipinfo(ip: &str) -> Result<String, reqwest::Error> {
    #[derive(Deserialize)]
    struct IpInfoResponse {
        country: Option<String>,
    }
    let url = format!("https://ipinfo.io/{ip}/json");
    let response = http_client().get(url).send().await?.error_for_status()?;
    let body: IpInfoResponse = response.json().await?;
    Ok(body.country.unwrap_or_default())
}

async fn lookup_ip9(ip: &str) -> Result<String, reqwest::Error> {
    #[derive(Deserialize)]
    struct Ip9Envelope {
        ret: i32,
        data: Option<Ip9Data>,
    }
    #[derive(Deserialize)]
    struct Ip9Data {
        country_code: Option<String>,
    }
    let url = format!("https://ip9.com.cn/get?ip={ip}");
    let response = http_client().get(url).send().await?.error_for_status()?;
    let body: Ip9Envelope = response.json().await?;
    if body.ret != 200 {
        return Ok(String::new());
    }
    Ok(body
        .data
        .and_then(|data| data.country_code)
        .unwrap_or_default())
}

async fn lookup_ip_sb(ip: &str) -> Result<String, reqwest::Error> {
    #[derive(Deserialize)]
    struct IpSbResponse {
        country_code: Option<String>,
    }
    let url = format!("https://api.ip.sb/geoip/{ip}");
    let response = http_client().get(url).send().await?.error_for_status()?;
    let body: IpSbResponse = response.json().await?;
    Ok(body.country_code.unwrap_or_default())
}

pub async fn refresh_client_region(pg: &PgPool, client_id: uuid::Uuid, public_ip: &str) {
    let Some(region) = country_code_for_ip(pg, public_ip).await else {
        return;
    };
    if let Err(error) = sqlx::query(
        "UPDATE clients SET region = $2, updated_at = now() WHERE id = $1 AND public_ip = $3",
    )
    .bind(client_id)
    .bind(&region)
    .bind(public_ip.trim())
    .execute(pg)
    .await
    {
        warn!(%client_id, %public_ip, %error, "failed to persist client region from geo API");
    }
}
