# 05 · Sub2API 邀请返利（affiliate）契约

返利全链路（归因绑定 → 返利计提 → 冻结解冻 → 主动划转）都在 Sub2API
（`https://api.lumio.games`，fork [Go1c/sub2api](https://github.com/Go1c/sub2api) 的 `publish` 分支线）。
本文是「邀请返利活动」下游两卡（门户归因接线、门户返利视图）的**唯一业务真值**：
注册字段、用户侧端点、错误码、规则配置项与生产当前值。

核对基准：2026-08-16 生产实测（curl）+ fork `origin/publish`（合并基线 `643d36fbd`）源码；
上游对照 `Wei-Shaw/sub2api` `main`（`baeac1f3d`）。生产 `GET /api/v1/settings/public`
返回 `version: "publish"`（构建管线把分支名注入 `main.Version`，Dockerfile:91-104）。

## 0. G-R1 三阈值（下游开工门槛）

| # | 阈值 | 结论 | 证据 |
| --- | --- | --- | --- |
| ① | 注册接口可携带 aff 绑定字段 | **通过** | `aff_code` / `aff_fingerprint` 字段实测被接受；生产已有 2 条真实绑定成功记录（2026-04-26，§3 日志实测） |
| ② | 用户侧查询 + 划转 API 可用 | **通过** | `GET /user/aff`、`GET /user/aff/logs` 实测 200；`POST /user/aff/transfer` 实测可达（零额度时按预期返回 `AFFILIATE_QUOTA_EMPTY`） |
| ③ | affiliate 总开关已启用 | **通过** | `GET /settings/public` 实测 `affiliate_enabled: true` |

## 1. 生产部署与上游 main 的实现差异

生产**不是**上游 main：public settings 含 fork 独有字段 `invitation_registration_mode`（上游全库无此键），
`GET /user/aff` 返回 fork 独有字段（`affiliate_tiers` / `effective_rebate_rate_percent`），
`GET /user/aff/logs` 在上游不存在路由而生产返回 200。局限：无法从公网确定确切 commit，
如需精确到 commit 须管理端查构建信息（未执行，无管理凭据）。

affiliate 相关差异（fork `publish` vs 上游 `main`）：

| 能力 | 上游 main | 生产 fork publish |
| --- | --- | --- |
| 注册 `aff_code` 字段 + 绑定吞错 | 有 | 有（行为一致） |
| 返利比例解析 | 专属比例 → **全局扁平比例**（`affiliate_rebate_rate`） | 专属比例 → **L1-L4 阶梯**（`affiliate_rebate_tiers`）；扁平比例键仍存在但计提路径无消费者 |
| 阶梯返利 `affiliate_tiers.go` | 无 | 有（L1-L4，双维度：邀请人数 + 被邀充值总额） |
| 注册赠送（signup bonus） | 无 | 有（固定额度 + 单邀请人累计上限 + 全站日上限 + 设备指纹防刷） |
| `aff_fingerprint` 注册指纹 | 无 | 有（随注册传入，落邀请日志，防注册赠送薅羊毛） |
| `invitation_registration_mode` | 无 | 有（`redeem_code` / `affiliate_link` / `both`） |
| 用户侧邀请日志端点 | 无 | `GET /api/v1/user/aff/logs` |
| 管理端 affiliate 路由 | 有（invites/rebates/transfers/users） | 同左 + `invite-logs` + 用户级码/比例管理 |
| 被邀列表 100 条 | 有 | 有——**只是展示条数上限，不是绑定数上限**（两版都无绑定上限；修正蓝图） |

源码位置（除注明外均为 fork `publish`）：

- 注册 DTO：`backend/internal/handler/auth_handler.go:50`（上游同字段在 `:59`）
- 绑定：`backend/internal/service/affiliate_service.go:415`（`BindInviterByCode`）
- 阶梯：`backend/internal/service/affiliate_tiers.go:12-17`
- 比例解析差异：fork `affiliate_service.go:641` vs 上游 `affiliate_service.go:391-400`
- 设置键与默认值：`backend/internal/service/domain_constants.go:27-39,154-164`

## 2. 注册接口：`POST /api/v1/auth/register`

### 2.1 请求字段

| 字段 | 必填 | 说明 | 生产现状 |
| --- | --- | --- | --- |
| `email` | 是 | 注册邮箱 | 后缀白名单 16 个（`@qq.com`…，见 public settings `registration_email_suffix_whitelist`） |
| `password` | 是 | ≥6 位 | — |
| `verify_code` | 条件 | 邮箱验证码；`email_verify_enabled=true` 时必填 | **必填**（实测缺省报 `EMAIL_VERIFY_REQUIRED`） |
| `turnstile_token` | 条件 | 人机校验 | `turnstile_enabled=false`，留空 |
| `promo_code` | 否 | 优惠码 | `promo_code_enabled=false`，忽略 |
| `invitation_code` | 否 | 注册**门槛码**（兑换码，一次性） | `invitation_code_enabled=false`，**完全忽略**（见 §5） |
| `aff_code` | 否 | 邀请**归因码**；`A-Z0-9_-`，4–32 位，输入自动大写化 | 生效中（§3 实测有成功绑定） |
| `aff_fingerprint` | 否 | 设备指纹（fork 独有），供注册赠送防刷，不传也能绑定 | 可选 |

成功响应与登录一致：`data = {access_token, refresh_token, expires_in, token_type:"Bearer", user}`。

### 2.2 aff_code 绑定行为矩阵（`BindInviterByCode`，注册层吞错）

**任何 aff_code 问题都不会让注册失败**（前提=生产现状门槛关闭，见 §5）——服务层返回的错误在
`auth_service.go` 注册流程里只记日志（上游同样吞错；「非法码报错」只是服务层返回值，
不是 API 行为）。若将来开启门槛且 mode 含 `affiliate_link`，非法码会以
`INVITATION_CODE_INVALID` 拒绝注册（§5）。绑定成功与否进邀请日志，管理端与用户侧 `/aff/logs` 可见：

| 场景 | 注册 | 绑定 | 日志 failure_reason |
| --- | --- | --- | --- |
| 空码 / 纯空白 | 成功 | 不绑定，静默 | （无日志） |
| 总开关关闭 | 成功 | 不绑定 | `affiliate_disabled` |
| 格式非法（长度/字符集） | 成功 | 不绑定 | `invalid_code` |
| 码不存在 | 成功 | 不绑定 | `invalid_code` |
| 自己邀请自己 | 成功 | 不绑定 | `self_invite` |
| 该账号已有邀请人 | 成功 | 不重复绑定（幂等） | `already_bound` |
| 有效码 | 成功 | 绑定，邀请人 `aff_count+1`，触发注册赠送（若启用） | `success:true` |

注册赠送的失败不回滚绑定，只少送钱：`fingerprint_reused` / `inviter_total_cap_reached` /
`daily_total_cap_reached` / `cap_reached`（文案见 `affiliate_service.go` `affiliateInviteFailureMessage`）。

OAuth 注册同样收 `aff_code`：`/auth/oauth/*/complete-registration` 系列
（`auth_email_oauth.go:331`、`auth_dingtalk_oauth.go:690` 等）。生产仅微信 OAuth 开启。

限流：注册 5 次/分、`send-verify-code` 5 次/分、`validate-invitation-code` 10 次/分（`routes/auth.go:35-58`；
限流响应不套信封，见 §3）。

## 3. 用户侧端点全表

鉴权统一 `Authorization: Bearer <access_token>`（与 `/auth/me` 同一令牌）。
信封：成功 `{code:0, message:"success", data}`；业务错误 HTTP 4xx/5xx + `{code, message, reason}`
（reason 是稳定错误码）；**中间件错误（401 未带令牌、429 限流）不套信封**，
形如 `{"code":"UNAUTHORIZED","message":...}`——门户分支处理时不能只依赖信封。

| 端点 | 方法 | 请求 | data 响应 | 实测 |
| --- | --- | --- | --- | --- |
| `/api/v1/user/aff` | GET | — | `AffiliateDetail`（下表） | 200 ✅ |
| `/api/v1/user/aff/logs` | GET | `page`（默认 1）、`page_size`（默认 20，≤100，别名 `limit`） | 分页 `{items,total,page,page_size,pages}`，items 为脱敏邀请日志（下表） | 200 ✅ |
| `/api/v1/user/aff/transfer` | POST | 无请求体 | `{transferred_quota, balance}`；无可划额度时报 400 `AFFILIATE_QUOTA_EMPTY` | 400（零额度，符合预期）✅ |
| `/api/v1/auth/validate-invitation-code` | POST | `{code}` | `{valid, error_code}`；门槛关闭时 `{valid:false, error_code:"INVITATION_CODE_DISABLED"}` | 200 ✅ |

`AffiliateDetail`（`affiliate_service.go:116`，2026-08-16 实测样例值来自 user_id=2）：

| 字段 | 类型 | 说明（实测值） |
| --- | --- | --- |
| `user_id` | int | 本人 |
| `aff_code` | string | 本人邀请码（`39XZR7KLHECZ`，12 位大写） |
| `inviter_id` | int? | 本人绑定过的邀请人；无则省略 |
| `aff_count` | int | 有效被邀人数（2） |
| `aff_quota` / `aff_frozen_quota` / `aff_history_quota` | float | 可划转 / 冻结中 / 历史累计（均为 0） |
| `invitee_recharge_total` | float | 全部被邀人充值总额（0） |
| `effective_rebate_rate_percent` | float? | 当前实际生效比例（1 = L1） |
| `affiliate_tiers` | array | L1-L4 阶梯配置（见 §4 生产值） |
| `current_affiliate_tier` / `next_affiliate_tier` | object? | 当前/下一档 |
| `invitees` | array | 被邀人列表，**≤100 条**（展示上限），每项 `{user_id, email(脱敏), username, created_at, total_rebate}` |

邀请日志 item（用户侧已抹掉 `fingerprint_hash`/`ip_address`/`user_agent`，邮箱脱敏）：
`{id, inviter_id?, inviter_email?(脱敏), inviter_username?, invitee_id?, invitee_email?(脱敏),
invitee_username?, affiliate_code, success, failure_reason?, failure_message?, bonus_amount, created_at}`。

**邮箱脱敏规则**（`maskEmail`，`affiliate_service.go:785`）：取本地部分与域名首字符，
其余以 `***` 代替，保留 TLD——`someone@icloud.com` → `s***@i***.com`。

**额度状态机**：返利计提先入 `aff_frozen_quota`（冻结小时数 > 0 时；=0 直接入 `aff_quota`）；
读取详情/划转时惰性解冻到期额度 → `aff_quota`；`POST /aff/transfer` 一次性全额划入
`users.balance`（事务内解冻→清零→加余额，`affiliate_repo.go:477`）。返利在被邀人**充值订单完成时**
计提（`payment_fulfillment.go:618`，基数为订单金额）。

## 4. 规则配置项与生产当前值

设置键（管理端 `PUT /api/v1/admin/settings` 下发；默认值 `domain_constants.go:27-39`）：

| 设置键 | 含义 | 默认（范围） | 生产当前值 | 证据 |
| --- | --- | --- | --- | --- |
| `affiliate_enabled` | 总开关 | false | **true** | settings/public 实测 |
| `affiliate_rebate_tiers` | L1-L4 阶梯（JSON） | 空（不配比例=不计提） | **已配置**：L1 `0人/0元→1%`；L2 `2人+100元→3%`；L3 `5人+500元→5%`；L4 `20人+2000元→10%`（人数与充值两维度**同时满足**才进档） | `/user/aff.affiliate_tiers` 实测 |
| `affiliate_rebate_rate` | 全局扁平比例 | 20.0（0–100） | **未见**（运营截图未含该项）；fork 计提路径不消费此键，可不再追溯 | `setting_features.go:174` 仅暴露无调用方 |
| `affiliate_rebate_freeze_hours` | 返利冻结小时 | 0=不冻结（≤720） | **0**（不冻结，返利直接入 `aff_quota`） | 管理后台截图 2026-08-16 |
| `affiliate_rebate_duration_days` | 返利有效期（天） | 0=永久（≤3650） | **60** | 管理后台截图 2026-08-16 |
| `affiliate_rebate_per_invitee_cap` | 单被邀人返利上限 | 0=无上限 | **100**（超出部分截断，不产生返利） | 管理后台截图 2026-08-16 |
| `affiliate_signup_bonus_enabled/_amount/_total_cap/_daily_cap` | 注册赠送 | false / 0 | **开启 / 0.99 / 单邀请人累计 9.99 / 全站每日 100** | 管理后台截图 2026-08-16；4 月历史日志为 1 元/笔，后被调低 |
| `invitation_code_enabled` | 注册门槛码开关 | false | **false** | settings/public 实测 |
| `invitation_registration_mode` | 门槛校验方式 | `redeem_code`（枚举 `redeem_code`/`affiliate_link`/`both`） | **affiliate_link** | settings/public 实测 |

比例解析优先级（fork）：被邀充值总额（阶梯判定时含本笔）→ 邀请人**专属比例**
（管理端按用户设置 `aff_rebate_rate_percent`，覆盖阶梯）→ 命中阶梯的比例；无任何可用比例则不计提。
除扁平比例「未见」外，其余数值已于 2026-08-16 由运营从管理后台截图回填；后台再改配置，
须同步更新本表并注明日期。

管理端面（需 admin 鉴权，本次未测）：`GET /api/v1/admin/affiliates/{invite-logs,invites,rebates,transfers}`、
`GET/PUT/DELETE /api/v1/admin/affiliates/users[...]`（改邀请码、设专属比例、批量设比例）、
`POST /api/v1/admin/affiliates/users/batch-rate`（`routes/admin.go:759`）。

## 5. `invitation_code`（门槛码）与 `aff_code`（归因码）并存

两个**独立字段、独立机制**，互不挤占：

- `invitation_code` = 一次性**兑换码**（type=invitation），门槛开着时校验并标记已用；
  `aff_code` = 邀请关系**归因绑定**，可重复被不同人使用，绑定不可改。
- **生产现状（门槛关）**：`invitation_code_enabled=false` → `invitation_code` 被完全忽略
  （既不校验也不当 aff 码用）；`aff_code` 独立生效。`validate-invitation-code` 端点固定回
  `INVITATION_CODE_DISABLED`（实测）。
- 若将来开门槛：mode=`affiliate_link` 时 aff 码即可作门槛凭证，且 `invitation_code` 字段的值
  也会作为 aff 码候选参与校验（先 `aff_code` 后 `invitation_code`）；mode=`redeem_code` 只认兑换码；
  `both` 两者皆可。门槛开启后「两码皆空」会拒注册（`INVITATION_CODE_REQUIRED`）。
  逻辑见 `auth_service.go:173`（`validateRegistrationInvitation`）。
- **建议**：门户活动链接只发 `aff_code`，不碰 `invitation_code`，与生产现状零冲突。

## 6. 对下游两卡的建议

**门户归因接线**：
- 邀请链接形如 `https://lumiogame.com/register?aff=<code>`，注册提交时原样放进 `aff_code`
  （服务端自动大写化，门户无需规范化）；`aff_fingerprint` 可选，不传不影响绑定。
- aff 码**永不阻断注册**、前端拿不到绑定成败回执——注册成功后若需确认归因，
  用 `GET /user/aff` 的 `inviter_id` 是否非空兜底展示（可选，勿做成硬流程）。
- 邮箱验证码仍是注册必经环节（`send-verify-code` → `verify_code`），与 aff 无关。

**门户返利视图**：
- 数据源就用 §3 三端点；列表页脱敏邮箱按服务端返回直接渲染（勿自行再脱敏）。
- 「划转到余额」按钮调 `POST /aff/transfer`，把 `AFFILIATE_QUOTA_EMPTY` 映射为
  「暂无可划转额度」；成功后以响应里的 `balance` 刷新余额显示。
- 展示比例用 `effective_rebate_rate_percent` + `affiliate_tiers`（阶梯进阶条件都在响应里）。
- 冻结中额度（`aff_frozen_quota`）解释文案的**具体冻结时长**取决于管理端配置（§4），
  上线前须运营回填，不得写死。当前冻结期配置为 0，`aff_frozen_quota` 恒为 0——
  冻结区块做成条件渲染（值 > 0 才显示），后台改配置即自动生效，前端无需改代码。

**给产品/运营的动作项**（不替产品定数值，仅列待办）：
1. ~~管理后台核对并回填 §4「未获取」项~~ **已完成（2026-08-16 截图回填）**；扁平比例管理界面未见、
   fork 计提不消费，如无历史遗留诉求可关闭追溯。
2. 决定阶梯/注册赠送数值是否沿用现状（阶梯 L1-L4 = 1/3/5/10%；注册赠送 0.99 元/笔、
   单邀请人累计上限 9.99、全站日上限 100；冻结 0；有效期 60 天；单人返利上限 100 元）。
3. 确认是否需要开注册门槛（现状关，活动链接纯 aff 模式即可跑通）。

## 7. 复现命令（无凭据部分，2026-08-16 实测留存）

```bash
B=https://api.lumio.games
curl -sS "$B/api/v1/settings/public"            # affiliate_enabled / invitation_* / version
curl -sS -X POST -H 'Content-Type: application/json' -d '{}' "$B/api/v1/auth/register"
# → 400 RegisterRequest 校验错（字段缺失）
curl -sS -X POST -H 'Content-Type: application/json' \
  -d '{"email":"x@gmail.com","password":"xxxxxx","aff_code":"ZZZZZZ9"}' "$B/api/v1/auth/register"
# → 400 EMAIL_VERIFY_REQUIRED（aff 码不阻断，卡在邮箱验证门槛）
curl -sS -o /dev/null -w '%{http_code}\n' "$B/api/v1/user/aff"   # 401（需 Bearer）
```

登录态实测（token 经环境变量携带，勿入库）：`GET /user/aff`、`GET /user/aff/logs?page=1&page_size=10`、
`POST /user/aff/transfer`。实测账号为运营提供的测试号（user_id=2，邮箱脱敏 `1***@q***.com`），
凭证未写入本文档。

## 8. 维护

后台改 affiliate 配置（开关、阶梯、冻结期、赠送）→ 运营在 §4 回填新值并注明日期；
后台升级 fork 版本 → 复查 §1 差异表是否仍成立。上游同步 merge 时重点看
`affiliate_service.go` / `auth_service.go` 注册路径与 `invitation_registration_mode` 的合并冲突
（fork 在这三处有定制）。Sub2API 通用信封与 CORS 契约见
[03-service-prerequisites.md](./03-service-prerequisites.md)。
