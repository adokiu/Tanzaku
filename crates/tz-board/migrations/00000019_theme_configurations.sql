-- 每主题一份 managed 配置值（键来自 tanzaku-theme.json configuration.data）
CREATE TABLE IF NOT EXISTS theme_configurations (
    short TEXT PRIMARY KEY NOT NULL,
    data JSONB NOT NULL DEFAULT '{}'::jsonb,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
