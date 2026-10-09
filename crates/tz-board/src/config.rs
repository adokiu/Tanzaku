use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use std::{env, ffi::OsStr, fs, net::SocketAddr, path::{Path, PathBuf}};
use tokio::net::TcpListener;

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BoardConfig {
    pub listen: ListenConfig,
    pub postgres: Option<PgConfig>,
    pub redis: Option<RedisConfig>,
    /// 保留字段（兼容旧配置）。真实 IP 始终优先读转发头，与是否在此列表无关。
    #[serde(default = "crate::client_ip::default_trusted_proxy_cidrs")]
    pub trusted_proxies: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ListenConfig {
    /// 管理前端 / API + server（node）控制面 WebSocket（`/ws`）。
    pub admin: String,
    /// 用户前端 / API + client（agent）控制面 WebSocket（`/ws`）。
    pub user: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PgConfig {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub user: String,
    pub password: String,
    pub sslmode: String,
    pub max_connections: u32,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct RedisConfig {
    pub host: String,
    pub port: u16,
    pub database: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    pub tls: bool,
}

pub struct Listeners {
    pub admin: TcpListener,
    pub user: TcpListener,
}

impl Default for BoardConfig {
    fn default() -> Self {
        Self {
            listen: ListenConfig::default(),
            postgres: None,
            redis: None,
            trusted_proxies: crate::client_ip::default_trusted_proxy_cidrs(),
        }
    }
}

impl Default for ListenConfig {
    fn default() -> Self {
        Self {
            admin: "0.0.0.0:9000".into(),
            user: "0.0.0.0:9001".into(),
        }
    }
}

/// 可执行文件同级目录下的 `board.toml`（无法解析 exe 时用 `./board.toml`）。
pub fn default_config_path() -> PathBuf {
    env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|dir| dir.join("board.toml")))
        .unwrap_or_else(|| PathBuf::from("board.toml"))
}

/// 命令行 `-c` / `--config` > 环境变量 `TANZAKU_CONFIG` > [`default_config_path`].
pub fn resolve_config_path() -> anyhow::Result<PathBuf> {
    let mut args = env::args_os().skip(1);
    while let Some(arg) = args.next() {
        let flag = arg.as_os_str();
        if flag == OsStr::new("-c") || flag == OsStr::new("--config") {
            let path = args
                .next()
                .context("usage: tz-board [-c|--config <path>]")?;
            return Ok(PathBuf::from(path));
        }
        if let Some(path) = arg.to_str().and_then(|value| value.strip_prefix("-c")) {
            if !path.is_empty() {
                return Ok(PathBuf::from(path));
            }
        }
        if flag == OsStr::new("-h") || flag == OsStr::new("--help") {
            bail!("usage: tz-board [-c|--config <path>]");
        }
        bail!("usage: tz-board [-c|--config <path>]");
    }
    if let Ok(path) = env::var("TANZAKU_CONFIG") {
        return Ok(PathBuf::from(path));
    }
    Ok(default_config_path())
}

impl BoardConfig {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let mut config = if path.exists() {
            let source = fs::read_to_string(path).context("failed to read board configuration")?;
            toml::from_str(&source).context("failed to parse board configuration")?
        } else {
            Self::default()
        };
        config.apply_environment_overrides()?;
        config.validate_listeners()?;
        Ok(config)
    }

    pub fn with_database(listen: ListenConfig, postgres: PgConfig, redis: RedisConfig) -> Self {
        Self {
            listen,
            postgres: Some(postgres),
            redis: Some(redis),
            trusted_proxies: crate::client_ip::default_trusted_proxy_cidrs(),
        }
    }

    /// 只写入/更新 `[postgres]` 与 `[redis]`；若文件中已有 `[listen]` 原样保留，绝不写入或修改监听配置。
    pub fn persist_database_config(
        path: &Path,
        postgres: &PgConfig,
        redis: &RedisConfig,
        replace: bool,
    ) -> anyhow::Result<()> {
        if path.exists() && !replace {
            bail!("board configuration already exists");
        }
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).context("failed to create configuration directory")?;
        }
        let mut root = if path.exists() {
            let source = fs::read_to_string(path).context("failed to read board configuration")?;
            match toml::from_str::<toml::Table>(&source) {
                Ok(table) => table,
                Err(_) => toml::map::Map::new(),
            }
        } else {
            toml::map::Map::new()
        };
        root.insert(
            "postgres".into(),
            toml::Value::try_from(postgres).context("failed to encode postgres configuration")?,
        );
        root.insert(
            "redis".into(),
            toml::Value::try_from(redis).context("failed to encode redis configuration")?,
        );
        let source =
            toml::to_string(&toml::Value::Table(root)).context("failed to serialize board configuration")?;
        write_atomic(path, source.as_bytes())
    }

    fn apply_environment_overrides(&mut self) -> anyhow::Result<()> {
        for (key, value) in [
            ("TANZAKU_LISTEN_ADMIN", &mut self.listen.admin),
            ("TANZAKU_LISTEN_USER", &mut self.listen.user),
        ] {
            if let Ok(override_value) = env::var(key) {
                *value = override_value;
            }
        }
        Ok(())
    }

    fn validate_listeners(&self) -> anyhow::Result<()> {
        let addresses = [&self.listen.admin, &self.listen.user];
        let mut parsed = Vec::with_capacity(addresses.len());
        for address in addresses {
            parsed.push(
                address
                    .parse::<SocketAddr>()
                    .context("invalid listener address in board configuration")?,
            );
        }
        if parsed[0] == parsed[1] {
            bail!("board listener addresses must be unique");
        }
        Ok(())
    }
}

