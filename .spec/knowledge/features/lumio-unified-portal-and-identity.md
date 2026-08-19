---
name: lumio-unified-portal-and-identity
description: 统一门户与统一身份：门户 + BestCodex 单站、Sub2API 唯一账号源、跨域会话、控制面令牌校验、充值与余额扣款——改账号面或站点时查
metadata:
  type: doc
  status: 已交付
---

# Lumio 统一门户与统一身份

一个总门户（`lumiogame.com`）分发两个子产品，账号只有一套：邮箱 / 口令 / 2FA / 余额
全部由 Sub2API（`api.lumio.games`）保管，产品站与 CC 控制面都不再自持终端用户账号。

## 背景 / 目标

- 合并前两个产品各有一套注册登录（Lumio Codex 对接 Sub2API，CC避风港自建于
  `cchaven-control`），同一个人要注册两次，运营也拿不到统一的用户视图。
- 目标：**一处注册、处处可用**——账号入口收敛到门户一个页面，产品站只做介绍与下载，
  充值统一落到 Sub2API 的收银台；Claude 包月开通走控制面余额支付，不走门户、不走收银台。
- 约束：`api.lumio.games` 被存量桌面客户端硬编码，迁移期间不可变更；CC 的存量业务数据
  （订阅、邀请、设备、订单）必须原地保留。

## 设计

### 站点分工（`web/`，npm workspaces）

| 工作区 | 域名 | 职责 |
|--------|------|------|
| `apps/portal` | 独立门户（本期部署不变） | 用户可见品牌 **BestCodex**；**唯一**注册 / 登录 / 2FA / 账户中心（`/login`、`/signup`、`/account`、`/logout`）+ CC 授权页 `/authorize`。账号源仍是 Sub2API |
| `apps/bestcodex` | `bestcodex.app` | 产品站：`/` `/codex` 为 Codex 落地页，`/claude` 为 Claude 落地页，帮助 `/help` |

两站是纯静态 SPA，共用 `packages/ui` 与 `packages/auth`。
门户没有营销首页：`/` 重定向到 `/account`，顶栏品牌标也落 `/account`；产品介绍只在产品站。
产品站**不做自己的登录**，账号入口一律跳门户并带 `?next=` 回跳（`portalAccountLinks()`）。
`next` 只放行站内相对路径与根域下的地址（`isAllowedNext()`），防开放重定向。
门户与产品站在 apex 的共存见 [ADR-0007](../../decisions/0007-bestcodex-apex-portal-coexistence.md)。旧 `apps/cc` / `apps/codex` 已退役。

### 身份：Sub2API 是唯一真源

- 门户直连 Sub2API 的 auth 系列端点（`packages/auth/src/client.ts`），统一信封
  `{ code, message, reason, data }`，`code === 0` 才算成功；2FA 挑战是 HTTP 200 的成功响应，
  只能靠 `requires_2fa` 判断；限流响应不套信封。
- 桌面端（`codex/crates/codex-plus-core/src/lumio/`）连的是同一套端点与同一份用户数据，
  与门户不是两套账号。
- CC 控制面 `cchaven-control` 降级为纯业务服务：拿请求里的
  `Authorization: Bearer <Sub2API access token>` 调 `GET {base}/api/v1/auth/me` 认人
  （`internal/sub2api/client.go`，带短 TTL 缓存），再映射到本地影子账号
  （`sub2api_identities` 表，首次出现即开户）。上游不可达时返回 503 `identity_unavailable`，
  **不静默放行**；自有终端用户认证端点保留路由但返回 410 `auth_migrated`，
  `details.portal_url` 指向门户登录页。管理员账号与强制 TOTP 仍完全在控制面本地。

### 会话

令牌写在规范账号根域 Cookie（`.bestcodex.app`，`Path=/`、`SameSite=Lax`、https 下带 `Secure`），
由 `packages/auth/src/session.ts` 读写。父域 Cookie 必须前端可读，因此**不是 HttpOnly**，
风险由「短 access token 有效期 + 收紧 CORS + 门户可信根域」兜住。
产品站不做自己的登录页，账号入口走门户路径（线上 apex 按路径把 `/login` `/account` 指到门户产物）并带 `?next=`。
遗留主机 `lumiogame.com` 与 `bestcodex.app` 不在同一注册域，Cookie 过不去：门户在遗留主机上会整页搬到规范账号 origin，若本地已有会话则用 URL 片段做一次性令牌交接（`packages/auth/src/handoff.ts`），落地立刻抹掉 hash。
Sub2API 的 refresh 是轮换式，禁止两套 Cookie 长期并存。独立访问遗留主机且本地无会话时，只做无令牌回跳——规范主机上若已登录即可接上。运维应对 `lumiogame.com` 做 301，前端回跳是未切 DNS 前的兜底。
日后接入统一网关时 `session.ts` / `handoff.ts` 是改动点。

### CC 桌面端授权（门户 `/authorize`）

