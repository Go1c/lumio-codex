# M1 控制面服务 — 表结构与 API 清单

> 权威依据：`docs/design/interaction-design.md`（v3 评审稿）与 `design/prototype/`。
> 本文档只描述 M1（控制面服务）范围。冲突与缺口见文末「规范缺口与建议」。

- 服务名：`cchaven-control`（新建，不改造 `fast-note-sync-service`；理由见 [`docs/adr-0001-new-service.md`](./adr-0001-new-service.md)）
- 模块路径：`github.com/Go1c/fns-workspace/services/cchaven-control`
- 数据库：PostgreSQL 14+，迁移脚本 `migrations/*.sql` 随代码，服务启动时自动执行
- 对外前缀：用户面 `/api/v1`，管理端 `/api/admin/v1`

---

## 0. 附录 A 依赖逐条核对

| 附录 A 条目 | 依赖能力 | M1 落点 | 状态 |
| --- | --- | --- | --- |
| 注册/验证码/登录/找回 | 控制面 identity API | `auth.*` 表 + `/api/v1/auth/*` | ✅ M1 |
| 登录设备列表与撤销 | session family 管理 | `session_families` / `refresh_tokens` + `/api/v1/me/sessions` | ✅ M1 |
| 订阅状态（已订阅/试用中/未订阅） | 单一套餐 entitlement snapshot | `subscriptions` + `/api/v1/me/entitlement` | ✅ M1 |
| 邀请裂变 | 邀请码生成/归因、三步闭环事件、试用发放与「每账号一次」、防滥用 | `referral_codes` / `referral_visits` / `referral_attributions` / `trial_fingerprints` | ✅ M1 |
| 邀请者奖励 | 可配 X 天（0 关闭）、奖励发放事件、累计延长天数查询 | `ops_configs` + `subscription_events` + `/api/v1/me/referrals` | ✅ M1 |
| 分阶段部署进度 | agent 部署阶段事件上报 | 桌面端本地事件，不经控制面 | ⛔ M3 |
| 同步状态条 | sync-core 聚合状态事件 | `fns-sync-core` → Tauri 事件 | ⛔ M3 |
| 冲突列表与解决 | 冲突读取 + 三种 resolution | `fns-sync-core` | ⛔ M3 |
| 运营后台 | 管理员账号体系（含 2FA）、DAU/注册/收入/留存聚合、用户列表与禁用、订单查询与退款、运营配置读写、审计日志 | `admins` / `admin_sessions` / `audit_logs` / `orders` / `refunds` / `ops_configs` + `/api/admin/v1/*` | ✅ M1（UI 在 M4） |

**附录 A 未列出、但正文交互要求、M1 必须提供的能力**（补齐项）：

| 来源 | 能力 | M1 落点 |
| --- | --- | --- |
| 3.4 / 5.1 | OAuth 授权码 + PKCE 授权服务器、`/authorize` 确认页、回环回调 + 自定义 scheme | `oauth_clients` / `oauth_authorization_codes` + `/api/v1/oauth/*` |
| 4.2 / 5.6 / 6.5 | 价格、邀请奖励天数、试用时长由后台下发（页面不写死） | `/api/v1/config/public` |
| 4.3 | 下载页版本号与更新日期、DMG 链接 | `app_releases` + `/api/v1/config/public` |
| 5.6「安全」 | 修改密码、修改邮箱（两步验证 + 原邮箱通知） | `/api/v1/me/password`、`/api/v1/me/email-change/*` |
| 5.6「个人资料」 | 显示名称可编辑 | `PATCH /api/v1/me` |
| 5.6「危险区」 | 注销账号 7 天冷静期，期间可撤销 | `users.deletion_requested_at` + `/api/v1/me/deletion` |
| 7.1 / 7.2 | 平台分布、APP 版本分布、用户列表「使用平台」列 | `user_devices` + `/api/v1/app/heartbeat` |
| 7.1 | DAU、7 日留存 | `user_activity_days` |

---

## 1. 表结构

约定：主键 `BIGSERIAL`（除非注明 UUID）；时间列一律 `TIMESTAMPTZ`；枚举用 `TEXT + CHECK`（便于迁移，不用 PG ENUM）；所有令牌只存 SHA-256 摘要，绝不存明文；金额一律以「分」为单位的 `BIGINT`。

### 1.1 身份与账号

**`users`**

