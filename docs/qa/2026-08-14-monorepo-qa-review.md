# Lumio Monorepo QA 审查报告（2026-08-14）

> 代码快照：`publish` 分支 `69f7b94`（工作区 clean）。文中行号以该 commit 为准。
> 审查方法：4 路并行只读代码审查（web 三站 + auth/ui 共享包、Go 控制面、桌面端前端 + Rust 账户模块），最高危 11 项已由主审逐一复核源码坐实（文中标 ✅复核）。未运行任何写操作。

## 范围与基线

| 范围 | 内容 |
| --- | --- |
| `web/apps/portal` + `web/packages/auth` + `web/packages/ui` | 总门户（账号中心，对接 Sub2API）、会话/共享 UI |
| `web/apps/cc`、`web/apps/codex` | 两个产品营销站（dark starfield 重构后） |
| `cchaven/services/cchaven-control` | Go 控制面（chi + pgx + PostgreSQL，账号收口 Sub2API） |
| `codex/apps/codex-plus-manager` + `codex/crates/codex-plus-core/src/lumio/` | 桌面前端（React 19 + Tauri 2）与 Rust 账户模块 |

基线状态：`go vet ./...` 与 `go test ./...` 全绿。总体结论：代码质量与安全意识较高（错误折叠、开放重定向防御、事务/行锁、IDOR 防护、可达性均扎实），但存在 **2 个功能性 P0/P1 级缺陷**（桌面端邀请码注册死循环、web 会话续期未闭环）与一批服务端纵深问题。

严重级别定义：

- **P0**：当前或某配置开启下核心功能不可用 / 可直接接管权限，需立即修。
- **P1**：重要功能失效、安全纵深缺口、显著损害用户的数据正确性或信任。
- **P2**：体验缺陷、隐患型问题、需特定条件触发的错误。

问题编号前缀：`W-` web 三站、`S-` cchaven-control 服务端、`D-` 桌面端。

---

## 一、P0 / P1（建议立即处理）

### D-1（P0·条件触发）桌面端邀请码注册完全不可用 ✅复核 → ✅已修（2026-08-14）

> **修复**：`lumio_register` 增加 `invitation_code: String` 参数并以 `non_empty()` 透传；settings 三层（`RawPublicSettings` → `PublicSettings` → `LumioServiceSettingsPayload`）补 `invitation_code_enabled` 下发（`#[serde(default)]`，服务端未下发时为 None，行为不回退）。测试：core wiremock 注册请求体含/不含邀请码两例、settings 开关映射一例、manager 命令面源码契约两例（`lumio_command_surface.rs`）。遗留：外部 Sub2API 需实际下发 `invitation_code_enabled` 字段，桌面端代码已就绪。

- 位置：`codex/apps/codex-plus-manager/src-tauri/src/lumio_commands.rs:341-354`
- 问题：前端 `src/lumio/invoke.ts:192` 发送 `invitationCode`，但 `lumio_register` 命令签名只收 `email/password/verify_code/accepted_revision`，第 353 行硬编码 `invitation_code: None`；Tauri 对多余 invoke 参数静默忽略。关联：`LumioServiceSettingsPayload`（`lumio_commands.rs:95-108`）不含 `invitationCodeEnabled`，前端 `types.ts:58` 的可选字段恒 undefined，注册页的邀请码输入框只能靠服务端报错后才出现。
- 触发/影响：服务端一旦开启邀请码模式——用户填了邀请码提交 → 码被静默丢弃 → 服务端返回 `AUTH_INVITATION_CODE_REQUIRED` → banner 提示「注册需要邀请码，请填写后重试」（用户明明已填）→ **死循环，该部署下无人能注册**。前端整套邀请码 UI（错误码映射、聚焦、清空）与 Rust 错误归一化均已就位，只差命令层接线。
- 修复建议：`lumio_register` 增加 `invitation_code: Option<String>` 参数并透传；`LumioServiceSettingsPayload` 补 `invitation_code_enabled` 下发。

### W-1（P1）Web 端 refresh token 形同虚设，1~2 小时后静默掉登录 ✅复核 → ✅已修（2026-08-14）

> **修复**：`session.ts` 新增 `readRefreshToken()`/`hasSession()`/`rotateSession(force)`——轮换在 Web Locks（可用时）内执行并在锁内重读 cookie，多标签页后来者复用新令牌而非拿旧令牌撞 `REFRESH_TOKEN_REUSED`；`useSession` 初始态用 `hasSession()`，effect 对「无 access、有 refresh」分支主动轮换再拉资料，既有 `AUTH_SESSION_EXPIRED` 路径同样改走 `rotateSession`；`signOut` 在仅剩 refresh cookie 时也通知服务端撤销。测试：仅 refresh 自动续期、续期失败清会话、access 被拒后刷新恢复三场景 + helper 单测。

- 位置：`web/packages/auth/src/session.ts:57-76`（`readSession` 以 `if (!accessToken) return null` 为门槛）、`web/packages/auth/src/useSession.tsx:36-43`（effect 对 `!stored` 直接置 anonymous）
- 问题：`writeSession` 把 `lumio_at` cookie 的 Max-Age 设为 access TTL（服务端 1~2 小时），到期浏览器即删除；此时 30 天的 `lumio_rt` cookie 虽在，`readSession()` 却因 access cookie 缺失返回 null——**`refreshTokens` 从不会被尝试**（刷新路径仅在「cookie 还在但服务端已判过期」的时钟偏差窄窗口内可达）。`expiresAt` 字段写入又读出但全仓无消费者，证明主动续期从未实现。
- 触发/影响：用户离开 1~2 小时后回来，任何子站刷新都变未登录、须重输密码；「30 天会话」承诺仅桌面端兑现。附带风险：Sub2API refresh 为轮转式（旧 rt 立即失效，有 `REFRESH_TOKEN_REUSED`），多标签页若同时命中刷新窗口会互相作废并清掉共享 cookie。
- 修复建议：`readSession` 在仅有 refresh cookie 时也返回可刷新的会话（或 effect 对「无 access、有 refresh」分支主动走一次刷新）；补「cookie 真实过期」的测试用例。

