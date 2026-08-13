# 03 · 服务端前置项（本仓做不完的部分）

三站是纯静态站点，登录态与账号数据全部来自 Sub2API。下面几项**必须由 Sub2API 侧
与运维配合完成**，前端改代码解决不了；没完成就上线，表现是「页面能打开、一登录就报错」。

## 1. Sub2API 的 CORS

三站直连 `https://api.lumio.games`，没有同源反代，所以浏览器会对每个请求做跨源检查。

需要放行的来源：

```text
https://lumiogame.com
https://cc.lumiogame.com
https://codex.lumiogame.com
```

本地联调还需要（可只在预发 / 开发环境放行）：

```text
http://localhost:5280    # apps/portal
http://localhost:5281    # apps/cc
http://localhost:5282    # apps/codex
```

请求头要求：

- 允许请求头 `Authorization`、`Content-Type`、`Accept`——账号中心的所有已登录请求都带
  `Authorization: Bearer <access token>`（`web/packages/auth/src/client.ts`）。
- 允许方法 `GET`、`POST`，并正确响应 `OPTIONS` 预检。
- **不需要** `Access-Control-Allow-Credentials`：令牌走请求头，不走 cookie。

> 只放行 `lumiogame.com` 是不够的：产品站也会在页面加载时读会话（`useSession`）。

## 2. `GET /api/v1/auth/me` 的信封与字段

这是三站与控制面共同依赖的**唯一身份端点**，两个实现对它的宽容度不同，
所以上线前必须实际 `curl` 一次核对：

```bash
curl -sS -H 'Authorization: Bearer <一个有效 access token>' \
  https://api.lumio.games/api/v1/auth/me
```

| 消费方 | 代码位置 | 对响应的要求 |
| --- | --- | --- |
| 三站前端 | `web/packages/auth/src/client.ts` | **必须**是统一信封 `{ code, message, reason, data }` 且 `code === 0`；`data` 里读 `id`（数字）、`email`（字符串）、`balance`（数字）、`status`（字符串） |
| CCHaven 控制面 | `cchaven/services/cchaven-control/internal/sub2api/client.go` | 信封或裸对象都能解析；`id` 允许字符串或数字，`balance` 允许字符串或数字；只有缺 `id` 才判失败 |

因此**以前端的要求为准**（更严）：

- [ ] 响应是信封形态，成功时 `code` 为 `0`
- [ ] `data.id` 是数字（前端 `num()` 拿不到数字会回落成 `0`）
- [ ] `data.email`、`data.status` 是字符串；`data.balance` 是数字（单位：元，前端按两位小数展示）
- [ ] 令牌无效时返回 401 / 403（控制面据此回 401；返回 200 加错误码会被当成有效身份）

字段名对不上时，**先改前端映射，不要改 Sub2API 数据**——Sub2API 还服务着存量桌面客户端。

## 3. 其他账号端点

门户用到的端点全在 `web/packages/auth/src/client.ts`，都要求同一套信封：

| 端点 | 方法 | 用途 |
| --- | --- | --- |
| `/api/v1/settings/public` | GET | 注册开关、邮箱验证开关、邮箱后缀白名单、协议文档 |
| `/api/v1/auth/send-verify-code` | POST | 注册验证码 |
| `/api/v1/auth/register` | POST | 注册（可带 `verify_code`、`invitation_code`） |
| `/api/v1/auth/login` | POST | 登录；返回 `requires_2fa` 时进 2FA 步骤 |
| `/api/v1/auth/login/2fa` | POST | 提交 `temp_token` + `totp_code` |
| `/api/v1/auth/refresh` | POST | 用 `refresh_token` 换新令牌 |
| `/api/v1/auth/logout` | POST | 注销 refresh token |
| `/api/v1/auth/me` | GET | 见 §2 |

约定要点（与桌面端一致，见 `codex/crates/codex-plus-core/src/lumio/api.rs`）：

- 2FA 挑战是 **HTTP 200 的成功响应**，只能靠 `requires_2fa` 字段判断，不能靠状态码。
- 限流中间件的响应**不套信封**，所以状态码分支必须在没有信封时也能工作。

## 4. 充值页

- [ ] `https://api.lumio.games/purchase` 可直接在浏览器打开并完成充值。

门户账户中心、Codex 桌面端与 CC 的下单端点最终都落到这一个地址：