| 列 | 类型 | 说明 |
| --- | --- | --- |
| `id` | BIGSERIAL PK | 序列从 `100000` 起，对外展示为 `U-{id}`（原型 `U-100986`） |
| `email` | TEXT NOT NULL | 存小写规范化值；`UNIQUE` |
| `password_hash` | TEXT NOT NULL | Argon2id 编码串 `$argon2id$v=19$m=...` |
| `display_name` | TEXT NOT NULL DEFAULT '' | 账户中心可编辑 |
| `status` | TEXT NOT NULL | `pending_email` / `active` / `disabled` |
| `email_verified_at` | TIMESTAMPTZ NULL | |
| `locked_until` | TIMESTAMPTZ NULL | 登录失败锁定（15 分钟） |
| `failed_login_count` | INT NOT NULL DEFAULT 0 | 成功登录清零 |
| `registration_source` | TEXT NOT NULL | `organic` / `invite` / `other`（后台「来源」列） |
| `referred_by_user_id` | BIGINT NULL FK users | 归因邀请者 |
| `trial_granted_at` | TIMESTAMPTZ NULL | **每账号一次试用的硬约束载体** |
| `deletion_requested_at` | TIMESTAMPTZ NULL | 7 天冷静期起点 |
| `disabled_at` / `disabled_by_admin_id` / `disabled_reason` | | 后台禁用 |
| `signup_ip` / `signup_user_agent` | INET / TEXT | 防滥用 |
| `last_active_at` | TIMESTAMPTZ NULL | 后台「最近活跃」列 |
| `created_at` / `updated_at` | | |

索引：`UNIQUE(email)`、`(status)`、`(referred_by_user_id)`、`(created_at DESC)`。

**`email_verification_codes`** — 注册验证码与改邮箱验证码共用

`id`, `user_id` FK, `purpose`(`signup`|`email_change`), `target_email`（改邮箱时为新邮箱）, `code_hash`(SHA-256(code+pepper)), `expires_at`（+10 分钟）, `attempts_used` INT, `max_attempts` INT DEFAULT 5, `consumed_at`, `last_sent_at`, `created_at`。
部分唯一索引：`UNIQUE(user_id, purpose) WHERE consumed_at IS NULL` —— 同一用途同时只存在一个有效码，重发即替换。

**`password_reset_tokens`**

`id`, `user_id` FK, `token_hash`(SHA-256 of 32 随机字节), `expires_at`（+20 分钟）, `consumed_at`, `requested_ip`, `created_at`。一次性。

### 1.2 会话族（session family + refresh rotation）

**`session_families`** — 一次登录 = 一个会话族 = 「登录设备与授权」列表中的一行

`id` UUID PK, `user_id` FK, `client`(`web`|`app`), `oauth_client_id` NULL, `device_name`（`MacBook Pro — CC避风港 APP 1.4.2` / `Safari · macOS`）, `platform`(`browser`|`macos`), `platform_detail`（`macOS 15 · Apple Silicon`）, `app_version`, `user_agent`, `ip` INET, `ip_region`（`上海`）, `created_at`, `last_seen_at`, `revoked_at`, `revoked_reason`(`user_logout`|`user_revoke`|`revoke_others`|`password_change`|`password_reset`|`admin_disable`|`reuse_detected`|`account_deleted`)。

**`refresh_tokens`**

`id` UUID PK, `family_id` FK, `token_hash` UNIQUE, `issued_at`, `expires_at`, `used_at`, `replaced_by_id`, `revoked_at`。
**重放检测**：presented token 的 `used_at IS NOT NULL` → 整个 family 立即撤销（`reuse_detected`）。

Access token：JWT（HS256，15 分钟），claims 含 `sub`(user id)、`sid`(family id)、`jti`、`scope`。每个受保护请求校验签名 + 查 `session_families.revoked_at IS NULL` + `users.status='active'`，保证「禁用用户立即被登出」。

### 1.3 OAuth 2.0 授权服务器（APP 通过浏览器登录）

**`oauth_clients`**：`id` TEXT PK（`cchaven-desktop`）, `name`, `redirect_uri_patterns` TEXT[]（`http://127.0.0.1:*/callback`、`cchaven://auth/callback`）, `is_public` BOOL（公开客户端，强制 PKCE、无 secret）, `scopes` TEXT[], `created_at`。

**`oauth_authorization_codes`**：`code_hash` PK, `client_id`, `user_id`, `redirect_uri`, `scope`, `code_challenge`, `code_challenge_method`（只接受 `S256`）, `expires_at`（+5 分钟）, `consumed_at`, `device_name`, `platform`, `platform_detail`, `app_version`, `created_at`。
授权请求本身无状态（参数在 query 中每次校验），仅落库已签发的 code。

### 1.4 订阅与订单

**`subscriptions`** — 单一包月，每用户一行

`user_id` PK FK, `kind`(`trial`|`paid`), `expires_at` TIMESTAMPTZ NULL, `trial_expires_at` NULL, `bonus_days_total` INT DEFAULT 0, `created_at`, `updated_at`。
对外状态**派生**，不落库：`expires_at IS NULL` → `none`；`expires_at > now()` → `trialing`(kind=trial) / `active`(kind=paid)；否则 `expired`。

**`subscription_events`** — 时长变更总账（可审计、可回溯）

