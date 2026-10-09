-- 异常隧道自动重建：完全关闭后重新下发；重建后 30 秒内再次异常计为失败，连续 3 次失败停止重试。
ALTER TABLE tunnels ADD COLUMN IF NOT EXISTS auto_recover_attempts INT NOT NULL DEFAULT 0;
ALTER TABLE tunnels ADD COLUMN IF NOT EXISTS auto_recover_at TIMESTAMPTZ;
ALTER TABLE tunnels ADD COLUMN IF NOT EXISTS auto_recover_closing BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE tunnels ADD COLUMN IF NOT EXISTS active_since TIMESTAMPTZ;
ALTER TABLE tunnels ADD COLUMN IF NOT EXISTS last_active_secs DOUBLE PRECISION;

-- 状态写入点分散（node/client 上报、API、调度），统一由触发器记录进入/离开运行中的时间。
CREATE OR REPLACE FUNCTION tunnels_track_active_period() RETURNS trigger AS $$
BEGIN
    IF NEW.status = 'active' AND OLD.status IS DISTINCT FROM 'active' THEN
        NEW.active_since := now();
    ELSIF NEW.status IS DISTINCT FROM 'active' AND OLD.status = 'active' THEN
        NEW.last_active_secs := EXTRACT(EPOCH FROM now() - COALESCE(OLD.active_since, now()));
        NEW.active_since := NULL;
    END IF;
    RETURN NEW;
END
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS tunnels_track_active_period ON tunnels;
CREATE TRIGGER tunnels_track_active_period
    BEFORE UPDATE OF status ON tunnels
    FOR EACH ROW EXECUTE FUNCTION tunnels_track_active_period();

UPDATE tunnels SET active_since = now() WHERE status = 'active' AND active_since IS NULL;

CREATE INDEX IF NOT EXISTS tunnels_auto_recover_idx ON tunnels (status) WHERE status = 'error' OR auto_recover_closing;
