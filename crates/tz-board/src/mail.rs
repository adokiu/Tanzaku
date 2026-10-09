use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{Mailbox, header::ContentType},
    transport::smtp::{
        authentication::Credentials,
        client::{Tls, TlsParameters},
    },
};
use serde_json::Value;
use sqlx::PgPool;

#[derive(Debug, Clone)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub encryption: String,
    pub username: String,
    pub password: String,
    pub from_address: String,
}

async fn setting_value(pg: &PgPool, key: &str) -> Option<Value> {
    sqlx::query_scalar("SELECT value FROM system_settings WHERE key = $1")
        .bind(key)
        .fetch_optional(pg)
        .await
        .ok()
        .flatten()
}

fn value_string(value: Option<&Value>) -> String {
    value
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}

fn value_u16(value: Option<&Value>, default: u16) -> u16 {
    value
        .and_then(|v| v.as_u64())
        .and_then(|n| u16::try_from(n).ok())
        .unwrap_or(default)
}

pub async fn load_smtp_config(pg: &PgPool) -> anyhow::Result<SmtpConfig> {
    let keys = [
        "mail_smtp_host",
        "mail_smtp_port",
        "mail_smtp_encryption",
        "mail_smtp_username",
        "mail_smtp_password",
        "mail_from_address",
    ];
    let mut map = std::collections::HashMap::new();
    for key in keys {
        if let Some(value) = setting_value(pg, key).await {
            map.insert(key, value);
        }
    }
    let host = value_string(map.get("mail_smtp_host"));
    let from_address = value_string(map.get("mail_from_address"));
    let username = value_string(map.get("mail_smtp_username"));
    let password = value_string(map.get("mail_smtp_password"));
    if host.is_empty() || from_address.is_empty() {
        anyhow::bail!("请先配置 SMTP 主机与发件人地址");
    }
    if username.is_empty() || password.is_empty() {
        anyhow::bail!("请先配置 SMTP 用户名与密码");
    }
    Ok(SmtpConfig {
        host,
        port: value_u16(map.get("mail_smtp_port"), 465),
        encryption: value_string(map.get("mail_smtp_encryption")).to_ascii_lowercase(),
        username,
        password,
        from_address,
    })
}

pub async fn send_test_mail(pg: &PgPool, to: &str) -> anyhow::Result<()> {
    let to = to.trim();
    if to.is_empty() {
        anyhow::bail!("请填写收件人邮箱");
    }
    let cfg = load_smtp_config(pg).await?;
    let from: Mailbox = cfg
        .from_address
        .parse()
        .map_err(|_| anyhow::anyhow!("发件人地址无效"))?;
    let recipient: Mailbox = to
        .parse()
        .map_err(|_| anyhow::anyhow!("收件人地址无效"))?;
    let email = Message::builder()
        .from(from)
        .to(recipient)
        .subject("Tanzaku 测试邮件")
        .header(ContentType::TEXT_PLAIN)
        .body(String::from(
            "这是一封来自 Tanzaku 管理端的 SMTP 测试邮件。若你收到此邮件，说明邮件配置可用。",
        ))
        .map_err(|error| anyhow::anyhow!("构造邮件失败: {error}"))?;

    let credentials = Credentials::new(cfg.username.clone(), cfg.password.clone());
    let mailer = match cfg.encryption.as_str() {
        "none" => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&cfg.host)
            .port(cfg.port)
            .credentials(credentials)
            .build(),
        "starttls" | "tls" => {
            let tls = TlsParameters::new(cfg.host.clone())
                .map_err(|error| anyhow::anyhow!("TLS 参数无效: {error}"))?;
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&cfg.host)
                .map_err(|error| anyhow::anyhow!("SMTP 连接失败: {error}"))?
                .port(cfg.port)
                .tls(Tls::Required(tls))
                .credentials(credentials)
                .build()
        }
        _ => {
            let tls = TlsParameters::new(cfg.host.clone())
                .map_err(|error| anyhow::anyhow!("TLS 参数无效: {error}"))?;
            AsyncSmtpTransport::<Tokio1Executor>::relay(&cfg.host)
                .map_err(|error| anyhow::anyhow!("SMTP 连接失败: {error}"))?
                .port(cfg.port)
                .tls(Tls::Wrapper(tls))
                .credentials(credentials)
                .build()
        }
    };

    mailer
        .send(email)
        .await
        .map_err(|error| anyhow::anyhow!("发送失败: {error}"))?;
    Ok(())
}