### S-1（P1）管理端 TOTP 验证码可无限暴破 ✅复核 → ✅已修（2026-08-14）

> **修复**：`/auth/login/totp` 挂新中间件 `rateLimitAdminTOTP`（`RuleAdminTOTPByIP`，10 次/分钟，按来源 IP）；`AdminVerifyTOTP` 前置锁定检查、失败复用 `RecordAdminLoginFailure`（5 次锁 15 分钟）、成功清零。测试：`TestAdminTOTPBruteForceLocksAccount`（5 连错后正确验证码也被 `account_locked` 拒绝）+ `TestRateLimitAdminTOTP` 单测。

- 位置：`cchaven/services/cchaven-control/internal/api/api.go:119`（`POST /api/admin/v1/auth/login/totp` 未挂限频）、`internal/service/admin.go:184-212`（`AdminVerifyTOTP` 对错误码只返回 `mfa_invalid`，不累计失败、不锁定）
- 问题：对比 `AdminLogin` 口令错误走 `RecordAdminLoginFailure`（5 次锁 15 分钟），TOTP 端点既无限频也无失败计数；`totp.Validate` 默认 skew 还允许相邻时间窗。
- 触发/影响：攻击者拿到管理员口令（半会话）后可对 6 位 TOTP 无限次在线穷举，小时级概率可暴破，直接接管后台（禁用用户、退款、改价）。
- 修复建议：挂上现有限频中间件 + 失败计数锁定（复用口令锁定机制即可），修复成本极低。

### S-2（P1）Sub2API 停用账号无法撤销已签发的 CC 会话（桌面端可无限续期）→ ✅部分修复（2026-08-14）

> **修复**：会话族增加绝对寿命上限 `SessionAbsoluteTTL`（默认 14 天，`CCHAVEN_SESSION_ABSOLUTE_TTL` 可配）——`RefreshSession` 在 `family.CreatedAt + TTL` 到期后拒绝轮换（`session_expired`），桌面端走既有浏览器重新授权流程，该链路每次回源校验 Sub2API 账号状态。把「停用永不生效」收敛为「≤14 天生效」。测试：`TestRefreshSessionAbsoluteLifetimeCap`。
> **附带修复**（复核时发现的既有缺陷）：重放检测分支在 `InTx` 闭包内撤销会话族后返回 error，pgx 回滚把撤销一并吞掉——401 响应正确但撤族从未落盘，新令牌仍可继续使用。改为撤销落盘后于事务外返回错误，`TestRefreshTokenReuseRevokesFamily` 恢复通过。
> **遗留**：「停用立即生效」需 Sub2API 提供按用户 ID 查询状态的接口（本仓库控制面仅有 `Verify(token)`），超出本仓库范围。

- 位置：`cchaven/services/cchaven-control/internal/service/session.go:101-116`（`RefreshSession` 只查本地 `session_families`/`users.status`）、`:158-192`（`AuthenticateAccess` 同样只查本地）
- 问题：上游停用仅在 Bearer 走 Sub2API 校验路径时生效（`identity.go:59-61`）。桌面端经 `/oauth/authorize` 拿到本服务签发的会话族后，access 校验与 refresh 轮换均不再回查 Sub2API，且每次轮换新发 60 天 refresh token（滑动续期）。
- 触发/影响：账号真源（README 明确为 Sub2API）的封禁决策对主客户端失效，被停用/封禁用户的 CC 桌面端权益可无限期使用，除非运营手工在本服务再禁一次（代码无任何同步）。
- 修复建议：refresh 轮换时定期（如每 24h 或每 N 次轮换）回查一次 Sub2API；或在本地会话加短暂 TTL 强制走上游校验。

### W-17（P1）CC 下载页「打开 APP」深链指向未注册的 URL scheme，死链 → ✅已修（2026-08-14）

> **修复**：按本报告建议先隐藏——移除 `web/apps/cc/src/pages/Download.tsx` 的 `cchaven://open` 深链按钮，改为说明文案「安装后从应用程序或启动台打开」。长期方案（Tauri 注册 deep-link，涉及新插件依赖）另立任务；旧站 `cchaven/apps/web` 为遗留站点未动。

- 位置：`web/apps/cc/src/pages/Download.tsx:5`（`const APP_DEEP_LINK = "cchaven://open"`）、`:51`（渲染为 `<a href="cchaven://open">`）
- 问题：桌面端 `cchaven/apps/desktop/src-tauri/tauri.conf.json` 无 `deep-link` 插件 / `CFBundleURLTypes` 注册；`cchaven/apps/desktop/docs/spec-gaps.md:110`（B4）明确「自定义 scheme 尚未注册……M3 未做」。该链接系从旧站原样搬来，重构后未修复。
- 触发/影响：点击后浏览器报「找不到处理该协议的应用」或无反应。门户 `/authorize` 的 `cchaven://auth/callback` 兜底（`web/apps/portal/src/lib/redirect.ts:43`）依赖同一 scheme，注册前同样不可用（好在有回环回调兜底）。
- 修复建议：桌面端注册 scheme 前先隐藏该按钮；长期在 Tauri 配置注册 deep-link。

