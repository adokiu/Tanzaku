ALTER TABLE orders ADD COLUMN IF NOT EXISTS order_no TEXT;

UPDATE orders
SET order_no = 'TZ'
    || to_char(created_at AT TIME ZONE 'UTC', 'YYYYMMDDHH24MISS')
    || substr(replace(id::text, '-', ''), 1, 8)
WHERE order_no IS NULL OR order_no = '';

ALTER TABLE orders ALTER COLUMN order_no SET NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS orders_order_no_uidx ON orders (order_no);
