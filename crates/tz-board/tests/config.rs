use tz_board::config::{BoardConfig, PgConfig, RedisConfig};

#[test]
fn default_listener_ports_are_separated() {
    let config = BoardConfig::default();
    assert_eq!(config.listen.admin, "0.0.0.0:9000");
    assert_eq!(config.listen.user, "0.0.0.0:9001");
    assert!(config.postgres.is_none());
    assert!(config.redis.is_none());
    assert!(config.trusted_proxies.iter().any(|cidr| cidr == "127.0.0.0/8"));
}

#[test]
fn redis_connection_url_encodes_credentials() {
    let config = RedisConfig {
        host: "127.0.0.1".into(),
        port: 6379,
        database: 2,
        password: Some("p@ss:/?#word".into()),
        tls: true,
    };
    let url = config.connection_url().unwrap();
    let parsed = url::Url::parse(&url).unwrap();
    assert_eq!(parsed.scheme(), "rediss");
    // `Url::password` 返回的是百分号编码后的原文。
    assert_eq!(parsed.password(), Some("p%40ss%3A%2F%3F%23word"));
    assert_eq!(parsed.path(), "/2");
    assert!(!url.contains("p@ss:/?#word"));
}

#[test]
fn postgres_configuration_validates_ssl_modes_and_bounds() {
    let config = PgConfig {
        host: "localhost".into(),
        port: 5432,
        database: "tanzaku".into(),
        user: "board".into(),
        password: "secret-not-logged".into(),
        sslmode: "verify-full".into(),
        max_connections: 32,
    };
    assert!(config.validate().is_ok());
    let invalid = PgConfig { port: 0, ..config };
    assert!(invalid.validate().is_err());
}

#[test]
fn persist_database_config_never_writes_listen() {
    use std::io::Write;
    let dir = std::env::temp_dir().join(format!("tanzaku-config-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("board.toml");
    std::fs::File::create(&path)
        .unwrap()
        .write_all(b"[listen]\nadmin = \"127.0.0.1:9000\"\n")
        .unwrap();
    let postgres = PgConfig {
        host: "db".into(),
        port: 5432,
        database: "tanzaku".into(),
        user: "tanzaku".into(),
        password: "secret".into(),
        sslmode: "disable".into(),
        max_connections: 8,
    };
    let redis = RedisConfig {
        host: "redis".into(),
        port: 6379,
        database: 0,
        password: None,
        tls: false,
    };
    BoardConfig::persist_database_config(&path, &postgres, &redis, true).unwrap();
    let source = std::fs::read_to_string(&path).unwrap();
    assert!(source.contains("[postgres]"));
    assert!(source.contains("[redis]"));
    assert!(source.contains("[listen]"));
    assert!(source.contains("127.0.0.1:9000"));
    let _ = std::fs::remove_dir_all(dir);
}