`id`, `user_id` FK, `type`(`trial_granted`|`invite_bonus`|`purchase`|`refund_revoke`|`admin_adjust`), `days_delta` INT, `expires_at_before`, `expires_at_after`, `ref_type`, `ref_id`, `note`, `created_at`。
唯一索引：`UNIQUE(user_id) WHERE type='trial_granted'` —— 数据库层保证**每账号只发一次试用**。
唯一索引：`UNIQUE(ref_type, ref_id) WHERE type='invite_bonus'` —— 同一次邀请只奖励一次。

**`orders`**：`id`, `order_no` TEXT UNIQUE（`CC{YYYYMMDD}-{6 位序号}`）, `user_id`, `amount_cents` BIGINT, `currency` TEXT DEFAULT 'CNY', `channel`(`alipay`|`wechat`|`card`|`mock`), `status`(`pending`|`paid`|`refunding`|`refunded`|`failed`), `period_months` INT DEFAULT 1, `provider`, `provider_txn_id`, `idempotency_key` UNIQUE, `paid_at`, `created_at`, `updated_at`。
每日序号由 `order_no_seq_{date}` 逻辑生成（`orders` 表内 `COUNT + advisory lock`，或独立 `order_sequences(day, next_seq)` 表——采用后者，避免并发空洞）。

**`order_sequences`**：`day` DATE PK, `next_seq` BIGINT。

**`payment_events`**：`id`, `order_id` FK, `type`, `provider`, `payload` JSONB, `signature_ok` BOOL, `created_at`（webhook 原始记录，便于对账）。

**`refunds`**：`id`, `order_id` FK, `amount_cents`, `status`(`pending`|`succeeded`|`failed`), `requested_by_admin_id`, `provider_refund_id`, `reason`, `created_at`, `completed_at`。

### 1.5 邀请裂变

**`referral_codes`**：`code` TEXT PK（`mary8k2f` 风格，8 位小写字母数字，无易混字符）, `user_id` FK UNIQUE, `disabled_at`, `created_at`。

**`referral_visits`**（三步闭环第 1 步：链接→cookie）：`id`, `code`, `visitor_id` UUID（HttpOnly cookie `cch_ref` 内的值）, `ip`, `user_agent`, `created_at`。

**`referral_attributions`**（第 2、3 步：注册 → 首次 APP 登录）

`id`, `code`, `inviter_user_id` FK, `invitee_user_id` FK **UNIQUE**（首次归因胜出，一人只归因一次）, `visitor_id`, `stage`(`registered`|`activated`), `registered_at`, `activated_at`, `trial_granted` BOOL, `inviter_bonus_days` INT DEFAULT 0, `inviter_bonus_granted_at`, `created_at`。
`CHECK (inviter_user_id <> invitee_user_id)` —— 禁止自邀。

**`trial_fingerprints`**（防重复领取）：`id`, `kind`(`device`|`payment`|`signup_ip`), `value_hash`, `user_id`, `created_at`，`UNIQUE(kind, value_hash)`。命中已存在指纹 → 拒发试用，返回固定文案「每个账号只可享用一次免费试用。」

### 1.6 运营配置与发布

**`ops_configs`**：`key` TEXT PK, `value` JSONB, `updated_at`, `updated_by_admin_id`。

| key | 类型 | 默认 | 用途 |
| --- | --- | --- | --- |
| `invite.reward_days` | int | `7` | 邀请者每邀 1 人延长天数，`0` = 关闭（前端隐藏相关文案） |
| `invite.trial_days` | int | `30` | 被邀请者免费试用时长 |
| `pricing.monthly` | `{amount_cents,currency}` | `{6800,"CNY"}` | 官网定价页 / 账户中心读取 |

**`app_releases`**：`id`, `version`, `channel`(`stable`), `arch`(`arm64`|`x86_64`), `download_url`, `min_os`, `released_at`, `is_current` BOOL。

### 1.7 遥测与后台

**`user_devices`**（后台「使用平台」列 + 平台/版本分布）：`id`, `user_id` FK, `device_id` TEXT（客户端稳定 ID）, `platform`, `os_version`, `arch`, `app_version`, `first_seen_at`, `last_seen_at`，`UNIQUE(user_id, device_id)`。

**`user_activity_days`**（DAU / 留存）：`user_id` BIGINT, `day` DATE, PK`(user_id, day)`；活动时 `ON CONFLICT DO NOTHING`。

**`admins`**：`id`, `email` UNIQUE, `password_hash`(Argon2id), `display_name`, `role`(`owner`|`ops`|`support`), `totp_secret_enc`（AES-GCM，密钥来自环境变量）, `totp_enabled_at`, `status`(`active`|`disabled`), `failed_login_count`, `locked_until`, `last_login_at`, `created_at`。

