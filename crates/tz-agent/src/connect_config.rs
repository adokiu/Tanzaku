use anyhow::{Context, bail};
use serde::Deserialize;
use std::{
    env,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

/// server / client 共用：连接 board 的配置。
#[derive(Debug, Clone)]
pub struct ConnectConfig {
    pub board: String,
    pub token: String,
}

#[derive(Debug, Default, Deserialize)]
struct ConnectFile {
    #[serde(default)]
    board: Option<String>,
    #[serde(default)]
    token: Option<String>,
}

/// 可执行文件同级目录 `{name}.toml`，否则 `./{name}.toml`。
pub fn default_config_path(name: &str) -> PathBuf {
    env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|dir| dir.join(format!("{name}.toml"))))
        .unwrap_or_else(|| PathBuf::from(format!("{name}.toml")))
}

/// 解析连接参数。
///
/// 配置文件路径：`-c` / `--config` > `TANZAKU_CONFIG` > 默认同级 `{name}.toml`。
/// 字段优先级（高者覆盖低者）：命令行 `--board`/`--token` > 环境变量
/// `TANZAKU_BOARD`/`TANZAKU_TOKEN` > 配置文件。
pub fn load_connect_config(name: &str) -> anyhow::Result<ConnectConfig> {
    let mut cli_board = None;
    let mut cli_token = None;
    let mut cli_config = None;
    let mut args = env::args_os().skip(1);
    while let Some(arg) = args.next() {
        let flag = arg.as_os_str();
        if flag == OsStr::new("-h") || flag == OsStr::new("--help") {
            bail!(
                "usage: tanzaku-{name} [--board <url>] [--token <token>] [-c|--config <path>]\n\
                 default config: {}",
                default_config_path(name).display()
            );
        }
        if flag == OsStr::new("-c") || flag == OsStr::new("--config") {
            let path = args
                .next()
                .context(format!("usage: tanzaku-{name} [-c|--config <path>]"))?;
            cli_config = Some(PathBuf::from(path));
            continue;
        }
        if let Some(path) = arg.to_str().and_then(|value| value.strip_prefix("-c")) {
            if !path.is_empty() {
                cli_config = Some(PathBuf::from(path));
                continue;
            }
        }
        if flag == OsStr::new("--board") {
            cli_board = Some(
                args.next()
                    .context(format!("usage: tanzaku-{name} --board <url>"))?
                    .into_string()
                    .map_err(|_| anyhow::anyhow!("invalid --board value"))?,
            );
            continue;
        }
        if flag == OsStr::new("--token") {
            cli_token = Some(
                args.next()
                    .context(format!("usage: tanzaku-{name} --token <token>"))?
                    .into_string()
                    .map_err(|_| anyhow::anyhow!("invalid --token value"))?,
            );
            continue;
        }
        bail!(
            "unknown argument: {}\nusage: tanzaku-{name} [--board <url>] [--token <token>] [-c|--config <path>]",
            arg.to_string_lossy()
        );
    }

    let config_path = if let Some(path) = cli_config {
        path
    } else if let Ok(path) = env::var("TANZAKU_CONFIG") {
        PathBuf::from(path)
    } else {
        default_config_path(name)
    };

    let file = load_file(&config_path)?;
    let board = cli_board
        .or_else(|| env::var("TANZAKU_BOARD").ok())
        .or(file.board)
        .filter(|value| !value.trim().is_empty())
        .with_context(|| {
            format!(
                "missing board URL: set in {} or pass --board / TANZAKU_BOARD",
                config_path.display()
            )
        })?;
    let token = cli_token
        .or_else(|| env::var("TANZAKU_TOKEN").ok())
        .or(file.token)
        .filter(|value| !value.trim().is_empty())
        .with_context(|| {
            format!(
                "missing token: set in {} or pass --token / TANZAKU_TOKEN",
                config_path.display()
            )
        })?;

    tracing::info!(path = %config_path.display(), "using {name} configuration");
    Ok(ConnectConfig { board, token })
}

fn load_file(path: &Path) -> anyhow::Result<ConnectFile> {
    if !path.exists() {
        return Ok(ConnectFile::default());
    }
    let source = fs::read_to_string(path)
        .with_context(|| format!("failed to read configuration {}", path.display()))?;
    toml::from_str(&source)
        .with_context(|| format!("failed to parse configuration {}", path.display()))
}
