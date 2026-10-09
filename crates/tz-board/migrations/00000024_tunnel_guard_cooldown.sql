-- 攻击自动暂停后的重新开启冷却：冷却期内用户不能再次开启隧道。
ALTER TABLE tunnels ADD COLUMN IF NOT EXISTS guard_cooldown_until TIMESTAMPTZ;

CREATE INDEX IF NOT EXISTS tunnels_guard_cooldown_idx ON tunnels (guard_cooldown_until) WHERE guard_cooldown_until IS NOT NULL;