impl PgConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.host.trim().is_empty()
            || self.database.trim().is_empty()
            || self.user.trim().is_empty()
        {
            bail!("PostgreSQL host, database and username are required");
        }
        if self.port == 0 || !(1..=256).contains(&self.max_connections) {
            bail!("invalid PostgreSQL port or connection pool size");
        }
        if !matches!(
            self.sslmode.as_str(),
            "disable" | "prefer" | "require" | "verify-ca" | "verify-full"
        ) {
            bail!("unsupported PostgreSQL SSL mode");
        }
        Ok(())
    }
}

impl RedisConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.host.trim().is_empty() || self.port == 0 {
            bail!("Redis host and a valid port are required");
        }
        Ok(())
    }

    pub fn connection_url(&self) -> anyhow::Result<String> {
        self.validate()?;
        let scheme = if self.tls { "rediss" } else { "redis" };
        let mut url = url::Url::parse(&format!("{scheme}://localhost/{}", self.database))?;
        url.set_host(Some(&self.host))
            .map_err(|_| anyhow::anyhow!("invalid Redis host"))?;
        url.set_port(Some(self.port))
            .map_err(|_| anyhow::anyhow!("invalid Redis port"))?;
        if let Some(password) = self
            .password
            .as_deref()
            .filter(|password| !password.is_empty())
        {
            url.set_username("")
                .map_err(|_| anyhow::anyhow!("invalid Redis credentials"))?;
            url.set_password(Some(password))
                .map_err(|_| anyhow::anyhow!("invalid Redis credentials"))?;
        }
        Ok(url.into())
    }
}

fn write_atomic(path: &Path, source: &[u8]) -> anyhow::Result<()> {
    use std::{
        fs::OpenOptions,
        io::Write,
    };
    let temporary = path.with_extension(format!("board.{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .context("failed to create board configuration")?;
        file.write_all(source)
            .context("failed to write board configuration")?;
        file.sync_all()
            .context("failed to sync board configuration")?;
        drop(file);
        fs::rename(&temporary, path).context("failed to activate board configuration")
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

impl Listeners {
    pub async fn bind(config: &ListenConfig) -> anyhow::Result<Self> {
        Ok(Self {
            admin: TcpListener::bind(&config.admin)
                .await
                .context("failed to bind admin listener")?,
            user: TcpListener::bind(&config.user)
                .await
                .context("failed to bind user listener")?,
        })
    }
}
