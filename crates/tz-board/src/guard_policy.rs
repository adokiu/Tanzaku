use serde_json::{Map, Value};
use sqlx::PgPool;

pub const GUARD_MODULES: &[&str] = &[
    "ip_acl",
    "per_ip_limit",
    "auto_ban",
    "udp_amplify",
    "http_guard",
    "tls_guard",
    "preauth_guard",
    "node_limits",
    "block_http_on_l4",
    "per_tunnel_ip_limit",
    "on_attack",
    "cn_residency",
    "cn_http_filing",
];

/// 节点覆盖元数据（非防护模块）：`{"overlay":{"enabled":true}}`。
pub const POLICY_META_KEYS: &[&str] = &["overlay"];

pub fn is_allowed_policy_key(key: &str) -> bool {
    GUARD_MODULES.contains(&key) || POLICY_META_KEYS.contains(&key)
}

/// 节点是否已配置独立覆盖（含任一防护模块）。
pub fn has_node_overlay(policy: &Value) -> bool {
    policy
        .as_object()
        .map(|object| object.keys().any(|key| GUARD_MODULES.contains(&key.as_str())))
        .unwrap_or(false)
}

/// 覆盖是否启用；缺省为启用。禁用时下发仅用全局策略，配置仍保留。
pub fn node_overlay_enabled(policy: &Value) -> bool {
    match policy
        .get("overlay")
        .and_then(|cfg| cfg.get("enabled"))
        .and_then(Value::as_bool)
    {
        Some(false) => false,
        _ => true,
    }
}

pub fn set_node_overlay_enabled(policy: &mut Value, enabled: bool) {
    let Some(object) = policy.as_object_mut() else {
        *policy = serde_json::json!({ "overlay": { "enabled": enabled } });
        return;
    };
    object.insert(
        "overlay".into(),
        serde_json::json!({ "enabled": enabled }),
    );
}

/// 合并时剔除 meta，避免把 overlay 写进生效策略。
pub fn strip_policy_meta(policy: &Value) -> Value {
    match policy {
        Value::Object(object) => {
            let mut out = Map::new();
            for (key, value) in object {
                if POLICY_META_KEYS.contains(&key.as_str()) {
                    continue;
                }
                out.insert(key.clone(), value.clone());
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

pub fn default_global_policy() -> Value {
    serde_json::json!({
        "per_ip_limit": {"enabled": true, "max_new": 64, "window_secs": 1},
        "udp_amplify": {
            "enabled": true,
            "max_new_flows": 128,
            "max_packets": 200000,
            "window_secs": 1
        },
        "node_limits": {"enabled": true, "max_tunnels": 0},
        "block_http_on_l4": {"enabled": false},
        "per_tunnel_ip_limit": {"enabled": false, "max_distinct_ips": 64, "window_secs": 300},
        "on_attack": {
            "enabled": false,
            "pause_minutes": 0,
            "min_hits": 32,
            "reenable_cooldown_minutes": 0
        },
        "cn_residency": {"enabled": false},
        "cn_http_filing": {"enabled": false},
        "http_guard": {"enabled": true, "max_header_bytes": 16384, "max_headers": 100},
        "auto_ban": {"enabled": true},
        "ip_acl": {"enabled": false, "allow": [], "deny": []},
        "tls_guard": {"enabled": true},
        "preauth_guard": {"enabled": true}
    })
}

/// 节点策略覆盖全局：对象 deep-merge，标量以节点为准；节点缺省字段继承全局。
pub fn merge_guard_policy(global: &Value, node: &Value) -> Value {
    match (global, node) {
        (Value::Object(base), Value::Object(over)) => {
            let mut out = Map::new();
            for (key, value) in base {
                out.insert(key.clone(), value.clone());
            }
            for (key, value) in over {
                let merged = match (out.get(key), value) {
                    (Some(existing), Value::Object(_)) => merge_guard_policy(existing, value),
                    _ => value.clone(),
                };
                out.insert(key.clone(), merged);
            }
            Value::Object(out)
        }
        (_, override_value) if !override_value.is_null() => override_value.clone(),
        (base, _) => base.clone(),
    }
}

pub async fn load_global_guard_policy(pg: &PgPool) -> Value {
    let value: Option<Value> =
        sqlx::query_scalar("SELECT policy FROM global_guard_policy WHERE id = 1")
            .fetch_optional(pg)
            .await
            .ok()
            .flatten();
    match value {
        // 与默认策略 deep-merge，保证新增模块（如 on_attack）对旧库也生效。
        Some(stored) if stored.is_object() => merge_guard_policy(&default_global_policy(), &stored),
        _ => default_global_policy(),
    }
}

pub async fn save_global_guard_policy<'e, E>(executor: E, policy: &Value) -> Result<(), sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    sqlx::query(
        "INSERT INTO global_guard_policy (id, policy, updated_at) VALUES (1, $1, now())
         ON CONFLICT (id) DO UPDATE SET policy = EXCLUDED.policy, updated_at = now()",
    )
    .bind(policy)
    .execute(executor)
    .await?;
    Ok(())
}

/// 隧道被攻击时的处置：`pause_minutes=0` 表示暂停后需手动恢复。开启即在攻击开始时暂停。
/// `reenable_cooldown_minutes>0` 时，自动暂停后进入冷却期，冷却结束前用户不能再次开启。
#[derive(Debug, Clone, Copy)]
pub struct OnAttackAction {
    pub pause_minutes: u32,
    pub reenable_cooldown_minutes: u32,
}

pub fn on_attack_action(policy: &Value) -> Option<OnAttackAction> {
    let cfg = policy.get("on_attack")?;
    if cfg.get("enabled").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let number = |key: &str| {
        cfg.get(key)
            .and_then(|value| value.as_u64().or_else(|| value.as_f64().map(|n| n as u64)))
            .unwrap_or(0)
            .min(u64::from(u32::MAX)) as u32
    };
    Some(OnAttackAction {
        pause_minutes: number("pause_minutes"),
        reenable_cooldown_minutes: number("reenable_cooldown_minutes"),
    })
}

pub fn cn_http_filing_enabled(policy: &Value) -> bool {
    policy
        .get("cn_http_filing")
        .and_then(|cfg| cfg.get("enabled"))
        .and_then(Value::as_bool)
        == Some(true)
}

pub fn cn_residency_enabled(policy: &Value) -> bool {
    policy
        .get("cn_residency")
        .and_then(|cfg| cfg.get("enabled"))
        .and_then(Value::as_bool)
        == Some(true)
}

/// 中国大陆节点不允许非 CN 地区 Client 接入（防跨境传输）。
pub fn cn_residency_blocks_client(policy: &Value, node_region: &str, client_region: &str) -> bool {
    if !cn_residency_enabled(policy) {
        return false;
    }
    if !node_region.eq_ignore_ascii_case("CN") {
        return false;
    }
    !client_region.eq_ignore_ascii_case("CN")
}

pub async fn effective_node_guard_policy(pg: &PgPool, node_policy: &Value) -> Value {
    let global = load_global_guard_policy(pg).await;
    if !node_overlay_enabled(node_policy) {
        return global;
    }
    merge_guard_policy(&global, &strip_policy_meta(node_policy))
}

pub fn max_tunnels_from_policy(policy: &Value) -> Option<u32> {
    let limits = policy.get("node_limits")?;
    if limits.get("enabled").and_then(Value::as_bool) == Some(false) {
        return None;
    }
    let max = limits.get("max_tunnels").and_then(Value::as_u64)? as u32;
    if max == 0 {
        None
    } else {
        Some(max)
    }
}