---

## 二、S：cchaven-control 服务端问题（P2）

| 编号 | 问题 | 位置 |
| --- | --- | --- |
| S-3 | `POST /auth/logout` 缺 CSRF 防护：路由在 `requireUser` 之外、未经 `originAllowedFor`；部署为 `SameSite=none`（README 明确支持）时第三方站点表单 POST 即可登出用户 | `internal/api/api.go:57`、`handler_auth.go:30-42` |
| S-4 | 客户端 IP 可伪造：优先采信 XFF 首段 + chi `RealIP` 无信任名单；限频键（300/min）、登录/注册限频、`signup_ip`/审计 IP 均可被指定污染 | `internal/httpx/httpx.go:99-115`、`api.go:33` |
| S-5 ✅已修（2026-08-14：paid 分支比对 `notification.Amount` 与 `order.AmountCents`，不符则留痕 `signature_ok=false` 并拒绝入账、订单保持 pending；留痕在事务内提交、错误事务外返回，`TestWebhookRejectsAmountMismatch`） | 支付回调不校验金额：`notification.Amount` 从未与 `order.AmountCents` 比对（`payments.go:41` 定义了 Amount 却无人消费）；签名合法但金额不符的通知会把整单入账。当前仅 mock 渠道注册故降 P2，**接真实渠道前必须补** | `internal/service/billing.go:130-164` |
| S-6 | 清理任务未接线：`DeleteExpiredAuthorizationCodes` 注释写「由后台任务调用」但 `runMaintenance` 只调 `ExpireDeletedAccounts`；`oauth_authorization_codes`、`refresh_tokens`（每设备每 15 分钟一行）、`email_outbox` sent 行、过期 `admin_sessions` 永不删除，库持续膨胀 | `internal/store/oauth.go:119-126` vs `cmd/control/main.go:106-122` |
| S-7 | 运营配置写入无键白名单/值校验、读取静默吞错：写入形状错误的 `pricing.monthly` 会入库成功、响应「已更新」，但 `LoadOpsConfig` 反序列化失败被忽略，官网价格悄悄回落默认 ¥68 | `internal/service/admin.go:889-913`、`internal/store/opsconfig.go:57-96` |
| S-8 ✅已修（2026-08-14：迁移 0004 加 `last_attempt_at` 与 `sending` 中间态；领取即提交、SMTP 投递移到事务外，单封 15s 超时 + 拨号 5s（含 STARTTLS）；重试按 attempts 递增退避（每次 5 分钟封顶 30）；维护任务回收停滞 sending 行；`TestMailerClaimsBeforeSending`/`BacksOffAfterFailure`/`RequeuesStaleSending`） | 邮件 worker：无超时的 `smtp.SendMail`（ctx 不传入）在 `InTx` + `FOR UPDATE SKIP LOCKED` 持锁事务内逐封执行——SMTP 挂起即整条 goroutine 与事务无限期卡死；失败重试无退避，25 秒内 5 连败即永久置 failed，验证码/重置邮件「收不到」 | `internal/mailer/mailer.go:60-83,122`、`internal/store/telemetry.go:158-166` |
| S-9 ✅已修（2026-08-14：`!Succeeded` 时退款单标 failed、订单恢复 `paid`（可重试）、审计落盘、事务外返回 `refund_declined`（新 apperr + i18n）；mock 渠道加 `SetRefundDeclined` 测试开关；`TestMockRefundDeclineRecoversTheOrder` 含重试成功路径。遗留：异步渠道退款回调对账需扩展 Provider 接口，另立任务） | 退款：`provider.Refund`（网络调用）在行锁事务内执行；`Succeeded=false` 后订单停 `refunding`、退款单停 pending，`HandleWebhook` 只处理支付通知，再次退款被 `status != OrderPaid` 拒绝——无任何路径推进到 refunded/failed，**永久卡死** | `internal/service/admin.go:799-867` |
| S-10 | TOTP setup 把已启用管理员的 MFA 降级为口令单因子：`AdminSetupTOTP` 无条件 `SetAdminTOTP(..., nil)` 置空 `totp_enabled_at`；被盗会话者可永久降低账号防线；未注册 TOTP 的管理员也永远单因子，README「强制 TOTP」无强制点 | `internal/service/admin.go:253-274,170` |
| S-11 | 未鉴权请求可无限制驱动对 Sub2API 的外呼：`/authorize/context` 无限频，本地校验失败即回源（5s 超时/次），无负缓存、无熔断、HTTP client 无连接数上限——可放大攻击账号中心或堆积 goroutine | `internal/api/api.go:65`、`middleware.go:49-62`、`sub2api/client.go:107-110,129-147` |
| S-12 ✅已修（2026-08-14：`parseEnv` 只接受 dev/prod（大小写不敏感、容忍首尾空格），production/prd/staging 等直接启动失败并给出指引；未显式设置时 `Warnings()` 增加开发默认密钥告警；config_test 四个新用例） | unsafe-by-default 足枪：忘记设 `CCHAVEN_ENV=prod` 时回退 dev——JWT/TOTP/pepper 用仓库内公开固定值、cookie 不带 Secure，等于任何人可伪造 access token，且 `Warnings()` 不覆盖此情形 | `internal/config/config.go:141,226-238` |
| S-13 | 「今日」指标按 UTC 日切（`now.Truncate(24h)`、`::date`、会话时区固定 UTC，全链路一致无混用 bug）：中文运营后台「今日订单/注册/DAU」在东八区早 8 点清零，易误读 | `internal/service/admin.go:324,409`、`internal/store/telemetry.go:27-32`、`internal/db/db.go:33-36` |
| S-14 | 自邀契约缺口：`resolveAttributionSource` 注释承诺「自邀静默降级」但只处理「邀请码不存在」，违反 `ck_referral_no_self` CHECK（23514）时整个开户事务 500。当前正常流程不可达，属注释与实现不符 | `internal/service/auth.go:124-139`、`identity.go:161-167`、`migrations/0001_init.sql:291` |
| S-15 | 分页 total 与 items 为两条独立查询、无同一快照，并发写入时轻微跳动（limit/offset 钳制已在 handler 层做好） | `internal/store/admin.go:179-206`、`orders.go:103-125`、`telemetry.go:236-260` |
| S-16 | （潜在）上游返回空邮箱时，两个无邮箱用户会在 `ux_users_email`（lower(email) 唯一）上相撞，第二个 provisioning 永久 500。取决于 Sub2API 契约是否保证 email 恒非空 | `internal/sub2api/client.go:244-246`、`identity.go:114-124`、`migrations/0001_init.sql:39` |

