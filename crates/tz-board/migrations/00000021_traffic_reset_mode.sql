INSERT INTO system_settings (key, value)
VALUES ('traffic_reset_mode', '"month_purchase"'::jsonb)
ON CONFLICT (key) DO NOTHING;

ALTER TABLE plans DROP CONSTRAINT IF EXISTS plans_traffic_period_check;
UPDATE plans SET traffic_period = CASE traffic_period
    WHEN 'year' THEN 'year_first'
    WHEN 'lifetime' THEN 'never'
    WHEN 'day' THEN 'month_purchase'
    WHEN 'week' THEN 'month_purchase'
    WHEN 'quarter' THEN 'month_purchase'
    WHEN 'month' THEN 'month_purchase'
    ELSE traffic_period
END;
ALTER TABLE plans ADD CONSTRAINT plans_traffic_period_check
    CHECK (traffic_period IN ('system', 'month_first', 'month_purchase', 'never', 'year_first', 'year_purchase'));

ALTER TABLE user_subscriptions DROP CONSTRAINT IF EXISTS user_subscriptions_traffic_period_check;
UPDATE user_subscriptions SET traffic_period = CASE traffic_period
    WHEN 'year' THEN 'year_first'
    WHEN 'lifetime' THEN 'never'
    WHEN 'day' THEN 'month_purchase'
    WHEN 'week' THEN 'month_purchase'
    WHEN 'quarter' THEN 'month_purchase'
    WHEN 'month' THEN 'month_purchase'
    ELSE traffic_period
END;
ALTER TABLE user_subscriptions ADD CONSTRAINT user_subscriptions_traffic_period_check
    CHECK (traffic_period IN ('system', 'month_first', 'month_purchase', 'never', 'year_first', 'year_purchase'));
