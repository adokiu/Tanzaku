CREATE TABLE orders (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    plan_id UUID REFERENCES plans(id) ON DELETE SET NULL,
    plan_name TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('new', 'upgrade')),
    period TEXT NOT NULL CHECK (period IN ('day', 'week', 'month', 'quarter', 'year', 'lifetime')),
    amount_cents BIGINT NOT NULL DEFAULT 0 CHECK (amount_cents >= 0),
    status TEXT NOT NULL CHECK (status IN ('completed', 'cancelled', 'credited')),
    subscription_id UUID REFERENCES user_subscriptions(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX orders_created_idx ON orders (created_at DESC);
CREATE INDEX orders_user_idx ON orders (user_id, created_at DESC);