小项（不计级）：`internal/service/oauth.go:153` 与 `public.go:102` 把 platform 硬编码 `"macos"`（门户 Sub2API 令牌也能调 `/app/heartbeat` 并以 macos 污染 `user_devices`）；`internal/store/admin.go:154` 字段名 `SubscriptionKnd` 拼写错误（未外泄）。

---

## 三、W：web 三站问题（P2）

### 3.1 门户 / 会话 / 账号（portal + @lumio/auth）

| 编号 | 问题 | 位置 |
| --- | --- | --- |
| W-2 | `/auth/me` 网络失败后账户页**永久骨架屏**：失败且非 `AUTH_SESSION_EXPIRED` 时故意保留本地会话、profile 留空，Account 页对 `!profile` 渲染 `LoadingBlock`，无错误提示无重试；fetch 无 AbortController/超时，请求挂起同样表现 | `useSession.tsx:54-62`、`portal/src/pages/Account.tsx:35-36`、`client.ts:86-93` |
| W-3 | 注册协议只能「勾选同意」，**全文无法查看**：只渲染 `《{doc.title}》` 纯文本非链接；`contentMd` 已拉取（`client.ts:17,145-148`）全站从未渲染，`@lumio/ui` 的 Modal 无人使用。合规风险 + 降转化 | `portal/src/pages/Signup.tsx:156-170` |
| W-4 | 密码强度规则只展示不校验：PasswordField 实时打勾「至少 8 字符/字母+数字」但提交前不拦截；`REASON_MAP` 无密码策略 reason，服务端拒绝会落到 `SERVICE_UNAVAILABLE` 显示「服务暂时不可用，请检查网络后重试」——与真实原因（密码太弱）完全不符 | `Signup.tsx:63-75`、`packages/ui/src/components/fields.tsx:41-53,107-124` |
| W-5 | `next` 白名单可被 `/\evil.com` 形式绕过边界：`isAllowedNext` 只挡 `//` 前缀，`/\` 开头被放行归 internal，`navigate()` pushState 解析为跨源抛 SecurityError，被 catch 后显示「服务暂时不可用」——用户已登录却卡在登录页看网络错误文案。距真正开放重定向只差一次 internal 改用 location 跳转的重构；测试未覆盖此形态 | `packages/ui/src/config.ts:83-102`、`portal/src/lib/redirect.ts:11-13` |
| W-6 | 登出与在飞 `/auth/me` 竞态：`signOut` 不触发 effect 重跑（依赖仅 `[nonce]`），页面刚加载时请求在飞、此刻登出，请求稍后成功返回会再次 `setProfile`+`setStatus("authenticated")`——cookie 已清但界面显示已登录，刷新前一直错乱 | `useSession.tsx:47-64,92-102` |
| W-7 | 注册流程 2FA 分支「卡片套卡片」+双标题：外层 `Signup` 始终渲染 `auth-card wide`，challenge 分支又渲染自带 `auth-card` 的 `TwoFactorStep`；Login 页是页面级整卡替换，写法不一致 | `Signup.tsx:17-28,96-107`、`TwoFactorStep.tsx:25` |
| W-8 | 邮箱后缀白名单用 `endsWith` 判断：`user@evilgmail.com`.endsWith(`"gmail.com"`) 为 true，本地拦截失效，依赖服务端 `EMAIL_SUFFIX_NOT_ALLOWED` 兜底（文案正确）。应取 `@` 后域名比对 | `Signup.tsx:63-66` |
| W-9 | 无任何「忘记密码」入口：`passwordResetEnabled` 拉取后无消费者（死字段），Login/Signup 无 reset 链接、路由无相关页面——`password_reset_enabled=true` 的站点用户无路可走 | `client.ts:24,142`、`App.tsx:43-51` |
| W-10 ✅已修（2026-08-14：成功 toast 移入成功分支，clipboard API 缺失或写入失败改提示「复制未成功，请手动选中复制」；authorize.test 成功/失败两用例） | 「授权码已复制」toast 在复制失败时也提示成功：`clipboard?.writeText` 抛错或非安全上下文下为 undefined 被静默跳过后，toast 无条件执行。应移入 try 成功分支 | `portal/src/pages/Authorize.tsx:113-120` |
| W-11 | 指向产品站的链接打开方式不一致：页头 SiteLink `target="_blank"`，页脚/首页/账户页同标签整页跳转，行为不可预期 | `packages/ui/src/components/SiteShell.tsx:51-61` vs `:142-148`、`Home.tsx:79`、`Account.tsx:69` |
| W-12 | portal SEO 元数据缺失 + 无 favicon：`index.html` 只有 title/description，无 og:*/twitter 卡片/canonical/favicon | `web/apps/portal/index.html` |
| W-13 | `status: "loading"` 是永不触发的死分支（初始态只有 anonymous/authenticated），SiteShell/Account 的 loading 语义与注释误导 | `useSession.tsx:25-27`、`SiteShell.tsx:13-14` |
| W-14 | 未知账号状态直接回显英文原值（如 `frozen`）：`STATUS_TONE` 只映射 active/disabled/pending，应兜底「未知状态」 | `Account.tsx:7-11,44-47` |
| W-15 | Authorize 页 token 过期无重新登录引导：本地乐观态仍 authenticated，点「同意授权」反复收 401 文案，页内「去登录」仅在本地 anonymous 时出现，需手动刷新触发重判 | `Authorize.tsx:86-105,182` |
| W-16 | 次要备忘：`EMAIL_VERIFY_NOT_ENABLED` 映射为「两步验证暂时不可用」（与桌面端对齐的既定取舍）；余额 `¥${balance.toFixed(2)}` 无千分位；LogoutButton 无 pending 态可重复点击；页头「登录/注册」不带 `next`（从 `/authorize` 误点丢失回跳目标）；`isAllowedNext` 放行 `http://lumiogame.com` 形态属多余放宽 | `errors.ts:53`、`Account.tsx:83-95`、`App.tsx:36`、`config.ts:94-98` |

