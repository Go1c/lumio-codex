-- 余额支付的幂等键是「同一用户同一 key」，不是全局唯一。
-- 不同用户可以碰巧用同一把客户端钥匙；同一用户的同一把钥匙只能对应一笔订单。
ALTER TABLE orders DROP CONSTRAINT IF EXISTS orders_idempotency_key_key;

CREATE UNIQUE INDEX ux_orders_user_idempotency
    ON orders (user_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;
