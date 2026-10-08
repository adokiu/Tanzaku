use sqlx::PgPool;
use tracing::info;
use uuid::Uuid;

const COALESCE_WINDOW_SECS: i64 = 60;

/// 视为「隧道遭攻击」的防护规则（达到阈值后可自动暂停隧道）。
const ATTACK_RULES: &[&str] = &[
    "udp_amplify",
    "per_ip_limit",
    "per_tunnel_ip_limit",
];

pub async fn ingest(
    pg: &PgPool,
    node_id: Uuid,
    rule: &str,
    peer: Option<&str>,
    tunnel_id: Option<Uuid>,
    detail: &str,
    hit_count: u32,
) {
    let rule = rule.trim();
    if rule.is_empty() || rule.len() > 128 {
        return;
    }
    let detail = detail.chars().take(512).collect::<String>();
    let hits = hit_count.max(1) as i32;
    let peer_sql = peer
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 64);

    // 同一节点 / 规则 / 来源 / 隧道，60 秒内合并计数，避免刷屏。
    let updated = sqlx::query(
        "UPDATE guard_events
         SET hit_count = hit_count + $6,
             detail = CASE WHEN $5 = '' THEN detail ELSE $5 END,
             last_seen_at = now()
         WHERE id = (
           SELECT id FROM guard_events
           WHERE node_id = $1
             AND rule = $2
             AND peer IS NOT DISTINCT FROM $3::inet
             AND tunnel_id IS NOT DISTINCT FROM $4
             AND last_seen_at > now() - make_interval(secs => $7)
           ORDER BY last_seen_at DESC
           LIMIT 1
         )",
    )
    .bind(node_id)
    .bind(rule)
    .bind(peer_sql)
    .bind(tunnel_id)
    .bind(&detail)
    .bind(hits)
    .bind(COALESCE_WINDOW_SECS)
    .execute(pg)
    .await;

    if !updated.map(|result| result.rows_affected() > 0).unwrap_or(false) {
        let _ = sqlx::query(
            "INSERT INTO guard_events (node_id, rule, peer, tunnel_id, detail, hit_count)
             VALUES ($1, $2, $3::inet, $4, $5, $6)",
        )
        .bind(node_id)
        .bind(rule)
        .bind(peer_sql)
        .bind(tunnel_id)
        .bind(&detail)
        .bind(hits)
        .execute(pg)
        .await;
    }

    maybe_pause_tunnel_on_attack(pg, node_id, rule, tunnel_id, hit_count.max(1), &detail).await;
}

async fn maybe_pause_tunnel_on_attack(
    pg: &PgPool,
    node_id: Uuid,
    rule: &str,
    tunnel_id: Option<Uuid>,
    hit_count: u32,
    detail: &str,
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
    if hit_count < action.min_hits {
        return;
    }

    let reason = format!(
        "guard: auto-paused after {rule} (hits={hit_count}{})",
        if detail.is_empty() {
            String::new()
        } else {
            format!(", {detail}")
        }
    );
    let reason = reason.chars().take(480).collect::<String>();

    // pause_minutes=0 → guard_pause_until NULL，需手动恢复；>0 → 到期由 scheduler 恢复。
    let row: Option<(Uuid, Uuid, Uuid)> = if action.pause_minutes == 0 {
        sqlx::query_as(
            "UPDATE tunnels
             SET status = 'suspended',
                 enabled = FALSE,
                 last_error = $2,
                 guard_pause_until = NULL,
                 revision = revision + 1,
                 updated_at = now()
             WHERE id = $1
               AND node_id = $3
               AND enabled = TRUE
               AND status NOT IN ('deleted', 'suspended', 'pending_review')
             RETURNING id, node_id, client_id",
        )
        .bind(tunnel_id)
        .bind(&reason)
        .bind(node_id)
        .fetch_optional(pg)
        .await
        .ok()
        .flatten()
    } else {
        sqlx::query_as(
            "UPDATE tunnels
             SET status = 'suspended',
                 enabled = FALSE,
                 last_error = $2,
                 guard_pause_until = now() + make_interval(mins => $3::int),
                 revision = revision + 1,
                 updated_at = now()
             WHERE id = $1
               AND node_id = $4
               AND enabled = TRUE
               AND status NOT IN ('deleted', 'suspended', 'pending_review')
             RETURNING id, node_id, client_id",
        )
        .bind(tunnel_id)
        .bind(&reason)
        .bind(action.pause_minutes as i32)
        .bind(node_id)
        .fetch_optional(pg)
        .await
        .ok()
        .flatten()
    };

    let Some((tunnel_id, node_id, client_id)) = row else {
        return;
    };
    info!(
        %tunnel_id,
        %node_id,
        %rule,
        hit_count,
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