**`admin_sessions`**：`id` UUID, `admin_id`, `token_hash`, `mfa_passed` BOOL, `ip`, `user_agent`, `expires_at`, `revoked_at`, `created_at`。

**`audit_logs`**（7.5 要求：操作人 + 时间 + 前后值）：`id`, `actor_type`(`admin`|`user`|`system`), `actor_id`, `action`, `target_type`, `target_id`, `before` JSONB, `after` JSONB, `ip`, `user_agent`, `created_at`。

**`email_outbox`**（可靠投递 + 可测试：测试断言 outbox，不依赖 SMTP）：`id`, `to_email`, `template`, `payload` JSONB, `status`(`pending`|`sent`|`failed`), `attempts`, `last_error`, `created_at`, `sent_at`。

---

## 2. API 清单

统一响应：成功 `{"data": ...}`；失败 `{"error":{"code":"...","message":"<zh-CN 文案>","details":{...}}}` + 语义化 HTTP 状态码。
6.2 节固定文案由服务端 `internal/i18n` 下发，前端可直接展示（同时给 `code` 供前端自行本地化）。

Web（官网与管理后台）使用 HttpOnly Cookie 会话（`cch_sess` / `cch_refresh` / `cch_admin`，Secure，SameSite 默认 Lax、跨站部署时配 `none`，见 §4.11）；APP 使用 `Authorization: Bearer`。

跨源与 CSRF：可信来源恰好两个——官网 `CCHAVEN_PUBLIC_URL` 与管理后台 `CCHAVEN_ADMIN_URL`（后台按 7. 要求独立部署在 `admin.cchaven.cn`）。CORS 响应头与 cookie 写操作的同源校验共用这一个集合；dev 额外放行 localhost 任意端口。

### 2.1 公共配置

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/api/v1/config/public` | 价格、`invite.reward_days`、`invite.trial_days`、当前版本与 DMG 下载地址。定价页/账户中心/邀请页/下载页全部读这里 |
| GET | `/api/v1/health` | 存活探针 |
| GET | `/api/v1/invites/current` | 读 `cch_ref` cookie，回答当前浏览器是否仍处于有效邀请归因下。详见 §2.5 |

限频：`/config/public`、`/invites/current`、`/invites/{code}` 三个匿名只读接口按 IP 限 300 次/分钟。
配额给得宽松是因为它们支撑官网首屏，而办公网常整栋楼共用一个出口 IP——误伤真实访客的代价大于这几条廉价查询被刷的代价。
`/health` **不限频**：探针被配额挡住会让编排系统误判服务不可用。

### 2.2 注册 / 验证 / 登录 / 找回（`/api/v1/auth`）

| 方法 | 路径 | 请求 | 成功 | 失败（HTTP / code / 文案） |
| --- | --- | --- | --- | --- |
| POST | `/register` | `{email,password}` + `cch_ref` cookie | `201 {email, next:"verify_email"}`，**不发放任何会话** | `409 email_taken`「该邮箱已注册。」；`429 rate_limited`「尝试次数过多，请 1 分钟后再试。」 |
| POST | `/verify-email` | `{email,code}` | `200 {user, entitlement}` + 建立 Web 会话 | `400 code_invalid`「验证码不正确，还剩 {n} 次尝试机会。」；`410 code_expired`「该验证码已过期，请重新发送。」；`429 code_attempts_exhausted` |
| POST | `/verification-code/resend` | `{email}` | `202 {retry_after:60}` | `429 resend_cooldown`「尝试次数过多，请 {n} 秒后再试。」 |
| POST | `/login` | `{email,password}` | `200 {user, entitlement}` + Web 会话 | `401 invalid_credentials`「邮箱或密码不正确。」；`403 email_unverified`；`423 account_locked`「尝试次数过多，请 {n} 分钟后再试。」；`403 account_disabled`「账号已停用，请联系支持。」 |
| POST | `/password/forgot` | `{email}` | `202` 恒定成功「如 {email} 已注册账号，你将很快收到重设链接。」 | 仅 `429` |
| GET | `/password/reset/{token}` | — | `200 {valid:true, email_masked}` | `410 reset_token_invalid`「该链接已过期或已被使用。」 |
| POST | `/password/reset` | `{token,password}` | `200`，撤销该账号**全部**会话 | `410 reset_token_invalid` |
| POST | `/refresh` | cookie 或 `{refresh_token}` | `200 {access_token,expires_in}` + 轮换 | `401 session_expired`「登录已过期，请重新登录。」 |
| POST | `/logout` | — | `204`，撤销当前会话族 | — |
| GET | `/session` | — | `200 {user, entitlement}` | `401` |

限频（M1 内存令牌桶，接口留 Redis 实现位）：注册/登录/找回 按 IP 与按邮箱双维度；验证码重发 60 秒冷却；验证码 5 次错误锁定；登录 5 次失败锁 15 分钟。

### 2.3 OAuth 2.0（APP 通过浏览器登录，`/api/v1/oauth`）

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/authorize/context` | 入参 `client_id,redirect_uri,scope,code_challenge,code_challenge_method,state`。返回 `{client:{name}, scopes:[{id,label}], logged_in, user:{email}, redirect_kind:"loopback"\|"scheme"}`，供 `/authorize` 确认页渲染（未登录 → 前端先走登录，回来后继续） |
| POST | `/authorize` | 需 Web 会话。校验参数后签发授权码，返回 `{code, redirect_to, expires_in}`。页面跳 `redirect_to`；失败时把 `code` 显示给用户，对应 APP「手动粘贴授权码」兜底 |
| POST | `/token` | `grant_type=authorization_code`（`code,code_verifier,client_id,redirect_uri`）或 `grant_type=refresh_token`。返回 `{access_token,refresh_token,expires_in,token_type:"Bearer"}`。**首次 APP 登录在此触发试用发放与邀请闭环** |
| POST | `/revoke` | `{token}` 撤销 refresh token 所属会话族 |

