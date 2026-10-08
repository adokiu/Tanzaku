-- 防护自动暂停：可选到期自动恢复时间（NULL = 需手动恢复）
ALTER TABLE tunnels
    ADD COLUMN IF NOT EXISTS guard_pause_until TIMESTAMPTZ;

CREATE INDEX IF NOT EXISTS tunnels_guard_pause_until_idx
    ON tunnels (guard_pause_until)
    WHERE guard_pause_until IS NOT NULL;
