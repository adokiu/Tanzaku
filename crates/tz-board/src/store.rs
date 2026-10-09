use crate::{
    config::{PgConfig, RedisConfig},
    port_cache::RedisPortCache,
};
use anyhow::{Context, bail};
use argon2::{PasswordHasher, PasswordVerifier};
use redis::aio::ConnectionManager;
use sqlx::{
    ConnectOptions, PgPool,
    postgres::{PgConnectOptions, PgPoolOptions, PgSslMode},
};
use std::time::Duration;

/// 将单一 `max_connections` 拆成 API / 控制 WS / 指标结算 三个池，避免互相抢连接。
#[derive(Clone)]
pub struct DbPools {
    api: PgPool,
    control: PgPool,
    metrics: PgPool,
}

impl DbPools {
    pub async fn connect(config: &PgConfig) -> anyhow::Result<Self> {
        config.validate()?;
        let total = config.max_connections.max(6);
        let api_cap = (total / 2).max(2);
        let control_cap = (total.saturating_sub(api_cap) / 2).max(2);
        let metrics_cap = total
            .saturating_sub(api_cap)
            .saturating_sub(control_cap)
            .max(2);
        Ok(Self {
            api: connect_postgres_with(config, api_cap, "tanzaku-board-api").await?,
            control: connect_postgres_with(config, control_cap, "tanzaku-board-control").await?,
            metrics: connect_postgres_with(config, metrics_cap, "tanzaku-board-metrics").await?,
        })
    }

    pub fn api(&self) -> PgPool {
        self.api.clone()
    }

    pub fn control(&self) -> PgPool {
        self.control.clone()
    }

    pub fn metrics(&self) -> PgPool {
        self.metrics.clone()
    }
}

pub async fn connect_postgres(config: &PgConfig) -> anyhow::Result<PgPool> {
    connect_postgres_with(config, config.max_connections.max(1), "tanzaku-board").await
}

async fn connect_postgres_with(
    config: &PgConfig,
    max_connections: u32,
    application_name: &str,
) -> anyhow::Result<PgPool> {
    config.validate()?;
    let ssl_mode = match config.sslmode.as_str() {
        "disable" => PgSslMode::Disable,
        "prefer" => PgSslMode::Prefer,
        "require" => PgSslMode::Require,
        "verify-ca" => PgSslMode::VerifyCa,
        "verify-full" => PgSslMode::VerifyFull,
        _ => bail!("unsupported PostgreSQL SSL mode"),
    };
    let options = PgConnectOptions::new_without_pgpass()
        .host(&config.host)
        .port(config.port)
        .database(&config.database)
        .username(&config.user)
        .password(&config.password)
        .application_name(application_name)
        .ssl_mode(ssl_mode)
        .log_statements(log::LevelFilter::Off)
        .log_slow_statements(log::LevelFilter::Off, Duration::ZERO);
    PgPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(Duration::from_secs(5))
        .connect_with(options)
        .await
        .context("failed to connect to PostgreSQL")
}

pub async fn connect_redis(config: &RedisConfig) -> anyhow::Result<ConnectionManager> {
    let url = config.connection_url()?;
    let client = redis::Client::open(url).context("invalid Redis connection settings")?;
    let mut manager = ConnectionManager::new(client)
        .await
        .context("failed to connect to Redis")?;
    let _: String = redis::cmd("PING")
        .query_async(&mut manager)
        .await
        .context("Redis health check failed")?;
    Ok(manager)
}

pub async fn migrate(pool: &PgPool) -> anyhow::Result<()> {
    sqlx::migrate!()
        .run(pool)
        .await
        .context("database migration failed")
}