| 入口 | 代码位置 |
| --- | --- |
| 门户账户中心「充值」按钮 | `web/packages/ui/src/config.ts` → `purchaseUrl()` |
| Codex 桌面端「充值」 | `codex/crates/codex-plus-core/src/lumio/product.rs` → `payment_url()` |
| CC 控制面 `POST /api/v1/billing/checkout` | 返回 **303**，`Location` 即该地址 |

这个地址**不允许**配成官网域名。

## 5. CCHaven 控制面侧

控制面的完整部署看 [`cchaven/docs/ops/02-deploy-production.md`](../../cchaven/docs/ops/02-deploy-production.md)，
与三站上线直接相关的只有这几条环境变量（名字以
[`cchaven/services/cchaven-control/.env.example`](../../cchaven/services/cchaven-control/.env.example) 为准）：

| 变量 | 生产取值 | 影响 |
| --- | --- | --- |
| `CCHAVEN_SUB2API_BASE` | `https://api.lumio.games` | 身份真源；配错则**全部**终端用户请求 401/503 |
| `CCHAVEN_PORTAL_URL` | `https://lumiogame.com` | 410 响应里的指路地址，同时是 CORS 可信来源 |
| `CCHAVEN_PUBLIC_URL` | `https://cc.lumiogame.com` | CC 产品站地址；可信来源之一 |
| `CCHAVEN_ADMIN_URL` | `https://admin.cchaven.cn` | 运营后台来源；漏配时生产环境后台写操作 403（本地 dev 测不出来） |
| `CCHAVEN_SUB2API_CACHE_TTL` | `1m` | 身份校验缓存；调大省外部调用，代价是停用账号生效更慢 |

预期行为（上线后逐条验证，见 [04-golive-checklist.md](./04-golive-checklist.md)）：

- 自有认证端点（`/api/v1/auth/register|login|…`）返回 **410 `auth_migrated`**，
  `details.portal_url` 指向门户登录页。
- `POST /api/v1/billing/checkout` 返回 **303**，`Location` 是 Sub2API 充值页。
- Sub2API 不可达时返回 **503 `identity_unavailable`**，不静默放行、也不伪装成 401。

### 5.1 门户 `/authorize` 的跨源要求（CC 桌面端登录链路）

CC 桌面端的浏览器登录页开在门户（`https://lumiogame.com/authorize`），授权端点仍在控制面，
所以确认页是**跨源**调控制面的，浏览器会先发 `OPTIONS` 预检：

| 页面 | 调用的端点 | 方法 |
| --- | --- | --- |
| `https://lumiogame.com/authorize` | `https://api.cc.lumiogame.com/api/v1/oauth/authorize/context` | GET |
| `https://lumiogame.com/authorize` | `https://api.cc.lumiogame.com/api/v1/oauth/authorize` | POST，带 `Authorization: Bearer <Sub2API access token>` |

- [ ] `CCHAVEN_PORTAL_URL=https://lumiogame.com`（可信来源的唯一出处是
      `config.Config.TrustedOrigins`，CORS 与同源校验共用），否则预检拿不到
      `Access-Control-Allow-Origin`，**CC 桌面端在生产完全无法登录**。dev 放行 localhost
      任意端口，本地联调看不出这个问题。
- [ ] 允许请求头 `Authorization`、`Content-Type`（控制面 `cors` 中间件已下发）。
- [ ] 本地联调门户（`http://localhost:5280`）时控制面须跑在 `CCHAVEN_ENV=dev`，
      或把门户地址配进 `CCHAVEN_PORTAL_URL`。
- [ ] 控制面地址若不是默认的 `https://api.cc.lumiogame.com`，需给门户配
      `VITE_CC_CONTROL_URL`（见 [`web/README.md`](../../web/README.md) 的环境变量表）。

## 6. 一次性、高风险的存量用户迁移

迁移前在 CC 侧注册的账号在 Sub2API 没有对应身份，需要 `cmd/migrate-identities` 补映射。
**这会在外部系统创建真实用户、并写生产库，必须先取得负责人确认。**
步骤、dry-run 与注意事项写在
[`cchaven/docs/ops/02-deploy-production.md`](../../cchaven/docs/ops/02-deploy-production.md) §10，
本文不重复。要提前公告的一点：迁移**不带口令**，这批用户首次登录必须走账号中心的「忘记密码」。