### 3.2 产品站（cc / codex）

| 编号 | 问题 | 位置 |
| --- | --- | --- |
| W-18 | CC 套餐价格 ¥68 从「接口下发」退化为硬编码（旧站从 `GET /config/public` 动态读价；控制面 seed 6800 且运营可改，admin_test 演示改 9900）：当前数值一致，但运营改价后营销站不跟随，「站上 ¥68、收银台 ¥99」。golive checklist 未列价格核对项 | `web/apps/cc/src/content.ts:30-33` vs 旧站 `cchaven/apps/web/src/components/PlanCard.tsx:40`、`migrations/0002_seed.sql:5` |
| W-19 | 「立即订阅」按钮打开的是 Sub2API 充值页（`https://api.lumio.games/purchase`），与「包月订阅」文案语义不匹配；FAQ 仍按订阅语义描述。若收银台无对应档位，转化中断 | `web/apps/cc/src/components/PlanCard.tsx:20`、`packages/ui/src/config.ts:53-56` |
| W-20 | 对比度全线低于 WCAG AA：主按钮白字压品牌渐变（`.btn-primary`），实测 brand≈2.6:1、cc 珊瑚≈2.5:1、**codex 青绿最差≈1.6:1**（要求 4.5:1）；同渐变+白字还用于 `.plan .tag`、`.chip`、`.steps3 .num`。次要文字 `--gray-dim`（#6d7488）对近黑底约 4.43:1，临界不达标，用于 12~13px 小字 | `packages/ui/src/styles/components.css:50-54,919-929,1048-1060,1096-1106`、`tokens.css:18,40-42`、`base.css:344,372` |
| W-21 | codex 导航「三步开始」锚点指向 `#top`（hero），steps3 区块没有 id，点击滚回页首与文案不符（`#downloads`、`#faq` 锚点真实存在） | `web/apps/codex/src/App.tsx:15` vs `pages/Home.tsx:45,80` |
| W-22 | cc 站 `/pricing` 页没有 h1（主标题是 h2，Home/Download/NotFound 都有）；全仓无 `document.title` 更新逻辑，`/pricing`、`/download` 标签页/书签/搜索摘要都显示首页标题 | `web/apps/cc/src/pages/Pricing.tsx:18-21`、`index.html:6` |
| W-23 | 两产品站均无 favicon / OG / canonical / robots.txt / sitemap（整个缺失，非引用错误）：`/favicon.ico` 404、分享无卡片、无 canonical/sitemap。portal 同（见 W-12） | `web/apps/cc/index.html`、`web/apps/codex/index.html`（均无 `public/`） |
| W-24 | 移动端无汉堡/收起菜单：窄屏唯一策略是 `.site-header { flex-wrap: wrap }`，375px 下 codex 站头部折成 2 行且 sticky 常驻，吃掉约 100px 视口 | `packages/ui/src/styles/base.css:238-251,387-396` |
| W-25 | 已订阅/试用中用户在 cc 站仍看到「立即订阅」（相比旧站按订阅态切换 CTA 是状态感知退化），付费用户点进去再次落到充值页，产生「是不是没订上」的困惑。cc 站已引 `@lumio/auth`，补订阅态展示成本低 | `web/apps/cc/src/components/PlanCard.tsx` vs 旧站 `PlanCard.tsx` 的 `PlanCTA` |
| W-26 | Intel Mac + Safari/Firefox 用户会被推荐 Apple Silicon 安装包：UA 含 Macintosh 一律默认 `mac-arm`，仅 Chromium 有 `userAgentData` 高熵提示。这是 69f7b94「不信任冻结的 Intel UA」修复的已知副作用，属可接受折衷，可在 Intel 卡加提示缓解 | `web/apps/codex/src/lib/releases.ts:40-47,49-52` |
| W-27 ✅已修（2026-08-14 补记，用户线上报告）：产品站弹窗层级被页头与后续区块盖住——主题给 `main > :not(.aurora)` 统一强加 `position:relative; z-index:1`（压住 Aurora 光晕），`Modal` 未走 portal、内联渲染在 `#downloads` section 内，backdrop 的 z-index:900 被困在该上下文里，sticky 页头（z100）与 DOM 靠后的 FAQ 区块都渲染在弹窗之上。修复：`Modal` 改用 `createPortal` 挂 `document.body`（组件级修复，三站所有内联 Modal 一并受益）；新增 portal 单测 + 真实浏览器验证（backdrop 挂 body、页头/FAQ 被遮罩压暗） | `web/packages/ui/src/components/Modal.tsx`、`web/packages/ui/src/styles/base.css:319-323`、`web/apps/codex/src/components/Downloads.tsx:81-105` |
| W-28 ⏳上游依赖（2026-08-14 补记，用户线上报告）：门户登录后点「充值」跳 `https://api.lumio.games/purchase` 不是登录态（需在 LumioAPI 再登录一次），且 LumioAPI 左上角 logo 无出口回主站、用户两次撞到 404。根因均在上游：LumioAPI 控制台会话存于 `api.lumio.games` 的 localStorage，与 `.lumiogame.com` 的主站 cookie 跨注册域隔离；本仓三处充值入口（`purchaseUrl()` / `payment_url()` / `PurchaseURL()`）契约一致无需改动。改造方案（`/auth/bridge` 令牌交接 + `site_home_url` 可配 logo 跳转 + 404 出口）已成文待 LumioAPI 侧实施：`docs/upstream/lumioapi-portal-integration.md` | `web/packages/ui/src/config.ts:53-56`（本仓侧入口，等上游就绪后加 handoff） |

