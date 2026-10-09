-- 全局节点防护策略独立成表，不再占用 system_settings
CREATE TABLE IF NOT EXISTS global_guard_policy (
    id SMALLINT PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    policy JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO global_guard_policy (id, policy)
SELECT 1, value
FROM system_settings
WHERE key = 'global_guard_policy'
ON CONFLICT (id) DO NOTHING;

INSERT INTO global_guard_policy (id, policy)
VALUES (
  1,
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
ON CONFLICT (id) DO NOTHING;

DELETE FROM system_settings WHERE key = 'global_guard_policy';