PKCE 强制 `S256`；`redirect_uri` 必须精确匹配注册的回环模式或自定义 scheme；授权码一次性、5 分钟过期、绑定 `code_challenge`。

### 2.4 账户（`/api/v1/me`，Web 会话或 APP token）

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/` | `{user:{id_display:"U-100986",email,display_name,created_at}, entitlement}` —— APP 账户菜单与官网账户中心共用 |
| PATCH | `/` | `{display_name}` |
| GET | `/entitlement` | `{status:"trialing"\|"active"\|"none"\|"expired", kind, expires_at, days_left, bonus_days_total}` |
| POST | `/password` | `{current_password,new_password}` → 撤销**其他**会话，保留当前 |
| POST | `/email-change` | `{new_email}` → 向新邮箱发验证码 |
| POST | `/email-change/verify` | `{code}` → 原子切换 + 原邮箱通知邮件 |
| DELETE | `/email-change` | 取消改邮箱流程 |
| GET | `/sessions` | 会话族列表：`{id,device_name,platform,platform_detail,app_version,last_seen_at,ip_region,current:bool,kind:"web"\|"app"}` |
| DELETE | `/sessions/{id}` | 退出该设备（撤销授权） |
| POST | `/sessions/revoke-others` | 退出所有其他设备 |
| GET | `/referrals` | `{code, link, reward_days, trial_days, invited_count, total_bonus_days, items:[{email_masked,status:"registered"\|"activated",bonus_days,at}]}`；`reward_days=0` 时前端隐藏奖励文案 |
| POST | `/deletion` | 申请注销，`{effective_at}`（+7 天） |
| DELETE | `/deletion` | 冷静期内撤销注销 |

### 2.5 邀请（公开）

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/api/v1/invites/{code}` | 落地页数据 `{valid, code, inviter, trial_days}`；**同时下发 `cch_ref` HttpOnly cookie（30 天）并记 `referral_visits`**。`valid:false` 时前端展示「此邀请链接已失效」但不阻断注册 |
| GET | `/api/v1/invites/current` | 首页邀请横幅（4.1 第 3 块「有邀请码 cookie 时高亮显示」）的权威数据源。读 `cch_ref` cookie：无 cookie、邀请码不存在或已停用 → `{attributed:false}`；有效 → `{attributed:true, inviter, trial_days}`，字段语义与 `/invites/{code}` 一致（共用 `service.lookupInvite`）。**只读**：不下发 cookie，也**不记 `referral_visits`** |

`cch_ref` 是 HttpOnly 的，前端 JS 读不到，所以横幅必须由服务端裁决。前端自己缓存的展示副本（localStorage）既不会随 30 天 cookie 过期、也不会因邀请码被停用而失效，会造成「首页高亮承诺首月免费、注册后实际拿不到」的用户可见错误承诺——归因只以服务端 cookie 为准。`invites/current` 刻意不记访问：`referral_visits` 统计的是「邀请链接被打开」这一次事件，首页每次渲染都会调用本接口，记进去会把三步闭环第一步刷成假数据。

