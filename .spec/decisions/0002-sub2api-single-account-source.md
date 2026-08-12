# 0002 · 以 Sub2API 为唯一账号中心，cchaven-control 降级为纯业务服务

- 日期：2026-08-13
- 状态：生效

## 背景

合并前两个产品各有一套终端用户账号：Lumio Codex 桌面端对接 Sub2API
（`https://api.lumio.games`，账号 / 余额 / 充值都在那里），CC避风港则由 Go 控制面
`cchaven/services/cchaven-control` 自建注册、登录、验证码、找回密码、改邮箱改口令与 2FA。

同一个人要在两个产品各注册一次，余额与充值也各走一套；运营拿不到统一的用户视图，
而统一官网（`lumiogame.com` 门户 + 两个产品子站）如果照搬现状，就得在一个站里放两个登录入口。

两个真源合并只能选一个方向：把 Sub2API 的账号搬进控制面，或反过来。
`api.lumio.games` 被存量 Lumio Codex 桌面客户端硬编码（`lumio/product.rs` 的 `API_BASE_URL`），
搬走会让所有已安装的老客户端整体失联，没有回旋余地。

## 决策

**Sub2API 是全 Lumio 唯一的用户身份与账号数据源**，`cchaven-control` 降级为只做 CC 业务的服务：

- 终端用户的邮箱 / 口令 / 2FA / 账号状态 / 余额只存在于 Sub2API；控制面不再自持。
- 控制面对每个终端用户请求，拿 `Authorization: Bearer <Sub2API access token>` 调
  `GET {base}/api/v1/auth/me` 认人，结果带短 TTL 缓存（`CCHAVEN_SUB2API_CACHE_TTL`，默认 1 分钟）。
  上游不可达时返回 **503 `identity_unavailable`**，绝不静默放行、也不伪装成 401。
- 身份映射表 `sub2api_identities`（`sub2api_user_id ↔ users.id`）是业务侧身份主键；
  Sub2API 用户首次出现即在 CC 侧开出影子账号，`users` 的历史数据与列一并保留。
- 控制面的自有终端用户认证端点**保留路由、返回 410 `auth_migrated`**，
  `details.portal_url` 指向门户登录页；不删路由是为了让存量客户端能区分
  「这个能力永久搬走了、该去哪」与「路径写错了」。
- 充值统一落 `https://api.lumio.games/purchase`：控制面的
  `POST /api/v1/billing/checkout` 改为返回 303 指向它，不再自建收银台。
- 浏览器侧的注册 / 登录 / 2FA / 账户中心只在门户 `lumiogame.com` 一处实现，
  产品站的账号入口一律跳门户并带 `next` 回跳。
- 管理员账号与强制 TOTP **不动**，仍完全在控制面本地（运营后台与终端用户是两套主体）。

## 后果

- **Sub2API 成为单点**：它不可用时，CC 的全部终端用户请求会返回 503，门户也登不上。
  换来的是账号数据不再需要双向同步——两个真源同时在线才是更糟的一致性问题。
- **多一跳外部调用**：每次认人可能打一次外部 HTTP。用短 TTL 缓存摊平，代价是账号中心
  停用某个账号后，最多一个 TTL 才在 CC 侧生效。
- **存量 CC 用户需要一次性迁移**：迁移前注册的账号在 Sub2API 没有身份，需由
  `cmd/migrate-identities` 补映射。该工具会在外部系统创建真实用户并写生产库，属高风险操作，
  必须单独确认后执行；且**不迁移口令**（本地只有不可逆摘要），这批用户首次登录必须走
  账号中心的「忘记密码」——这条要提前公告。
- **契约耦合到外部服务**：`/api/v1/auth/me` 的信封与字段由 Sub2API 说了算。控制面的解析
  宽容（信封或裸对象、`id` 允许字符串或数字），门户前端较严（要求 `code === 0` 的信封、
  `id` 为数字），上线前必须实测核对。字段对不上时改本仓映射，不改 Sub2API 数据。
- **`users.password_hash` 留下历史包袱**：字段仍在（不删历史数据），新建影子账号写入的是
  不合法的 argon2 占位值，任何口令都匹配不上。
- CC 的订单表与 `/billing/*` 只服务迁移前的存量订单，属于半通的支付链路，
  需在 Sub2API 侧账单回传接口定稿后消费上游账单或整体下线，不宜长期保留。
