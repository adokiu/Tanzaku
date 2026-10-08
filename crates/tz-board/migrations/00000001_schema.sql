CREATE TABLE system_settings (
    key TEXT PRIMARY KEY,
    value JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE users (
    id UUID PRIMARY KEY,
    email TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('admin', 'user')),
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'disabled')),
    must_change_password BOOLEAN NOT NULL DEFAULT FALSE,
    last_login_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE pki_ca (
    id SMALLINT PRIMARY KEY CHECK (id = 1),
    certificate_pem TEXT NOT NULL,
    private_key_pem TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE redis_outbox (
    id BIGSERIAL PRIMARY KEY,
    operation TEXT NOT NULL,
    payload JSONB NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX redis_outbox_pending ON redis_outbox (next_attempt_at, id);

CREATE TABLE audit_logs (
    id BIGSERIAL PRIMARY KEY,
    actor_id UUID REFERENCES users (id) ON DELETE SET NULL,
    action TEXT NOT NULL,
    target_type TEXT NOT NULL,
    target_id TEXT,
    details JSONB NOT NULL DEFAULT '{}'::jsonb,
    remote_ip INET,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX audit_logs_created_at ON audit_logs (created_at DESC);

CREATE TABLE node_groups (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    description TEXT NOT NULL DEFAULT '',
    domain_suffixes TEXT[] NOT NULL DEFAULT '{}',
    wildcard_certificate_id UUID,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE nodes (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    region TEXT NOT NULL DEFAULT '',
    public_host TEXT NOT NULL,
    bind_addr TEXT NOT NULL DEFAULT '0.0.0.0',
    token_hash BYTEA NOT NULL,
    token_prefix TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    online BOOLEAN NOT NULL DEFAULT FALSE,
    version TEXT,
    os TEXT,
    capabilities JSONB NOT NULL DEFAULT '{}'::jsonb,
    carrier_ports JSONB NOT NULL DEFAULT '{"tcp":{"port":7000,"enabled":true},"quic":{"port":7000,"enabled":true}}'::jsonb,
    tcp_port_ranges JSONB NOT NULL DEFAULT '[[20000,29999]]'::jsonb,
    udp_port_ranges JSONB NOT NULL DEFAULT '[[20000,29999]]'::jsonb,
    port_exclude JSONB NOT NULL DEFAULT '[]'::jsonb,
    http_shared_port INTEGER NOT NULL DEFAULT 80 CHECK (http_shared_port BETWEEN 1 AND 65535),
    https_shared_port INTEGER NOT NULL DEFAULT 443 CHECK (https_shared_port BETWEEN 1 AND 65535),
    tcp_congestion TEXT NOT NULL DEFAULT 'bbr' CHECK (tcp_congestion IN ('bbr', 'cubic', 'system')),
    quic_congestion TEXT NOT NULL DEFAULT 'cubic' CHECK (quic_congestion IN ('bbr', 'cubic', 'newreno')),
    tfo_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    tfo_queue_length INTEGER NOT NULL DEFAULT 256 CHECK (tfo_queue_length BETWEEN 1 AND 65535),
    backlog INTEGER NOT NULL DEFAULT 1024 CHECK (backlog BETWEEN 1 AND 65535),
    memory_budget_bytes BIGINT NOT NULL DEFAULT 1073741824 CHECK (memory_budget_bytes > 0),
    guard_policy JSONB NOT NULL DEFAULT '{}'::jsonb,
    trusted_proxies CIDR[] NOT NULL DEFAULT '{}',
    config_revision BIGINT NOT NULL DEFAULT 0,
    stats_cursor BIGINT NOT NULL DEFAULT 0,
    certificate_fingerprint TEXT,
    certificate_expires_at TIMESTAMPTZ,
    last_seen_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE node_group_members (
    node_group_id UUID NOT NULL REFERENCES node_groups(id) ON DELETE CASCADE,
    node_id UUID NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (node_group_id, node_id)
);

CREATE TABLE plans (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    description TEXT NOT NULL DEFAULT '',
    speed_limit_mbps BIGINT NOT NULL CHECK (speed_limit_mbps > 0),
    max_conns_per_tunnel INTEGER NOT NULL CHECK (max_conns_per_tunnel > 0),
    max_new_conns_per_sec INTEGER NOT NULL CHECK (max_new_conns_per_sec > 0),
    max_tunnels INTEGER NOT NULL CHECK (max_tunnels > 0),
    allow_custom_port BOOLEAN NOT NULL DEFAULT FALSE,
    traffic_quota_bytes BIGINT CHECK (traffic_quota_bytes > 0),
    traffic_period TEXT NOT NULL CHECK (traffic_period IN ('day', 'week', 'month', 'quarter', 'year', 'lifetime')),
    duration_days INTEGER CHECK (duration_days > 0),
    allowed_protocols TEXT[] NOT NULL DEFAULT ARRAY['tcp', 'udp', 'http']::text[],
    traffic_count_mode TEXT NOT NULL DEFAULT 'sum' CHECK (traffic_count_mode IN ('sum', 'inbound', 'outbound', 'max')),
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE plan_node_groups (
    plan_id UUID NOT NULL REFERENCES plans(id) ON DELETE CASCADE,
    node_group_id UUID NOT NULL REFERENCES node_groups(id) ON DELETE RESTRICT,
    PRIMARY KEY (plan_id, node_group_id)
);

CREATE TABLE user_subscriptions (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    plan_id UUID REFERENCES plans(id) ON DELETE SET NULL,
    status TEXT NOT NULL CHECK (status IN ('active', 'expired', 'cancelled', 'suspended')),
    speed_limit_mbps BIGINT NOT NULL CHECK (speed_limit_mbps > 0),
    max_conns_per_tunnel INTEGER NOT NULL CHECK (max_conns_per_tunnel > 0),
    max_new_conns_per_sec INTEGER NOT NULL CHECK (max_new_conns_per_sec > 0),
    max_tunnels INTEGER NOT NULL CHECK (max_tunnels > 0),
    allow_custom_port BOOLEAN NOT NULL DEFAULT FALSE,
    traffic_quota_bytes BIGINT CHECK (traffic_quota_bytes > 0),
    traffic_period TEXT NOT NULL CHECK (traffic_period IN ('day', 'week', 'month', 'quarter', 'year', 'lifetime')),
    duration_days INTEGER CHECK (duration_days > 0),
    allowed_protocols TEXT[] NOT NULL,
    traffic_count_mode TEXT NOT NULL DEFAULT 'sum' CHECK (traffic_count_mode IN ('sum', 'inbound', 'outbound', 'max')),
    starts_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ,
    period_anchor SMALLINT NOT NULL CHECK (period_anchor BETWEEN 1 AND 31),
    exhausted_period_start TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX user_subscriptions_one_active_per_user
    ON user_subscriptions (user_id) WHERE status = 'active';

CREATE TABLE subscription_node_groups (
    subscription_id UUID NOT NULL REFERENCES user_subscriptions(id) ON DELETE CASCADE,
    node_group_id UUID NOT NULL REFERENCES node_groups(id) ON DELETE RESTRICT,
    PRIMARY KEY (subscription_id, node_group_id)
);

CREATE TABLE clients (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    token_hash BYTEA NOT NULL,
    token_prefix TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    capabilities JSONB NOT NULL DEFAULT '{}'::jsonb,
    certificate_fingerprint TEXT,
    certificate_expires_at TIMESTAMPTZ,
    carrier_public_key BYTEA,
    version TEXT,
    os TEXT,
    online BOOLEAN NOT NULL DEFAULT FALSE,
    last_seen_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, name)
);

CREATE TABLE tunnels (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    client_id UUID NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    node_id UUID NOT NULL REFERENCES nodes(id) ON DELETE RESTRICT,
    name TEXT NOT NULL,
    carrier TEXT NOT NULL CHECK (carrier IN ('tcp', 'quic')),
    protocol TEXT NOT NULL CHECK (protocol IN ('tcp', 'udp', 'http')),
    remote_port INTEGER CHECK (remote_port BETWEEN 1 AND 65535),
    port_custom BOOLEAN NOT NULL DEFAULT FALSE,
    target_host TEXT,
    target_port INTEGER CHECK (target_port BETWEEN 1 AND 65535),
    target_url TEXT,
    http_access TEXT CHECK (http_access IN ('shared', 'dedicated')),
    https_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    cert_id UUID,
    host_rewrite TEXT NOT NULL DEFAULT '$http_host',
    backend_tls_insecure BOOLEAN NOT NULL DEFAULT FALSE,
    http_options JSONB NOT NULL DEFAULT '{}'::jsonb,
    speed_limit_mbps BIGINT NOT NULL CHECK (speed_limit_mbps > 0),
    max_conns INTEGER NOT NULL CHECK (max_conns > 0),
    max_new_conns_per_sec INTEGER NOT NULL CHECK (max_new_conns_per_sec > 0),
    node_select_mode TEXT NOT NULL DEFAULT 'manual' CHECK (node_select_mode = 'manual'),
    status TEXT NOT NULL DEFAULT 'provisioning' CHECK (status IN ('pending_review', 'provisioning', 'ready', 'active', 'client_offline', 'node_offline', 'suspended', 'error', 'deleted')),
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    revision BIGINT NOT NULL DEFAULT 0,
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, name),
    CHECK ((protocol = 'http' AND target_url IS NOT NULL) OR (protocol <> 'http' AND target_host IS NOT NULL AND target_port IS NOT NULL)),
    CHECK ((protocol = 'http' AND http_access IS NOT NULL) OR (protocol <> 'http' AND http_access IS NULL)),
    CHECK ((http_access = 'shared' AND remote_port IS NULL) OR (http_access = 'dedicated' AND remote_port IS NOT NULL) OR protocol <> 'http')
);

CREATE UNIQUE INDEX tunnels_remote_port_unique
    ON tunnels (node_id, (CASE WHEN protocol = 'udp' THEN 'udp' ELSE 'tcp' END), remote_port)
    WHERE remote_port IS NOT NULL AND status <> 'deleted';

CREATE INDEX tunnels_user_idx ON tunnels (user_id, created_at DESC) WHERE status <> 'deleted';
CREATE INDEX tunnels_node_idx ON tunnels (node_id, status) WHERE status <> 'deleted';

CREATE TABLE tunnel_domains (
    id UUID PRIMARY KEY,
    tunnel_id UUID NOT NULL REFERENCES tunnels(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    domain TEXT NOT NULL UNIQUE,
    status TEXT NOT NULL CHECK (status IN ('pending_review', 'approved', 'rejected', 'cooldown')),
    reviewed_by UUID REFERENCES users(id) ON DELETE SET NULL,
    reviewed_at TIMESTAMPTZ,
    cooldown_until TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE certificates (
    id UUID PRIMARY KEY,
    owner_user_id UUID REFERENCES users(id) ON DELETE CASCADE,
    domains TEXT[] NOT NULL,
    cert_pem TEXT NOT NULL,
    private_key_pem TEXT NOT NULL,
    not_before TIMESTAMPTZ NOT NULL,
    not_after TIMESTAMPTZ NOT NULL,
    source TEXT NOT NULL DEFAULT 'manual' CHECK (source IN ('manual', 'acme')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

ALTER TABLE node_groups ADD CONSTRAINT node_groups_wildcard_certificate_fk FOREIGN KEY (wildcard_certificate_id) REFERENCES certificates(id) ON DELETE SET NULL;

ALTER TABLE tunnels ADD CONSTRAINT tunnels_certificate_fk FOREIGN KEY (cert_id) REFERENCES certificates(id) ON DELETE SET NULL;

CREATE TABLE traffic_usage (
    subscription_id UUID NOT NULL REFERENCES user_subscriptions(id) ON DELETE CASCADE,
    period_start TIMESTAMPTZ NOT NULL,
    bytes_in BIGINT NOT NULL DEFAULT 0 CHECK (bytes_in >= 0),
    bytes_out BIGINT NOT NULL DEFAULT 0 CHECK (bytes_out >= 0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (subscription_id, period_start)
);

CREATE TABLE tunnel_traffic_hourly (
    tunnel_id UUID NOT NULL REFERENCES tunnels(id) ON DELETE CASCADE,
    hour_start TIMESTAMPTZ NOT NULL,
    bytes_in BIGINT NOT NULL DEFAULT 0,
    bytes_out BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (tunnel_id, hour_start)
) PARTITION BY RANGE (hour_start);

CREATE TABLE tunnel_traffic_hourly_default
    PARTITION OF tunnel_traffic_hourly
    FOR VALUES FROM ('2020-01-01 00:00:00+00') TO ('2100-01-01 00:00:00+00');

CREATE TABLE node_stats_cursors (
    node_id UUID PRIMARY KEY REFERENCES nodes(id) ON DELETE CASCADE,
    committed_seq BIGINT NOT NULL DEFAULT 0,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE ip_bans (
    node_id UUID NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    ip_prefix CIDR NOT NULL,
    reason TEXT NOT NULL,
    source TEXT NOT NULL CHECK (source IN ('auto', 'manual')),
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (node_id, ip_prefix)
);

CREATE INDEX ip_bans_expiry ON ip_bans (expires_at);