范围外顺带观察：`codex/CHANGELOG.md` 停在 1.2.22（2026-06-28）而应用已是 1.2.46；cc 站 `VITE_CC_VERSION` 为部署期注入的展示值，与桌面端 `version: "0.1.0"` 无联动校验，配错即展示错版本（上线按 checklist F 核对）。

---

## 四、D：桌面端问题（P2）

| 编号 | 问题 | 位置 |
| --- | --- | --- |
| D-2 ✅已修（2026-08-14：`ProvisioningView` catch 对 `isSessionExpired` 跳过 `onStepFailed`；`state.ts` 的 `provisioning-step-failed` 在 phase 已是 signed-out/authenticating 时保持不变（双保险）；state.test 补两条归并时序用例） | provisioning 中途会话过期：`runCommand` 先同步触发 `session-expired` 再 rethrow，监听器先派发 `session-expired` 再派发 `auth-step-changed: login`，随后 `provisioning-step-failed` 无条件把 phase 设回 `provisioning`——覆盖已设置的 `signed-out`/`authenticating`。用户看到的不是登录页而是 provisioning 失败态；点「重试」每次再过期，重复整个循环。测试只测了 session-expired 在后的顺序 | `invoke.ts:141-148`、`LumioApp.tsx:167-178`、`ProvisioningView.tsx:63-65`、`state.ts:284-298` |
| D-3 ✅已修（2026-08-14：状态机新增 `codex-app-changed` 事件——任何 phase 都更新 `codexApp`，ready 阶段同步重算 `canLaunch`/提示；`LumioApp.onCodexAppChanged` 去掉仅在线守卫。遗留：Rust 侧手选仍只存进程内存，重启丢失，需持久化另立任务） | 离线/登出阶段手动选择官方应用被丢弃：`onCodexAppChanged` 守卫 `account === null \|\| phase !== "ready-online"` 直接 return——`ready-offline`/`signed-out` 下自动检测失败时用户唯一的补救无效，首页持续「未检测到官方应用」、`canLaunch` 恒 false，离线兜底承诺失效；重连携带的仍是旧 null。且 Rust 侧手选只存进程内存，重启即丢 | `LumioApp.tsx:246-256,227-235`、`SettingsView.tsx:94-100`、`lumio_commands.rs:173,264-266,569-580` |
| D-4 | 无 React 错误边界 + `csp: null`：任何渲染期异常（如 IPC payload 契约破裂）= 整窗白屏；关窗默认隐藏到托盘，白屏后用户难自救 | `src/main.tsx:7`、`src-tauri/tauri.conf.json:24-26`、`src-tauri/src/lib.rs:126-139` |
| D-5 | 退出登录可被服务端请求阻塞最长约 25 秒且无 pending 反馈：`lumio_logout` 先 await（超时 20s+连接 5s），`finally` 才派发 `signed-out`，退出按钮无 loading，用户反复点击 | `lumio_commands.rs:392-403`、`api.rs:10-11`、`LumioApp.tsx:213-217`、`SettingsView.tsx:269` |
| D-6 | provisioning 失败文案引导去「修复页」，但该阶段导航被锁（`navLocked = !ready && phase !== "signed-out"`），修复页不可达，误导性提示 | `ProvisioningView.tsx:145-149` vs `LumioApp.tsx:263` |
| D-7 | 设置页「恢复本机配置」成功后既不登出也不重估状态（只 toast）：首页仍显示在线/已配置、启动按钮可用，但磁盘 config/auth 已回滚、接管记录已删除；修复页同操作会登出，行为不一致 | `SettingsView.tsx:140-147` vs `RepairView.tsx:58-69` |
| D-8 | 登录/注册在途切换表单的竞态：`submitting` 只禁输入不禁「创建账户」链接，迟到成功响应经 `applyResult` 直接把用户拽进 provisioning，填一半的注册表单丢失（RegisterView 同理） | `LoginView.tsx:44-51,157-161`、`RegisterView.tsx:366-370` |
| D-9 | 上次同步时间只显示时:分（无日期段），应用常驻托盘数天后「使用 09:15 的本机缓存」看起来像今天早上 | `HomeView.tsx:40-45` |
| D-10 | 登录后 provisioning 路径缺接管冲突复查（`planStartup` 只在启动时查）：同会话登出→外部改 `~/.codex`→再登录会直接覆盖外部改动（快照可恢复，但违背「冲突必须进修复页」承诺）——已在 `.spec/knowledge/features/lumio-account-and-home.md:36` 记录为已知缺口 | `LumioApp.tsx:74-102` |
| D-11 | 两处呈现失真：① `account_payload` 恒置 `plan_label: None`，首页永远显示「当前没有生效套餐」，即使服务端有（`AccountProfile` 无 plan 字段，文案做了无数据支撑的断言）；② 协议正文 `contentMd` 以纯文本渲染在 `<p>`，用户看到 `#` 等原始 Markdown 符号 | `lumio_commands.rs:237-243`、`RegisterView.tsx:345-347` |
| D-12 ✅已修（2026-08-14 补记，用户线上报告「启动检查」页报 UNKNOWN）：修复页错误码被服务探活轮询洗掉——启动编排判冲突进 `needs-repair` 时直接返回（不加载 settings），`serviceAvailable` 仍为 false，探活 effect（`LumioApp.tsx:143-164`）随即成功派发 `service-settings-loaded`，其 reducer 分支把 `errorCode` 清 null，phase 却停在 needs-repair——用户看到的不是 `CODEX_CONFIG_CONFLICT` + 冲突文案，而是 UNKNOWN + 「出现未知问题」兜底文案，误判为未知故障。修复：`needs-repair` 阶段 `service-settings-loaded`/`service-unavailable` 不再触碰 `errorCode`（服务可用性归 `serviceAvailable`）；state.test 两条新用例（先红后绿）+ 重嵌 dist 的真机验证（胶囊正确显示 CODEX_CONFIG_CONFLICT 与专属文案） | `state.ts:212-243`、`LumioApp.tsx:143-164`、`views/RepairView.tsx:21-25` |
| D-13 ⏳设计缺口（2026-08-14 补记，与 D-12 同场景发现）：全文件 sha256 冲突校验对官方 Codex 的正常写回必然误报——官方应用启动即重写 `config.toml`（实测：接管完成 24 秒后官方 Codex 更新 `last_updated` 与 `model_reasoning_effort`），下次启动管理器必进冲突修复页；「用管理器启动官方 Codex」这一核心流程自带触发器。修复方向：把完整性校验从「整文件哈希」收敛到 Lumio 拥有的字段（`model`/`model_provider`/`model_providers.lumio` + `auth.json` 的 `OPENAI_API_KEY`），解析后逐字段比对；涉及接管契约与快照语义，需单独设计评审 | `config_takeover.rs:183-221`（check_takeover）、`product.rs` |
| D-14 ✅已修（2026-08-14 补记，用户线上报告「启动 Codex 还是要登录」）：接管后官方 Codex（本机为 ChatGPT.app 的 Codex 功能 / codex CLI）启动报「我们无法加载您的账号信息」并要求登录——`auth.json` 残留的 `auth_mode:"chatgpt"` + 过期 tokens（接管前用户用 ChatGPT 账号登录过，令牌 8 天前过期）让官方端无视 Lumio 写入的 `OPENAI_API_KEY`（官方端按 auth_mode 选凭据）。修复：`render_auth` 在写入 key 的同时把 `auth_mode` 置为 `"apikey"`；旧值与 tokens 属快照管辖，restore 原样还原（用户原 ChatGPT 登录可完整退回）。测试：`takeover_switches_auth_mode_to_apikey_so_official_codex_uses_the_key`（先红后绿，含 restore 还原断言）；本机实测 `codex login status` → `Logged in using an API key` | `config_takeover.rs:291-311`（render_auth） |
| D-15 ✅已修（2026-08-15 补记，用户线上报告发消息报 `Missing environment variable: 'OPENAI_API_KEY'`）：provider 配置写了 `env_key = "OPENAI_API_KEY"`，官方 Codex 对自定义 provider 只从**环境变量**取 key、不走 auth.json——GUI 启动无此变量，任何任务必报错。实测（codex-cli 0.146）：带 env_key 无环境变量 → 报错；去掉 env_key → 走 auth.json 的 key 正常出话。修复：`render_config` 不再写 env_key，历史接管残留的该字段在重复接管时移除；key 唯一落点为 auth.json。测试：`takeover_does_not_pin_the_provider_to_an_environment_variable`（先红后绿） | `config_takeover.rs:280-290`（render_config） |

