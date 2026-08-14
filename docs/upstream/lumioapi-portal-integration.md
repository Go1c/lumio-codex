# LumioAPI（api.lumio.games）与 Lumio 主站集成改造说明

> 状态：**待上游实施**。LumioAPI 是 [Wei-Shaw/sub2api](https://github.com/Wei-Shaw/sub2api) 的自有部署，源码不在本仓库；本文是给 LumioAPI 维护者的改造清单，含验收标准。三个问题都源自 2026-08-14 主站联调实测（用户报告 + QA 复核）。

## 背景：三个实测问题

| # | 问题 | 实测证据 |
| --- | --- | --- |
| 1 | 从主站点「充值」跳到 `https://api.lumio.games/purchase` 后**不是登录态**，需在 LumioAPI 再登录一次 | 干净浏览器访问 `/purchase` → 302 `/login?redirect=/purchase`。根因：主站会话 cookie 在 `.lumiogame.com`，LumioAPI 控制台会话在 `api.lumio.games` 的 localStorage（`auth_token`/`refresh_token`，见 `frontend/src/stores/auth.ts`），两个注册域之间物理隔离 |
| 2 | LumioAPI 页面**左上角 logo 无法返回主站**，只做站内跳转（`/home`） | 用户在 404 页上点击左上角无出口可回 lumiogame.com |
| 3 | 用户两次撞到 LumioAPI 的 404（"Error / This page could not be found"） | 2026-08-14 复核时 `/purchase` 路由已存在（302 登录页），404 疑似更早的部署版本残留；需确认线上版本并让 404 页有出口 |

主站侧契约（本仓库，三处一致、无需改动）：`https://api.lumio.games/purchase` —— 门户 `purchaseUrl()`（`web/packages/ui/src/config.ts`）、桌面端 `product::payment_url()`、CC 控制面 `PurchaseURL()`。

## 改动一：跨域登录交接（`/auth/bridge`）

**原理**：主站登录后持有的 `lumio_at` 本来就是 LumioAPI 签发的 access token。跳转时通过 URL 片段（`#` 后，不进服务器日志）带给 LumioAPI，由 bridge 页换成控制台自己的 localStorage 会话。

**后端**（新增一个接口）：

```
POST /api/v1/auth/bridge
Authorization: Bearer <现有 access token>
→ 200 { access_token, refresh_token, expires_in, user: {...} }
```

- 校验逻辑复用现有 access token 中间件；换发**新的**令牌对（走既有签发路径），不回显旧 token；
- 可选加固：限制该接口只接受未过期的 access token（不支持 refresh token 直调）。

**前端**（新增一个路由页面 `frontend/src/views/auth/BridgeView.vue`）：

1. 路由 `/auth/bridge`（公开，title "登录中…"）；
2. 读取 `location.hash` 中的 `t`（token）与 `r`（目标路径，默认 `/purchase`）；
3. 调 `POST /api/v1/auth/bridge`；
4. 成功：按 `stores/auth.ts` 现有格式写入 `auth_token` / `refresh_token` / `token_expires_at` / `auth_user`；
5. `history.replaceState` 抹掉 URL 片段，再 `router.replace(r)`；
6. 失败（token 过期/无效）：跳 `/login?redirect=<r>`，与现状一致。

**验收**：主站登录后点「充值」→ 直达 `/purchase` 且为登录态；地址栏与浏览器历史不残留 `#t=`；伪造/过期 `t` 落在登录页。

**主站侧配合**（本仓库，等上游上线后再做）：门户「充值」按钮在有会话时生成
`https://api.lumio.games/auth/bridge#t=<access_token>&r=/purchase`，无会话时维持 `/purchase`。桌面端与 CC 站不改（无浏览器会话，落登录页合理）。

## 改动二：左上角 logo 可配置返回主站

LumioAPI 已有管理后台可配的站点品牌（`site_logo` / `site_name`，经 public settings 下发，`HomeView.vue` 等处消费 `cachedPublicSettings`）。顺着同一机制：

1. public settings 增加 `site_home_url`（默认空 = 现行为 `/home`）；
2. 页头 / 404 页的 logo 点击：`site_home_url` 非空时 `window.open(site_home_url, '_self')`（跨站跳转，不用 router.push），为空时维持站内 `/home`；
3. 管理后台「站点设置」加对应字段。

**部署配置**：`site_home_url = https://lumiogame.com`。

**验收**：任意 LumioAPI 页面点左上角 → 回到 lumiogame.com；未配置的部署行为不变。

## 改动三：404 出口与版本核对

1. 核对线上部署版本包含 `/purchase` 路由（`frontend/src/router/index.ts` 用户端路由，`PaymentView.vue`）；2026-08-14 复核已存在，若再现 404 先查部署版本；
2. 404 页（`Go Home` 按钮旁）增加「返回 Lumio 官网」链接，同样读 `site_home_url`，与改动二联动。

## 安全注意事项

- token 只放 URL **片段**（`#`）：不随请求发给服务器、不进访问日志；
- bridge 换发新令牌对 + 片段即用即抹 + access token 本身 1~2 小时短时效，历史记录残留的暴露窗口很小；
- `r` 参数只允许**站内相对路径**（以 `/` 开头且不含 `//`），防止开放重定向；
- 不要把 refresh token 放进 URL。

## 关联

- QA 报告：`docs/qa/2026-08-14-monorepo-qa-review.md`（W-19、W-28）
- 账号架构：`.spec/knowledge/features/lumio-unified-portal-and-identity.md`（充值落点契约）