CC 桌面端仍由 `cchaven-control` 签发自己的令牌，但授权时的用户身份来自 Sub2API：桌面端开
`{portal}/authorize?client_id=cchaven-desktop&redirect_uri=http://127.0.0.1:<port>/callback&…`
（PKCE S256），门户先按控制面注册的回调形态校验 `redirect_uri`，再带 Sub2API access token
调 `GET/POST {cc}/api/v1/oauth/authorize`（`apps/portal/src/lib/ccControl.ts`——控制面信封是
`{data}` / `{error}`，与 Sub2API 不同，故不复用 `packages/auth`），最后按响应 `redirect_to`
跳回本机回环。`redirect_uri` 与 `redirect_to` 都过白名单（`isAllowedDesktopRedirect()`），
越界不跳。令牌交换 `/oauth/token` 仍在桌面端，门户不参与。

前置：控制面须配 `CCHAVEN_PORTAL_URL` 把门户列为可信来源（CORS 与 CSRF 同源校验共用同一份
`TrustedOrigins`），漏配则桌面端完全登不进，且 dev 放行 localhost 任意端口、本地测不出来。

### 充值与余额开通

收银台仍是 Sub2API：`https://api.lumio.games/purchase`。本仓不收集任何付款信息。
浏览器里已登录时，门户与产品站的「充值」走
`{api}/auth/bridge#t=<access>&r=/purchase`（`purchaseUrl(accessToken)`），把已有令牌交给
LumioAPI 控制台会话；未登录仍直开 `/purchase`。桌面端 `payment_url()` 与 CC 控制面
`PurchaseURL()` 不带浏览器会话，保持 `/purchase`。

控制面 `POST /api/v1/billing/checkout` 仍是 303 到 `/purchase`，只充值、不建单、不开通。
Claude 包月开通走 `POST /api/v1/billing/pay-with-balance`：控制面用**当前用户** JWT 调 LumioAPI `POST /api/v1/user/balance/debit`，并带服务端 `X-Balance-Client-Key`（`CCHAVEN_BALANCE_CLIENT_SECRET`，禁止进前端/日志）。金额按分编码成 `19.90`，`currency=CNY`，`purpose=cchaven_monthly`，`Idempotency-Key` 与 `ref` 都是本地订单号。订阅天数只写在控制面（+30 天，不自动续费）。余额不足返回 403 `insufficient_balance` + `purchase_url`。请求金额走严格 `ParseYuan`（`19.901` 仍拒绝）。回执余额只是快照：`ParseYuanSnapshot` 允许 0、超过两位小数按分四舍五入；`txn_id` 与金额已对上时，余额解不开或为 0 不得返回 503，只打 warn（余额原文，禁止 token / `bcs_` / 完整 body），`BalanceCents` 可当 0，订单仍标 `paid` 并 `CreditPurchase`。扣款已成功但本地还 pending 时，必须用原 `order_no` 重放 debit，禁止换新幂等键。
门户 `/account` 登录后用页签分栏：账户 / 余额 / 开通记录 / 邀请返利。`#orders`（及旧锚 `#claude-orders`）落地开通记录页签。开通记录页签读控制面 `GET /api/v1/me/entitlement` 与 `GET /api/v1/billing/orders`（信封走 `lib/ccControl.ts`，不复用 `@lumio/auth`）：展示当前有效期 / 剩余天数，以及开通账单表格。pending 写「处理中，请勿重复支付」。门户**不做套餐、不扣款**；none / expired 引导回桌面 Claude Tab。时长一律用服务端 `days_left` / `expires_at`（本地日历日），不要用 `account.planLabel` 或 `created_at+30`。桌面 Claude Tab 的「开通记录」打开 `https://bestcodex.app/account#orders`，不在客户端内嵌账单列表。

## 待解决

- `/authorize` 只有单测，未与真库跑的控制面端到端联调；首次联调重点看
  `OPTIONS` 预检与 `redirect_to` 的实际形态。授权请求的设备字段（`device_name` /
  `os_version` / `arch` / `app_version`）浏览器无从得知，当前送空对象。
- 门户账户中心已读控制面权益与开通账单；邀请视图仍未接。
- CC 的旧站 `cchaven/apps/web` 与旧 Codex 官网 `codex/site/` 待下线。
- 存量 CC 用户的身份映射需一次性迁移（`cmd/migrate-identities`，高风险，需单独确认）；
  迁移不带口令，这批用户首次登录必须走「忘记密码」。
- `api.cc.lumiogame.com` 域名待最终确认；`lumiogame.com` 上的 Workflow 页面待迁离。

## 相关

- 跨产品运维手册（门户 + 产品站部署 / 域名切换 / 上线验收）：[`docs/ops/`](../../../docs/ops/README.md)
- 统一官网开发说明：[`web/README.md`](../../../web/README.md)
- CC 侧线上拓扑与账号体系：[`cchaven/docs/ops/01-architecture.md`](../../../cchaven/docs/ops/01-architecture.md)
- Codex 侧后台契约：[`codex/docs/ops/04-backend.md`](../../../codex/docs/ops/04-backend.md)
- 桌面账户设计：[`lumio-account-and-home.md`](lumio-account-and-home.md)
- [ADR-0002 Sub2API 为唯一账号中心](../../decisions/0002-sub2api-single-account-source.md)
- [ADR-0003 双仓合并为并列 monorepo](../../decisions/0003-monorepo-three-way-merge.md)
- [ADR-0012 Claude 余额开通](../../decisions/0012-claude-balance-subscribe.md)