---

## 五、检查过、确认无问题的方面

- **开放重定向主路径**：三站 `next` 白名单 + 桌面回调白名单（`127.0.0.1.evil.com`、`javascript:`、非 `/callback`、非 http scheme 均拒），授权码/state 不影响判定，`redirect_to` 二次校验后才跳转。
- **错误信封与文案**：非 2xx（含无限流的 429）、`code!==0`、空 body 均覆盖；服务端原文/reqwest Display 不越层外泄，前端折叠为稳定码→中文文案，与桌面端 `errors.rs` 映射逐条一致。
- **事务与幂等**（服务端）：订阅变更事务+行锁；`subscription_events` 部分唯一索引 + savepoint 幂等（试用一生一次、订单/邀请只结算一次）；授权码单条 UPDATE 原子消费；订单号 UPSERT RETURNING 无重号；refresh 轮换 + 重放检测撤族；开户竞态正确处理 23505。
- **IDOR**：`GetMyOrder`/`RevokeSession`/`ListSessions`/`Heartbeat`/`ReferralOverviewFor` 全部按 `principal.User.ID` 过滤，未发现跨用户访问。
- **admin 能力矩阵**：默认拒绝、逐格单测；敏感读写有 `Can*` 检查 + 双路径审计；Sub2API fail-closed（上游不可用 503 且与 401 区分，有测试锁死）。
- **令牌存储与续期并发**（桌面）：owner-only 0600 文件 + 原子写（rename+fsync）；轮转落盘失败同清内存与磁盘；无 localStorage 存秘密；renewal 单飞 + 门内重读防旧 refresh 复用，有 wiremock 测试。
- **重复提交防护**：web 全表单 busy + fieldset disabled；桌面注册/登录/验证码有 submitting/countdown，provisioning 重试不重复建 Key。
- **配置接管**（桌面）：快照只记一次、损坏 manifest 报冲突、restore 保留无关 auth 字段、写入前解析失败不动用户文件，测试矩阵完整。
- **Tauri IPC 输入校验**：`open_in_browser` 仅 http/https；provision step 白名单；手选路径需存在且 `*.app`；命令面有 allowlist 契约测试。
- **可达性与动效**：skip-link 指向真实 `#main`；全局 `:focus-visible`；Modal 焦点圈定/Esc/归还焦点；FAQ `aria-expanded`；`prefers-reduced-motion` 双层保障（全局 kill-switch + Reveal），reduced-motion 下终态仍可见。
- **构建/资产/域名**：域名单点收敛于 `config.ts` 且与 ops 文档一致，无 localhost/旧域名残留；下载链三级回退与空态有实现和测试；vite 无自定义 base，无生产 404 风险；`dist/` 已 gitignore。
- **日志卫生**：全 slog 调用点核查无 token/口令/TOTP 种子外泄，邮箱打码；桌面诊断日志经 `redact()` 脱敏。
- **基线**：`go vet ./...`、`go test ./...` 全绿；migrations 与代码列一致无漂移。