### 2.6 订阅与付款（只在官网）

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/api/v1/billing/plan` | 单一包月套餐（读 `ops_configs.pricing.monthly`） |
| POST | `/api/v1/billing/checkout` | `{channel}` → `{order_no, pay_url, expires_at}`。M1 为 mock adapter，`PaymentProvider` 接口预留支付宝/微信实现 |
| GET | `/api/v1/billing/orders` | 当前用户订单列表 |
| GET | `/api/v1/billing/orders/{order_no}` | 轮询支付状态 |
| POST | `/api/v1/billing/webhook/{provider}` | 支付回调（验签 → 幂等入账 → 延长 `subscriptions.expires_at`） |

### 2.7 APP 遥测

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| POST | `/api/v1/app/heartbeat` | APP token。`{device_id,app_version,os_version,arch}` → 更新 `user_devices`、`user_activity_days`、`session_families.last_seen_at`；返回 `{entitlement, notices:[{type:"expiring_soon",days_left}]}` 驱动「剩余 ≤3 天」横幅 |

### 2.8 管理端（`/api/admin/v1`，独立账号体系 + TOTP）

#### 2.8.1 角色权限矩阵

`admins.role` 三种取值的能力边界。收敛原则只有一条：**破坏性操作的权限门槛不得低于读取敏感信息**。
矩阵的唯一实现出处是 `internal/service/admin.go` 的 `roleCapabilities`，由 `admin_capability_test.go` 逐格锁定。

| 能力 | support | ops | owner |
| --- | --- | --- | --- |
| 仪表盘指标、用户列表（邮箱打码）、订单列表、审计日志、读运营配置 | ✅ | ✅ | ✅ |
| 用户详情 `GET /users/{id}`（**明文邮箱**） | ❌ | ✅ | ✅ |
| 禁用 / 解禁用户 | ❌ | ✅ | ✅ |
| 订单退款 | ❌ | ✅ | ✅ |
| 修改运营配置 | ❌ | ✅ | ✅ |
| 导出订单 CSV | ❌ | ✅ | ✅ |

- **support 全线只读**：能看的都能看，一个写操作都不给。
- **导出 CSV 算在写操作一侧**：它一次性把上千行用户邮箱（即便打码）落到本地文件，属于批量数据外带，
  与「查一页订单」不是一回事。
- **owner 与 ops 目前完全相同，这是刻意的**，不是漏写。眼下两者没有任何真实职责差异，
  硬造一个只会变成需要靠记忆维持的规则。保留 `owner` 是为了将来的**管理员账号管理**
  （新增/停用管理员、修改他人角色）只给 owner——那将是第一个值得区分的能力，M1 还没有对应接口。
- **被拒一律 403 `forbidden`，且先写审计再返回**：审计动作是 `{原动作}_denied`
  （`user.view_detail_denied` / `user.disable_denied` / `user.enable_denied` / `order.refund_denied` /
  `ops_config.update_denied` / `orders.export_denied`），带 target 与 IP，`after.actor_role` 为被拒角色。
  拒绝路径不返回任何数据，故审计写失败只记 `slog.Error`，仍返回 403，不升级为 500。
  `ops_config.update_denied` 的 target 是本次提交的 key 列表（已排序），`orders.export_denied` 的 target 是筛选条件。

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| POST | `/auth/login` | `{email,password}` → `{mfa_required:true, mfa_token}`（已启用 TOTP）或直接建会话 |
| POST | `/auth/login/totp` | `{mfa_token, code}` → 建会话 |
| POST | `/auth/logout` / GET `/auth/me` | |
| POST | `/auth/totp/setup` / `/auth/totp/enable` | 首次强制启用 2FA 的注册流程 |
| GET | `/metrics/overview` | 六张卡：DAU（含环比）、今日新增注册（含经邀请数）、付费订阅用户（含试用中）、今日收入（含笔数）、试用→付费转化率（近 30 天）、7 日留存（含较上周）。缺数返回 `null`，前端显示「—」 |
| GET | `/metrics/dau?days=7` | 近 7 日柱状图数据 |
| GET | `/metrics/distributions?days=30` | `{platform:[], app_version:[], source:[]}` |
| GET | `/users` | `?query=&status=all\|sub\|trial\|none\|banned&page=&page_size=`。每行同时下发 `id`（展示用注册号 `U-100986`）与 `user_id`（数字主键，调用详情/禁用/退款用），前端不需要从 `id` 反解 |
| GET | `/users/{id}` | ✅ 已实现，`{id}` 为数字主键。详情：账号信息（含 `user_id` 与**明文邮箱**）、订阅快照、设备列表、邀请汇总与进度、最近 10 笔订单。**二次权限**（§2.8.1）：仅 `owner` / `ops`，`support` 403；放行时写一条 `audit_logs`（`action='user.view_detail'`），拒绝时写 `user.view_detail_denied`，详见 §4.10。用户不存在返回 404 |
| POST | `/users/{id}/disable` | `{reason}` → 立即撤销全部会话 + 审计。仅 `owner` / `ops`（§2.8.1），被拒写 `user.disable_denied` |
| POST | `/users/{id}/enable` | 仅 `owner` / `ops`，被拒写 `user.enable_denied` |
| GET | `/orders` | `?status=all\|paid\|refunding\|refunded\|failed&page=`，附当日汇总 `{count, amount_cents}` |
| GET | `/orders/export` | 按当前筛选导出 CSV（上限 5000 行）。批量数据外带，权限与写操作同级：仅 `owner` / `ops`。**成功与被拒都写审计**：成功写 `orders.export`（`after` 含筛选条件、行数、是否触顶），被拒写 `orders.export_denied`；审计写不进去就不交出数据 |
| POST | `/orders/{order_no}/refund` | 二次确认由前端负责；服务端幂等，状态 `paid → refunding → refunded`。仅 `owner` / `ops`，被拒写 `order.refund_denied` |
| GET | `/configs` | 全部运营配置。三种角色都可读——客服要能回答「现在奖励几天、包月多少钱」 |
| PUT | `/configs` | 批量写入 + 审计前后值。仅 `owner` / `ops`，被拒写 `ops_config.update_denied` |
| GET | `/audit-logs` | ✅ 已实现。`?actor=&action=&page=&page_size=`；`actor` 匹配 `actor_id`，`action` 精确匹配，两者可组合，空串表示不筛选 |

---

## 3. 关键链路：注册 → 授权 → 试用发放 → 邀请奖励

端到端测试（`test/e2e_referral_test.go`）覆盖：

1. 邀请者 A 注册并验证邮箱 → 拿到 `referral_codes.code`。
2. 被邀请者 B `GET /invites/{code}` → 收到 `cch_ref` cookie，落 `referral_visits`。
3. B 携带 cookie `POST /auth/register` → `referral_attributions` 生成，`stage=registered`，`users.registration_source='invite'`。
4. B `POST /auth/verify-email` → 账号激活；**此时尚未发放试用**。
5. B 走 OAuth：`POST /oauth/authorize` → `POST /oauth/token` → 首次 APP 会话建立。
   - 同一事务内：发放 `invite.trial_days` 天试用（`subscription_events.type='trial_granted'`）、`attribution.stage='activated'`、给 A 追加 `invite.reward_days` 天（`type='invite_bonus'`）、写 `email_outbox` 两封通知。
6. 断言：B `entitlement.status='trialing'` 且 `days_left=30`；A `bonus_days_total=7`；`GET /me/referrals`（A）返回 1 人 activated、+7 天。
7. 重复触发（B 二次授权、B 删除后重注册同指纹）→ 不重复发放，`trial_fingerprints` 命中返回固定文案。
8. `invite.reward_days=0` 时链路仍闭合，但 A 不获得奖励且响应中 `reward_days=0`。

---

## 4. 规范缺口与建议（不改规范，仅记录）

1. **验证码重发冷却口径不一**：6.2 节与 3.1 节均为 60 秒，原型 `verify-email` 实测为 10 秒并自注「规格为 60 秒」。→ 后端按 **60 秒**实现；原型的 10 秒仅为演示。
2. **限频文案单位**：6.2 表格为「尝试次数过多，请 {n} {unit}后再试。」，而 4.5 注册页写死「请一分钟后再试。」。→ 服务端下发时统一走 `{n}{unit}` 模板，注册场景填 `1 分钟`，视觉结果与 4.5 一致。
3. **「账号锁定」不是账号状态**：3.2 区分 `locked` 与 `disabled`，但 `locked` 是 15 分钟自动解除的临时态。→ 建模为 `users.locked_until`，而非 `status` 取值，避免与后台「已禁用」筛选混淆。
4. **后台用户「订阅状态」筛选混入了 `banned`**：`已禁用` 与 `已订阅/试用中/未订阅` 不是同一维度（一个被禁用的用户仍可能在订阅期内）。→ 后端筛选参数按单一 `status` 处理并以 `disabled` 优先展示，与原型视觉一致；建议 M4 评审时拆成两个筛选维度。
5. **`/authorize` 页原型缺失**：原型未实现该页，只在 APP 侧模拟。→ 按 5.1 与 3.4 的描述实现（确认页 + 授权码兜底展示），M2 落地时需要设计补图。
6. **注册来源「其他渠道」无定义**：后台有该枚举值但正文未定义采集方式。→ 预留 `utm_source` 入参，非 `organic`/`invite` 一律归入 `other`。
7. **退款对订阅时长的影响未定义**：7.3 只描述订单状态流转。→ 已确认按「退款成功即扣回该订单对应天数」实现（写 `subscription_events.type='refund_revoke'`），不影响试用与邀请奖励累积的天数。
8. **邮箱打码规则原型内部不一致**：同一张表里同时出现 `w***g@gmail.com`（首尾各留一位）与 `chen***@163.com`（保留前四位）两种写法。→ 后端统一采用信息泄露更少的一种：本地部分保留首尾各一位，中间固定三个星号；不足 3 字符时只保留首字符。
9. **验证码重发的防枚举口径**：4.6 未说明对未注册邮箱重发时应如何响应。→ 与忘记密码一致，一律返回 202 与冷却秒数，不泄露账号是否存在。
10. **管理员权限矩阵未定义**：7.5 只写了「用户详情页需二次权限」，既没说明哪些角色算「有权限」，也没有给出 `admins.role`（`owner` / `ops` / `support`）三种角色的职责边界。→ **已按下述原则实现完整矩阵（§2.8.1），但仍待产品确认。**
    - **曾经的半套矩阵比完全没有角色更危险**：早期只有 `GET /users/{id}` 做了角色判定，其余管理端写操作对三种角色一视同仁，于是 `support` **不能查看**用户明文邮箱（只读），却**可以禁用**该用户（破坏性，立即把真实客户锁在门外）、可以发起退款、可以改包月价格。这种倒挂给人一种「权限受控」的错觉，实际把最贵的操作留在门外无人看守。
    - **收敛原则**：破坏性操作的权限门槛不得低于读取敏感信息。据此 `support` 收敛为**全线只读**；导出订单 CSV 归入写操作一侧（一次性把上千行用户邮箱落到本地文件，属批量数据外带）。完整矩阵见 §2.8.1。
    - **owner 与 ops 暂无差异是刻意的**，不是漏写：两者眼下没有任何真实职责差异，硬造一个只会变成需要靠记忆维持的规则，评审时也无从判断对错。保留 `owner` 这个角色是为了将来的**管理员账号管理**（新增/停用管理员、修改他人角色）只给 owner——那是第一个值得区分的能力，M1 尚无对应接口。
    - **实现形态**：角色判定不散落在 handler，集中为一张能力表 `roleCapabilities`（默认拒绝）加一组语义化谓词（`CanViewUserDetail` / `CanManageUsers` / `CanRefundOrder` / `CanEditOpsConfig` / `CanExportOrders`），由 `internal/service/admin_capability_test.go` 逐格锁定；判定统一放在 service 方法入口，因为拒绝路径要写带 target 的审计，只有那里同时拿得到操作人与目标。
    - **审计**：放行时 `GET /users/{id}` 仍在同一事务内写 `user.view_detail`（写不进审计就不返回数据）；被拒时**先写 `{原动作}_denied` 再返回 403**，`after.actor_role` 为被拒角色，写失败只记 `slog.Error`、仍返回 403，不升级为 500。
    - **成功的导出也写审计**（已实现）：既然以「批量数据外带」为由限制这个能力，真正发生外带的那一次就比被拒的那一次更该可追溯，只审计失败是自相矛盾。审计记录筛选条件、行数与是否触顶，便于事后评估影响面。
    - **仍待产品确认**：(a) owner 与 ops 是否真的应当同权；(b) `support` 是否需要「查看明文邮箱但留痕」这类受限读取（当前完全不给）；(c) 是否需要第四种角色（如只读财务）；(d) 若运营抱怨导出被挡影响日常对账，可退一步改为「允许 support 导出但强制留痕 + 行数上限」。M4 后台 UI 评审时一并明确。
11. **后台独立部署对控制面的跨源要求未被点明**：7. 只说了后台「独立入口（如 `admin.cchaven.cn`），与官网、APP 完全隔离」，没有说明这对控制面意味着两个互不相干的浏览器来源。→ 可信来源建模为一个显式集合（`config.Config.TrustedOrigins` = `CCHAVEN_PUBLIC_URL` + `CCHAVEN_ADMIN_URL`），CORS 与 cookie 写操作的 CSRF 校验共用它；生产环境漏配 `CCHAVEN_ADMIN_URL` 时启动打 `slog.Warn`，因为此时后台的**每一个写操作**都会 403，而 dev 放行 localhost 会完全掩盖这个问题。新增前端只改这一处。
12. **会话 cookie 的 SameSite 不能写死**：规范假定控制面与前端同站，但部署形态未被约束。→ 增加 `CCHAVEN_COOKIE_SAMESITE`（`lax` / `none`，默认 `lax`）。判断依据是控制面与前端的 eTLD+1 是否相同：同站（`cchaven.cn` 与 `api.cchaven.cn`）保持 `lax` 更安全；不同站时必须配 `none`，否则浏览器根本不发送 cookie、登录直接失效。配 `none` 时服务强制 `Secure=true`（浏览器硬性要求），非 prod 环境下额外告警提示 cookie 可能被丢弃。不提供 `strict`，它会破坏「从邮件点重设密码链接跳回」的链路。
13. **首页邀请横幅的数据源缺失**：4.1 第 3 块要求「有邀请码 cookie 时高亮显示」，但归因载体 `cch_ref` 是 HttpOnly 的，前端 JS 读不到，规范未给出对应接口。→ 补 `GET /api/v1/invites/current`（见 §2.5）。前端自缓存的展示副本不可用作判据：它不随 cookie 过期、也不随邀请码停用而失效，会产生「首页承诺首月免费、注册后拿不到」的用户可见错误承诺。
