ALTER TABLE plans
    ADD COLUMN price_base_cents BIGINT CHECK (price_base_cents IS NULL OR price_base_cents >= 0),
    ADD COLUMN price_month_cents BIGINT CHECK (price_month_cents IS NULL OR price_month_cents >= 0),
    ADD COLUMN price_quarter_cents BIGINT CHECK (price_quarter_cents IS NULL OR price_quarter_cents >= 0),
    ADD COLUMN price_half_year_cents BIGINT CHECK (price_half_year_cents IS NULL OR price_half_year_cents >= 0),
    ADD COLUMN price_year_cents BIGINT CHECK (price_year_cents IS NULL OR price_year_cents >= 0),
    ADD COLUMN price_two_year_cents BIGINT CHECK (price_two_year_cents IS NULL OR price_two_year_cents >= 0),
    ADD COLUMN price_three_year_cents BIGINT CHECK (price_three_year_cents IS NULL OR price_three_year_cents >= 0),
    ADD COLUMN price_traffic_pack_cents BIGINT CHECK (price_traffic_pack_cents IS NULL OR price_traffic_pack_cents >= 0),
    ADD COLUMN price_reset_pack_cents BIGINT CHECK (price_reset_pack_cents IS NULL OR price_reset_pack_cents >= 0);

CREATE TABLE payment_channels (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    sort_order INTEGER NOT NULL DEFAULT 0,
    config JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX payment_channels_name_idx ON payment_channels (name);

INSERT INTO payment_channels (id, name, kind, enabled, sort_order, config)
VALUES ('00000000-0000-0000-0000-000000000001', '余额支付', 'balance', TRUE, 0, '{}'::jsonb);

ALTER TABLE orders DROP CONSTRAINT orders_kind_check;
ALTER TABLE orders ADD CONSTRAINT orders_kind_check
    CHECK (kind IN ('new', 'upgrade', 'addon', 'gift'));

ALTER TABLE orders DROP CONSTRAINT orders_period_check;
ALTER TABLE orders ADD CONSTRAINT orders_period_check
    CHECK (period IN (
        'day', 'week', 'month', 'quarter', 'half_year', 'year', 'two_year', 'three_year',
        'lifetime', 'traffic_pack', 'reset_pack'
    ));

ALTER TABLE orders DROP CONSTRAINT orders_status_check;
ALTER TABLE orders ADD CONSTRAINT orders_status_check
    CHECK (status IN ('pending', 'paying', 'completed', 'cancelled', 'failed', 'credited'));

ALTER TABLE orders
    ADD COLUMN payment_channel_id UUID REFERENCES payment_channels(id) ON DELETE SET NULL,
    ADD COLUMN paid_at TIMESTAMPTZ;
