-- 全局防护策略（与 nodes.guard_policy 同结构；下发时 deep-merge，节点覆盖全局）
INSERT INTO system_settings (key, value)
VALUES (
  'global_guard_policy',
  '{
    "per_ip_limit": {"enabled": true, "max_new_per_sec": 64},
    "udp_amplify": {"enabled": true, "max_new_flows_per_sec": 128, "max_packets_per_sec": 200000},
    "node_limits": {"enabled": true, "max_tunnels": 0},
    "block_http_on_l4": {"enabled": false},
    "per_tunnel_ip_limit": {"enabled": false, "max_distinct_ips": 64},
    "http_guard": {"enabled": true, "max_header_bytes": 16384, "max_headers": 100},
    "auto_ban": {"enabled": true},
    "ip_acl": {"enabled": false, "allow": [], "deny": []},
    "tls_guard": {"enabled": true},
    "preauth_guard": {"enabled": true}
  }'::jsonb
)
ON CONFLICT (key) DO NOTHING;

CREATE TABLE IF NOT EXISTS guard_events (
    id BIGSERIAL PRIMARY KEY,
    node_id UUID NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    rule TEXT NOT NULL,
    peer INET,
    tunnel_id UUID,
    detail TEXT NOT NULL DEFAULT '',
    hit_count INT NOT NULL DEFAULT 1,
    first_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS guard_events_node_last_idx
    ON guard_events (node_id, last_seen_at DESC);

CREATE INDEX IF NOT EXISTS guard_events_coalesce_idx
    ON guard_events (node_id, rule, peer, tunnel_id, last_seen_at DESC);
