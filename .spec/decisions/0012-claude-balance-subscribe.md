# 0012 · Claude 包月钱在 Sub2API、订阅权在控制面，不在 Sub2API 做套餐

- 日期：2026-08-19
- 状态：生效

## 背景

BestCodex 桌面 Claude Tab 要卖「在自己服务器跑 Claude」的包月权。钱已经在
Sub2API 账户余额里；订阅天数、到期日、开通订单已经在 `cchaven-control`。

若在 Sub2API 再做一套 Claude 套餐 / 自动续费 / 周期扣款，就会出现两个真源：
钱动了但控制面不知道，或控制面开了权但余额没扣。托管收银台（支付宝 / 微信）
和门户账户中心都不是开通页。`POST /api/v1/billing/checkout` 已经是 303 到
Sub2API `/purchase` 的充值入口，不能再拿它当开通。

## 决策

**钱只在 Sub2API 扣，订阅权只在 cchaven-control 记。不在 Sub2API 做 Claude
套餐、自动续费或周期扣款。**

- 桌面开通卡走控制面 `POST /api/v1/billing/pay-with-balance`（用户 Sub2API
  Bearer + `Idempotency-Key`）。
- 控制面用同一用户令牌调 Sub2API `POST /api/v1/user/balance/debit`（路径可配
  `CCHAVEN_SUB2API_DEBIT_PATH`，默认 `/api/v1/user/balance/debit`）：金额与
  `GET /api/v1/auth/me` 的 `data.balance` 同单位（元），`purpose=cchaven_monthly`，
  `ref` 为控制面订单号；认请求头 `Idempotency-Key`。
- 当前套餐价 **1990 分（¥19.9）**，一次 +30 天，不自动续费。已是 `active`
  再买则顺延 30 天。
- 余额不足：控制面 403 `insufficient_balance` + `purchase_url`；桌面打开
  `https://api.lumio.games/purchase`（`paymentUrl`），禁止打开门户账户中心当开通页。
- `POST /api/v1/billing/checkout` 仍是 303 到 `/purchase`，只充值、不建单、不开通。
- 开通订单 `channel=balance` 落在控制面；用户开通记录是
  `GET /api/v1/billing/orders`；后台订单页能看到。本期门户没有 Claude 开通区。
- 不恢复支付宝 / 微信托管收银台作为开通主路径。不改 Codex 按量网关扣费。

## 后果

- 开通成功依赖两跳：Sub2API 扣款成功之后，控制面才把订单标 `paid` 并入账。
  上游不可用返回 503，订单保持 pending，绝不假装开通。
- Sub2API 未交付 debit 前，本仓测试走 `FakeSub2API`。对方若只能提供管理员
  扣款接口，只改控制面 `debit.go` 的鉴权头与路径，不改桌面、不改入账、不改 19.9。
- 钱包与订阅对账靠 `purpose` + `ref`（订单号）和同一把 `Idempotency-Key`；
  Sub2API 控制台看不到 Claude 有效期。
- 0002 里「`/billing/*` 只服务存量订单」不再成立：余额开通会在控制面建新单。
  checkout 与托管回调仍只服务充值 / 存量渠道。
