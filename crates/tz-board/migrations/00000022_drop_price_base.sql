-- 基础价仅管理端前端用于按月倍率预填各周期，不再落库。
UPDATE plans
SET price_month_cents = COALESCE(price_month_cents, price_base_cents)
WHERE price_base_cents IS NOT NULL;

ALTER TABLE plans DROP COLUMN IF EXISTS price_base_cents;
