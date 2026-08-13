---
name: lumio-unified-portal-and-identity
description: 统一门户与统一身份：三站分工、Sub2API 唯一账号源、跨子域会话、控制面令牌校验、充值落点——改账号面或站点时查
metadata:
  type: doc
  status: 实施中
---

# Lumio 统一门户与统一身份

一个总门户（`lumiogame.com`）分发两个子产品，账号只有一套：邮箱 / 口令 / 2FA / 余额
全部由 Sub2API（`api.lumio.games`）保管，产品站与 CC 控制面都不再自持终端用户账号。

## 背景 / 目标

- 合并前两个产品各有一套注册登录（Lumio Codex 对接 Sub2API，CC避风港自建于
  `cchaven-control`），同一个人要注册两次，运营也拿不到统一的用户视图。
- 目标：**一处注册、处处可用**——账号入口收敛到门户一个页面，产品站只做介绍与下载，
  充值统一落到 Sub2API 的收银台。
- 约束：`api.lumio.games` 被存量桌面客户端硬编码，迁移期间不可变更；CC 的存量业务数据
  （订阅、邀请、设备、订单）必须原地保留。

## 设计

### 三站分工（`web/`，npm workspaces）

| 工作区 | 域名 | 职责 |
|--------|------|------|
| `apps/portal` | `lumiogame.com` | 品牌首页 + **唯一**的注册 / 登录 / 2FA / 账户中心（`/login`、`/signup`、`/account`、`/logout`）+ CC 桌面端授权页 `/authorize` |
| `apps/cc` | `cc.lumiogame.com` | CC避风港产品站（介绍 / 定价 / 下载）|
| `apps/codex` | `codex.lumiogame.com` | Lumio Codex 产品站（介绍 / 下载）|

三站是纯静态 SPA，共用 `packages/ui`（设计 token 抽自原 CC 站）与 `packages/auth`；
产品站**不做自己的登录**，账号入口一律跳门户并带 `?next=` 回跳（`portalAccountLinks()`）。
`next` 只放行站内相对路径与根域下的地址（`isAllowedNext()`），防开放重定向。

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

### 跨子域会话

令牌写在父域 Cookie（`.lumiogame.com`，`Path=/`、`SameSite=Lax`、https 下带 `Secure`），
三站共读（`packages/auth/src/session.ts`）。父域 Cookie 必须前端可读，因此**不是 HttpOnly**，
风险由「短 access token 有效期 + 收紧 CORS + 三站同属一个可信根域」兜住；
日后接入统一网关时 `session.ts` 是唯一改动点。

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

### 充值

只有一个落点 `https://api.lumio.games/purchase`：门户账户中心按钮（`purchaseUrl()`）、
桌面端「充值」（`product.rs` 的 `payment_url()`）、CC 控制面
`POST /api/v1/billing/checkout`（返回 303，`Location` 即该地址）三条路径殊途同归。
本仓不收集任何付款信息。

## 待解决

- `/authorize` 只有单测，未与真库跑的控制面端到端联调；首次联调重点看
  `OPTIONS` 预检与 `redirect_to` 的实际形态。授权请求的设备字段（`device_name` /
  `os_version` / `arch` / `app_version`）浏览器无从得知，当前送空对象。
- 除 `/authorize` 外，三站都不调用 CC 控制面，门户上没有 CC 的权益 / 邀请视图。
- CC 的旧站 `cchaven/apps/web` 与旧 Codex 官网 `codex/site/` 待下线。
- 存量 CC 用户的身份映射需一次性迁移（`cmd/migrate-identities`，高风险，需单独确认）；
  迁移不带口令，这批用户首次登录必须走「忘记密码」。
- `api.cc.lumiogame.com` 域名待最终确认；`lumiogame.com` 上的 Workflow 页面待迁离。

## 相关

- 跨产品运维手册（三站部署 / 域名切换 / 上线验收）：[`docs/ops/`](../../../docs/ops/README.md)
- 统一官网开发说明：[`web/README.md`](../../../web/README.md)
- CC 侧线上拓扑与账号体系：[`cchaven/docs/ops/01-architecture.md`](../../../cchaven/docs/ops/01-architecture.md)
- Codex 侧后台契约：[`codex/docs/ops/04-backend.md`](../../../codex/docs/ops/04-backend.md)
- 桌面账户设计：[`lumio-account-and-home.md`](lumio-account-and-home.md)
- [ADR-0002 Sub2API 为唯一账号中心](../../decisions/0002-sub2api-single-account-source.md)
- [ADR-0003 双仓合并为并列 monorepo](../../decisions/0003-monorepo-three-way-merge.md)
