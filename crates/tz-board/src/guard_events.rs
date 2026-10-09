use sqlx::PgPool;
use tracing::info;
use uuid::Uuid;

const COALESCE_WINDOW_SECS: i64 = 60;

/// 视为「隧道遭攻击」的防护规则。
/// `per_ip_limit` 不在其中：它是普通每 IP 新建连接限速，拒绝超量连接本身就是防护生效，
/// 不代表隧道被攻击（浏览器重试风暴也会触发，曾导致启用后秒被自动打停）。
const ATTACK_RULES: &[&str] = &[
    "udp_amplify",
    "per_tunnel_ip_limit",
];

/// 自动暂停的判定窗口与阈值：需在窗口内持续命中（guard_events 每 60s 合并一行），
/// 瞬时尖峰（浏览器残留重试等）不足以触发，避免“刚启用即再次被打停”。
const ATTACK_PAUSE_WINDOW_SECS: i64 = 600;
const ATTACK_PAUSE_MIN_HITS: i64 = 3;
/// 手动启用/编辑后的保护期（秒）：期间不自动暂停，给残留重试自然衰减的时间。
const ATTACK_MANUAL_GRACE_SECS: i64 = 120;

/// 写入防护状态事件：仅类型 + 强度；持续时间由 first_seen/last_seen 推导。
/// 不保存 peer / detail 等业务原始数据。
pub async fn ingest(
    pg: &PgPool,
    node_id: Uuid,
    rule: &str,
    tunnel_id: Option<Uuid>,
    intensity: u32,
) {
    let rule = rule.trim();
    if rule.is_empty() || rule.len() > 128 {
        return;
    }
    let intensity = intensity.max(1) as i32;

    // 同一节点 / 规则 / 隧道，60 秒内合并：强度取窗口最大值，延长 last_seen。
    let updated = sqlx::query(
        "UPDATE guard_events
         SET hit_count = GREATEST(hit_count, $4),
             detail = '',
             peer = NULL,
             last_seen_at = now()
         WHERE id = (
           SELECT id FROM guard_events
           WHERE node_id = $1
             AND rule = $2
             AND tunnel_id IS NOT DISTINCT FROM $3
             AND last_seen_at > now() - make_interval(secs => $5)
           ORDER BY last_seen_at DESC
           LIMIT 1
         )",
    )
    .bind(node_id)
    .bind(rule)
    .bind(tunnel_id)
    .bind(intensity)
    .bind(COALESCE_WINDOW_SECS)
    .execute(pg)
    .await;

    if !updated.map(|result| result.rows_affected() > 0).unwrap_or(false) {
        let _ = sqlx::query(
            "INSERT INTO guard_events (node_id, rule, peer, tunnel_id, detail, hit_count)
             VALUES ($1, $2, NULL, $3, '', $4)",
        )
        .bind(node_id)
        .bind(rule)
        .bind(tunnel_id)
        .bind(intensity)
        .execute(pg)
        .await;
    }

    maybe_pause_tunnel_on_attack(pg, node_id, rule, tunnel_id).await;
}

async fn maybe_pause_tunnel_on_attack(
    pg: &PgPool,
    node_id: Uuid,
    rule: &str,
    tunnel_id: Option<Uuid>,
) {
    let Some(tunnel_id) = tunnel_id else {
        return;
    };
    if !ATTACK_RULES.iter().any(|name| *name == rule) {
        return;
    }

    let node_policy: serde_json::Value =
        match sqlx::query_scalar("SELECT COALESCE(guard_policy, '{}'::jsonb) FROM nodes WHERE id = $1")
            .bind(node_id)
            .fetch_optional(pg)
            .await
        {
            Ok(Some(value)) => value,
            _ => return,
        };
    let effective = crate::guard_policy::effective_node_guard_policy(pg, &node_policy).await;
    let Some(action) = crate::guard_policy::on_attack_action(&effective) else {
        return;
    };

    let reason = format!("guard: auto-paused on {rule}");
    let reason = reason.chars().take(480).collect::<String>();

    let attack_rules = ATTACK_RULES.to_vec();
    // pause_minutes=0 → guard_pause_until NULL，需手动恢复；>0 → 到期由 scheduler 恢复。
    // reenable_cooldown_minutes>0 → 冷却期内用户不能手动再次开启（防止攻击未停时反复开启）。
    // 仅当窗口内持续命中达到阈值、且隧道并非刚被手动启用/编辑时才暂停。
    let row: Option<(Uuid, Uuid, Uuid)> = sqlx::query_as(
        "UPDATE tunnels
         SET status = 'suspended',
             enabled = FALSE,
             last_error = $2,
             guard_pause_until = CASE WHEN $3::int > 0
                 THEN now() + make_interval(mins => $3::int)
                 ELSE NULL END,
             guard_cooldown_until = CASE WHEN $9::int > 0
                 THEN now() + make_interval(mins => $9::int)
                 ELSE guard_cooldown_until END,
             revision = revision + 1,
             updated_at = now()
         WHERE id = $1
           AND node_id = $4
           AND enabled = TRUE
           AND status NOT IN ('deleted', 'suspended', 'pending_review')
           AND updated_at <= now() - make_interval(secs => $5::int)
           AND (
             SELECT COUNT(*) FROM guard_events ge
             WHERE ge.tunnel_id = tunnels.id
               AND ge.rule = ANY($6)
               AND ge.last_seen_at > now() - make_interval(secs => $7::int)
           ) >= $8::bigint
         RETURNING id, node_id, client_id",
    )
    .bind(tunnel_id)
    .bind(&reason)
    .bind(action.pause_minutes as i32)
    .bind(node_id)
    .bind(ATTACK_MANUAL_GRACE_SECS as i32)
    .bind(&attack_rules)
    .bind(ATTACK_PAUSE_WINDOW_SECS)
    .bind(ATTACK_PAUSE_MIN_HITS)
    .bind(action.reenable_cooldown_minutes as i32)
    .fetch_optional(pg)
    .await
    .ok()
    .flatten();

    let Some((tunnel_id, node_id, client_id)) = row else {
        return;
    };
    info!(
        %tunnel_id,
        %node_id,
        %rule,
        pause_minutes = action.pause_minutes,
        "tunnel auto-paused by on_attack policy"
    );
    crate::ws::push_tunnel_suspend(pg, node_id, client_id, tunnel_id).await;
}

/// scheduler：到期自动恢复因防护暂停的隧道。
pub async fn resume_expired_guard_pauses(pg: &PgPool) {
    let rows: Vec<(Uuid, Uuid, Uuid)> = match sqlx::query_as(
        "UPDATE tunnels
         SET status = 'provisioning',
             enabled = TRUE,
             last_error = NULL,
             guard_pause_until = NULL,
             guard_cooldown_until = NULL,
             revision = revision + 1,
             updated_at = now()
         WHERE status = 'suspended'
           AND guard_pause_until IS NOT NULL
           AND guard_pause_until <= now()
         RETURNING id, node_id, client_id",
    )
    .fetch_all(pg)
    .await
    {
        Ok(rows) => rows,
        Err(_) => return,
    };
    for (tunnel_id, node_id, client_id) in rows {
        info!(%tunnel_id, %node_id, "tunnel auto-resumed after guard pause");
        crate::ws::sync_tunnel_online(pg, node_id, client_id, tunnel_id).await;
    }
}