---

## 六、建议处理顺序

1. **小改大患，当天可修**：D-1 邀请码参数接线（约 1 行 + settings 下发）；S-1 TOTP 限频（挂现成中间件 + 失败锁定）；S-12 `CCHAVEN_ENV` 非 prod 时启动告警；W-10 复制 toast 移入成功分支。
2. **本周**：W-1 web refresh 闭环（含多标签页轮转竞态考虑）；S-2 CC 会话回查 Sub2API；S-5 支付金额校验；S-8 邮件 worker 事务外置 + 超时 + 退避；S-9 退款终结路径；D-2/D-3 桌面状态机两处死锁态。
3. **上线前**：W-17 深链按钮隐藏；W-3 协议 Modal；W-23/W-12 favicon/OG/sitemap；W-18 改价核对进 golive checklist；D-4 错误边界 + CSP。
4. **排期优化**：W-20 对比度；W-2 骨架屏兜底+重试；W-24 移动端菜单；W-25 订阅态 CTA；S-13 日切时区；其余 P2 按模块归并处理。

---

## 七、2026-08-14 修复批注附录（超出原条目的附带处理）

本节如实记录修复过程中发现、且已一并处理的**预先存在问题**（均经 stash 基线对照确认与本批改动无关）：

1. **基线声明更正**：本报告开头「`go test ./...` 全绿」在本机（commit 69f7b94）不成立，`./test/` 有 6 个预先存在的失败，分三类：
   - **metrics SQL 类型推断**：`store/metrics.go` 四处 `$1 - make_interval(...)` 在本机 PG 下 `$1` 被推断为 interval，`timestamptz >= interval` 报 500（`TestAdminMetricsOverview`、`TestAdminDistributions` 失败）。已加显式 `$1::timestamptz` cast 修复，对其他 PG 版本亦无副作用。
   - **refresh 重放撤销被回滚**（真实缺陷，原报告第五节「重放检测撤族」的结论不成立）：`RefreshSession` 重放分支在 `InTx` 闭包内撤销会话族后返回 error，pgx 回滚吞掉撤销——401 响应正确但撤族从未落盘、新令牌仍可续用。已改为撤销落盘后事务外返回错误（见 S-2 批注）。
   - **过时测试断言**：本地登录端点按设计已收口为 410，但 4 个测试仍按旧契约断言（`TestAdminSupportCannotWrite` 的「仍能登录」、`TestAdminIsSeparateFromUserAccounts` 的 401、`TestAdminDisableUserLogsOutImmediately` 的重登录段、`TestAdminUserDetailReturnsPlainEmailAndSnapshots` 的 trialing——邀请奖励按 `KindPaid` 入账会升级订阅状态）。已按现行设计改写断言，意图不变。
2. 修复后四套收口验证全绿：`go vet ./...` + `go test ./...`、`cargo fmt --all -- --check` + `cargo test -p codex-plus-core -p codex-plus-manager`、桌面前端 `npm test`（129/129）+ `npm run check`、web 工作区 `npm test` + `npm run check`。

---

*本报告由 QA 审查产出，问题修复后请在对应条目标注状态（已修/已豁免/ wontfix 及原因），不建议删除历史条目。*
