INSERT INTO system_settings (key, value)
VALUES ('client_ip_geo_provider', '"ipinfo"'::jsonb)
ON CONFLICT (key) DO NOTHING;
