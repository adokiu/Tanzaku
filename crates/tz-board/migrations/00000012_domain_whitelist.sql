-- 域名过白。审核队列不是独立表，绑定仍在 tunnel_domains。
CREATE TABLE domain_whitelist (
    id UUID PRIMARY KEY,
    domain TEXT NOT NULL UNIQUE,
    note TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

UPDATE tunnel_domains
SET status = 'approved',
    reviewed_at = COALESCE(reviewed_at, now()),
    cooldown_until = NULL
WHERE status = 'pending_review';

UPDATE tunnels
SET status = 'provisioning',
    enabled = TRUE,
    last_error = NULL,
    revision = revision + 1,
    updated_at = now()
WHERE status = 'pending_review';

DELETE FROM system_settings WHERE key = 'domain_review_enabled';