pub async fn verify_credentials(
    pool: &PgPool,
    email: &str,
    password: &str,
) -> anyhow::Result<bool> {
    let hash = sqlx::query_scalar::<_, String>(
        "SELECT password_hash FROM users WHERE email = $1 AND role = 'admin' AND status = 'active'",
    )
    .bind(email)
    .fetch_optional(pool)
    .await?;
    let Some(hash) = hash else { return Ok(false) };
    let parsed = argon2::PasswordHash::new(&hash).map_err(|_| anyhow::anyhow!("stored password hash is invalid"))?;
    Ok(argon2::Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

pub async fn initialize_database(pool: &PgPool, email: &str, password: &str) -> anyhow::Result<()> {
    let mut transaction = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(0x5441_4e5a_414b_5501_i64)
        .execute(&mut *transaction)
        .await?;
    let installed: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM system_settings WHERE key = 'installation_state')",
    )
    .fetch_one(&mut *transaction)
    .await?;
    if installed {
        transaction.rollback().await?;
        if verify_credentials(pool, email, password).await? {
            return Ok(());
        }
        bail!(
            "board has already been initialized; use the existing administrator credentials to resume setup"
        );
    }

    let password_hash = hash_password(password)?;
    let (certificate_pem, private_key_pem) = create_ca()?;
    let mut session_key = [0u8; 32];
    getrandom::fill(&mut session_key).context("failed to generate session key")?;
    let session_key_hex = session_key
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();

    sqlx::query("INSERT INTO users (id, email, password_hash, role) VALUES ($1, $2, $3, 'admin')")
        .bind(uuid::Uuid::new_v4())
        .bind(email)
        .bind(password_hash)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("INSERT INTO pki_ca (id, certificate_pem, private_key_pem) VALUES (1, $1, $2)")
        .bind(certificate_pem)
        .bind(private_key_pem)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("INSERT INTO system_settings (key, value) VALUES ('installation_state', '\"ready\"'::jsonb)")
        .execute(&mut *transaction)
        .await?;
    sqlx::query(
        "INSERT INTO system_settings (key, value) VALUES ('session_key', to_jsonb($1::text))",
    )
    .bind(session_key_hex)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO system_settings (key, value) VALUES
         ('registration_enabled', 'false'::jsonb),
         ('node_host_metrics_interval_secs', '1'::jsonb),
         ('client_ip_geo_provider', '\"ipinfo\"'::jsonb),
         ('site_title', '\"\"'::jsonb),
         ('site_subtitle', '\"\"'::jsonb),
         ('site_description', '\"\"'::jsonb),
         ('site_url', '\"\"'::jsonb),
         ('trial_plan_id', 'null'::jsonb),
         ('trial_duration_days', '7'::jsonb),
         ('traffic_reset_mode', '\"month_purchase\"'::jsonb),
         ('security_email_verification', 'false'::jsonb),
         ('security_safe_mode', 'false'::jsonb),
         ('security_email_suffix_whitelist_enabled', 'false'::jsonb),
         ('security_email_suffix_whitelist', '[]'::jsonb),
         ('security_captcha_enabled', 'false'::jsonb),
         ('security_ip_register_limit_enabled', 'false'::jsonb),
         ('security_ip_register_max_count', '3'::jsonb),
         ('security_ip_register_window_minutes', '60'::jsonb),
         ('security_password_attempt_limit_enabled', 'true'::jsonb),
         ('security_password_attempt_max', '5'::jsonb),
         ('security_password_lock_minutes', '0'::jsonb),
         ('mail_smtp_host', '\"\"'::jsonb),
         ('mail_smtp_port', '465'::jsonb),
         ('mail_smtp_encryption', '\"ssl\"'::jsonb),
         ('mail_smtp_username', '\"\"'::jsonb),
         ('mail_smtp_password', '\"\"'::jsonb),
         ('mail_from_address', '\"\"'::jsonb),
         ('mail_notify_enabled', 'false'::jsonb)
         ON CONFLICT (key) DO NOTHING",
    )
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(())
}

/// 长度 6–32，且数字 / 大写 / 小写 / 特殊字符四类中至少满足两类。
pub fn validate_password(password: &str) -> Result<(), &'static str> {
    if password.len() < 6 || password.len() > 32 {
        return Err("密码长度须为 6 到 32 个字符");
    }
    let mut classes = 0u8;
    if password.chars().any(|c| c.is_ascii_lowercase()) {
        classes += 1;
    }
    if password.chars().any(|c| c.is_ascii_uppercase()) {
        classes += 1;
    }
    if password.chars().any(|c| c.is_ascii_digit()) {
        classes += 1;
    }
    if password.chars().any(|c| !c.is_ascii_alphanumeric()) {
        classes += 1;
    }
    if classes < 2 {
        return Err("密码须包含数字、大写字母、小写字母、特殊字符中的至少两种");
    }
    Ok(())
}

pub fn hash_password(password: &str) -> anyhow::Result<String> {
    let salt = create_salt()?;
    Ok(argon2::Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|_| anyhow::anyhow!("failed to hash password"))?
        .to_string())
}

fn create_salt() -> anyhow::Result<argon2::password_hash::SaltString> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).context("failed to generate password salt")?;
    argon2::password_hash::SaltString::encode_b64(&bytes).map_err(|_| anyhow::anyhow!("failed to encode password salt"))
}

fn create_ca() -> anyhow::Result<(String, String)> {
    let mut params = rcgen::CertificateParams::default();
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    params.key_usages = vec![
        rcgen::KeyUsagePurpose::KeyCertSign,
        rcgen::KeyUsagePurpose::CrlSign,
    ];
    let key_pair = rcgen::KeyPair::generate().context("failed to generate internal CA key")?;
    let certificate = params
        .self_signed(&key_pair)
        .context("failed to create internal CA certificate")?;
    Ok((certificate.pem(), key_pair.serialize_pem()))
}

