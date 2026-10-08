-- 去掉「就绪 / client_offline」展示态：绑定成功后即视为运行中。
UPDATE tunnels
SET status = 'active', updated_at = now()
WHERE status IN ('ready', 'client_offline');

UPDATE tunnels
SET status = 'error', updated_at = now()
WHERE status = 'failed';
