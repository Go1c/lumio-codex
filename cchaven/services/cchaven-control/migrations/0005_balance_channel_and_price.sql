-- 余额支付渠道 + Claude 包月统一为 19.9 元。
--
-- orders.channel 原先只允许 alipay/wechat/card/mock。后续用 Sub2API 余额
-- 扣月费时会写入 channel='balance'，必须先放行约束。
-- 价格写进 ops_configs：已有库走 UPSERT 覆盖 68 元旧值；缺省回落在
-- store.defaultConfig（1990 分）。0002 种子与测试重置同为 1990；
-- 存量订单夹具若写死历史金额，不要改已经落库的那一笔。

ALTER TABLE orders DROP CONSTRAINT IF EXISTS orders_channel_check;
ALTER TABLE orders ADD CONSTRAINT orders_channel_check
  CHECK (channel IN ('alipay', 'wechat', 'card', 'mock', 'balance'));

INSERT INTO ops_configs (key, value, updated_at)
VALUES ('pricing.monthly', '{"amount_cents":1990,"currency":"CNY"}', now())
ON CONFLICT (key) DO UPDATE
   SET value = EXCLUDED.value, updated_at = now();