pub async fn rebuild_port_maps(pool: &PgPool, redis: &ConnectionManager) -> anyhow::Result<()> {
    let node_ids = sqlx::query_scalar::<_, uuid::Uuid>("SELECT id FROM nodes")
        .fetch_all(pool)
        .await?;
    for node_id in node_ids {
        rebuild_node_port_map(pool, redis, node_id).await?;
    }
    sqlx::query("DELETE FROM redis_outbox WHERE operation = 'rebuild_node_ports'")
        .execute(pool)
        .await?;
    let mut redis = redis.clone();
    let epoch = uuid::Uuid::new_v4().to_string();
    let _: Result<(), _> = redis::AsyncCommands::set(&mut redis, "meta:epoch", &epoch).await;
    let _: Result<(), _> = redis::AsyncCommands::set(
        &mut redis,
        "meta:ports:built_at",
        chrono::Utc::now().to_rfc3339(),
    )
    .await;
    Ok(())
}

pub async fn rebuild_node_port_map(
    pool: &PgPool,
    redis: &ConnectionManager,
    node_id: uuid::Uuid,
) -> anyhow::Result<()> {
    let mut transaction = pool.begin().await?;
    let lock_key = format!("tunnel-ports:{node_id}");
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 42))")
        .bind(lock_key)
        .execute(&mut *transaction)
        .await?;
    rebuild_node_port_map_tx(&mut transaction, redis, node_id).await?;
    transaction.commit().await?;
    Ok(())
}

pub async fn rebuild_node_port_map_tx(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    redis: &ConnectionManager,
    node_id: uuid::Uuid,
) -> anyhow::Result<()> {
    use sqlx::Row;

    let node = sqlx::query(
        "SELECT tcp_port_ranges, udp_port_ranges, port_exclude, protocols, carrier_ports, http_shared_port, https_shared_port FROM nodes WHERE id = $1",
    )
    .bind(node_id)
    .fetch_one(&mut **transaction)
    .await?;
    let tcp_ranges = decode_port_ranges(node.try_get("tcp_port_ranges")?)?;
    let udp_ranges = decode_port_ranges(node.try_get("udp_port_ranges")?)?;
    let exclusions = decode_port_ranges(node.try_get("port_exclude")?)?;
    let carrier_ports: serde_json::Value = node.try_get("carrier_ports")?;
    let protocols: Vec<String> = node.try_get("protocols")?;
    let mut tcp_reserved = Vec::new();
    if protocols.iter().any(|protocol| protocol == "http") {
        tcp_reserved.push(node.try_get::<i32, _>("http_shared_port")? as u16);
        tcp_reserved.push(node.try_get::<i32, _>("https_shared_port")? as u16);
    }
    let mut udp_reserved = Vec::new();
    tcp_reserved.extend(reserved_ports_for_socket(&carrier_ports, "tcp"));
    udp_reserved.extend(reserved_ports_for_socket(&carrier_ports, "udp"));
    let tunnels = sqlx::query_as::<_, (String, i32)>(
        "SELECT protocol, remote_port FROM tunnels WHERE node_id = $1 AND remote_port IS NOT NULL AND status <> 'deleted'",
    )
    .bind(node_id)
    .fetch_all(&mut **transaction)
    .await?;
    let mut tcp_tunnels = Vec::new();
    let mut udp_tunnels = Vec::new();
    for (protocol, port) in tunnels {
        let port = u16::try_from(port).context("stored tunnel port is invalid")?;
        if protocol == "udp" {
            udp_tunnels.push(port);
        } else {
            tcp_tunnels.push(port);
        }
    }
    RedisPortCache::new(redis.clone())
        .rebuild_node(
            node_id,
            &tcp_ranges,
            &udp_ranges,
            &exclusions,
            &tcp_reserved,
            &udp_reserved,
            &tcp_tunnels,
            &udp_tunnels,
        )
        .await?;
    Ok(())
}

pub(crate) fn decode_port_ranges(
    value: serde_json::Value,
) -> anyhow::Result<tz_common::ports::PortRanges> {
    use tz_common::ports::{PortRange, PortRanges};
    let ranges = value
        .as_array()
        .context("port ranges must be a JSON array")?
        .iter()
        .map(|range| {
            let pair = range
                .as_array()
                .filter(|pair| pair.len() == 2)
                .context("each port range must contain two endpoints")?;
            let start = pair[0]
                .as_u64()
                .context("port range start must be an integer")?;
            let end = pair[1]
                .as_u64()
                .context("port range end must be an integer")?;
            Ok(PortRange {
                start: u16::try_from(start).context("port range start is out of range")?,
                end: u16::try_from(end).context("port range end is out of range")?,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    if ranges.is_empty() {
        return Ok(PortRanges::empty());
    }
    PortRanges::new(ranges).context("stored port ranges are invalid")
}

pub(crate) fn reserved_ports_for_socket(config: &serde_json::Value, socket: &str) -> Vec<u16> {
    tz_carrier::registered_carriers()
        .into_iter()
        .filter(|carrier| carrier.socket == socket)
        .filter_map(|carrier| carrier_port(config, carrier.kind))
        .collect()
}

pub(crate) fn carrier_port(config: &serde_json::Value, kind: &str) -> Option<u16> {
    let carrier = config.get(kind)?;
    if carrier.get("enabled").and_then(serde_json::Value::as_bool) == Some(false) {
        return None;
    }
    u16::try_from(carrier.get("port")?.as_u64()?).ok()
}
