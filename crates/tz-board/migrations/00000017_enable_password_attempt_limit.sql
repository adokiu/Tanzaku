-- 开启账号级密码尝试限制（此前仅有设置项未接入登录）。
INSERT INTO system_settings (key, value)
VALUES ('security_password_attempt_limit_enabled', 'true'::jsonb)
ON CONFLICT (key) DO UPDATE
SET value = 'true'::jsonb, updated_at = now()
WHERE system_settings.value = 'false'::jsonb;
