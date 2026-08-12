# M2 官网实现中发现的规范矛盾与缺口

> 原则：**不改规范**（`docs/design/interaction-design.md` 与 `design/prototype/` 均为只读参考）。
> 这里逐条记录矛盾点、本次的处理方式与建议，供评审决定是否回写规范。
> 编号与 `services/cchaven-control/docs/m1-spec.md` 第 4 节的缺口清单独立。

---

## 1. 验证码过期文案两处不一致

- **矛盾**：4.6 节表格写「该验证码已过期。」，6.2 节固定文案表写「该验证码已过期，请重新发送。」。
- **处理**：以 6.2 节为准（该节明确声明「安全语义固定，不得随意改写」），且服务端
  `internal/i18n` 的 `MsgCodeExpired` 也是这一句。前端直接展示服务端 `message`，不本地拼装。
- **建议**：把 4.6 节的短句改为引用 6.2 节，避免两处维护。

## 2. 限频文案：模板 vs 写死的「一分钟」

- **矛盾**：6.2 节是模板「尝试次数过多，请 {n} {unit}后再试。」，4.5 节注册页写死「尝试次数过多，请一分钟后再试。」。
- **处理**：一律展示服务端 `message`。服务端对注册场景填 `1 分钟`，视觉结果与 4.5 一致
  （与 m1-spec 缺口 2 的结论相同）。
- **建议**：4.5 节改为「展示服务端下发的限频文案」。

## 3. 邀请归因 cookie 是 HttpOnly，前端读不到 —— **已解决**

- **矛盾**：4.1 节要求首页邀请横幅「有邀请码 cookie 时高亮显示」，但服务端下发的 `cch_ref`
  是 HttpOnly（`internal/api/middleware.go: setReferralCookies`），JS 无法读取。
- **原始处理（已废弃，保留记录）**：`/i/{code}` 落地成功（`valid:true`）时，前端在 localStorage
  存一份**纯展示用**的邀请提示（邀请人昵称 + 邀请码 + 30 天有效期），首页与注册页据此显示/高亮横幅。
  当时的判断是「归因判定完全由服务端 cookie 决定，localStorage 被清掉只影响横幅，不影响试用发放」。
- **为什么废弃**：那份副本不会随 30 天 cookie 过期而失效，也不会因为邀请码被停用而失效。
  结果是首页继续高亮「注册并登录 APP 即享首月免费试用」，而用户注册后实际拿不到——
  归因始终以服务端 cookie 为准。这是用户可见的**错误承诺**，比「横幅不高亮」严重得多。
- **解法（现行）**：后端补了 `GET /api/v1/invites/current`（m1-spec §2.5 / 缺口 13）：公开只读，
  服务端读 `cch_ref` 裁决，未归因 / cookie 过期 / 邀请码停用一律 `{attributed:false}`，
  有效时返回 `{attributed:true, inviter, trial_days}`，字段口径与 `/invites/{code}` 一致；
  该接口不下发 cookie、也不记 `referral_visits`。前端已删除 `src/state/inviteHint.ts`，
  改由 `src/state/inviteAttribution.tsx` 在一次页面会话内请求一次并共享结果（不轮询）；
  归因未确定或请求失败时**不渲染**邀请横幅，也不弹错误条（营销元素不该打断注册）。
  落地页 `/i/{code}` 行为不变（那里要记访问事件），拿到的同口径响应就地写入共享状态，
  同一次会话跳注册页时无需重复请求。防回归测试断言应用代码不再有任何 localStorage 读写。
- **保留的差异**：首页横幅在 `attributed:false` 时仍渲染 4.1 第 3 块要求的**常驻文案**
  （「获朋友邀请？…」，条件句，不构成承诺），只是不高亮、不出现邀请人名字；
  注册页横幅（4.5）只在 `attributed:true` 时整块出现。若产品希望首页在未归因时彻底不渲染该块，
  改一行即可，但那会与 4.1 的四块布局相左。

## 4. `/authorize` 授权页原型缺失

- **缺口**：原型未实现该页（m1-spec 缺口 5 亦记录）。
- **处理**：按 3.4 与 5.1 自行设计并实现，覆盖：请求方与权限清单（来自
  `GET /oauth/authorize/context` 的 `client_name` / `scopes`）、未登录先登录再回来
  （`/login?next=…`，只接受站内相对路径以防开放重定向）、授权成功跳 `redirect_to`
  并把 `code` 显示出来作为「手动粘贴授权码」兜底（对应 APP 侧 5.1 超时态）、
  参数非法时的不可继续态、取消态。
- **建议**：评审这版设计后补进规范第 4 章（官网逐页）。

## 5. 订阅状态有第四种：`expired`

- **矛盾**：5.6 节徽标只列「已订阅 / 免费试用中 / 未订阅」三种，但
  `domain.Entitlement.status` 还有 `expired`（付费到期未续费）。
- **处理**：新增「订阅已过期」徽标（橙色 + 文字），CTA 与未订阅一致（引导续费）。
- **建议**：规范补第四态。

## 6. 「已成功邀请 {n} 人」与进度列表长度会不一致

- **现象**：`GET /me/referrals` 的 `invited_count` 是**已激活**（走完三步闭环）的人数，
  而 `items` 同时包含 `registered`（已注册未登录 APP）。5.6 节没有说明二者关系。
- **处理**：汇总行用 `invited_count`（口径＝已成功邀请），列表照原样全量展示并区分两种状态。
  `invited_count = 0` 时按 5.6 要求隐藏汇总行。
- **建议**：规范注明汇总行口径为「已完成三步闭环的人数」。

## 7. 危险区归属

- **矛盾**：5.6 节把「退出登录 / 注销账号…」写在「登录设备与授权」分区之下，但它同时被描述为
  独立的「危险区」；原型 `Account.tsx` 是一张独立的红边卡片。
- **处理**：跟随原型（UI 真源），实现为独立危险区卡片，位于五个分区之后。
- **建议**：规范里把危险区显式列为第六块。

## 8. 定价页 CTA 依赖登录态

- **缺口**：4.2 节要求「已订阅显示徽标 / 试用中显示剩余天数」，但未登录时前端拿不到 entitlement。
- **处理**：未登录一律显示「立即订阅」→ `/signup`；已登录按 `GET /auth/session` 的 entitlement 分支；
  已登录但未订阅时 CTA 指向 `/account`（付款只在账户中心完成）。

## 9. 付款渠道选择没有 UI 定义

- **缺口**：`POST /billing/checkout` 需要 `{channel}`，但 5.6 节只写了一个「续费 / 充值」按钮。
- **处理**：渠道列表取自 `GET /billing/plan` 的 `channels`，多于一个时在按钮旁渲染一个下拉选择，
  只有一个时直接使用，不打扰用户。
- **建议**：规范补充渠道选择的位置与默认值。

## 10. 验证码重发冷却：原型 10 秒 vs 规范 60 秒

- **矛盾**：原型 `verify-email` 用 10 秒并自注「规格为 60 秒」。
- **处理**：实现按 **60 秒**；若服务端 429 返回 `details.retry_after_seconds`，以服务端为准重置倒计时。

## 11. 忘记密码「60 秒内重复提交冷却」由前端计时

- **说明**：4.8 节要求 60 秒重复提交冷却，但成功响应（202）不带冷却秒数，只有 429 才带
  `retry_after_seconds`。前端在提交成功后本地起 60 秒倒计时；命中 429 时改用服务端秒数。

## 12. 邮箱验证成功页的 deep link scheme 未定义

- **缺口**：4.6 / 5.1 说「尝试 deep link 唤起 APP」，但没给 scheme。
- **处理**：采用 `cchaven://`（与 m1-spec 1.3 节 `oauth_clients.redirect_uri_patterns` 里的
  `cchaven://auth/callback` 同源），下载页与验证成功页均使用 `cchaven://open`。
- **建议**：规范固化 scheme 与 host 约定，M3 桌面端注册同一 scheme。

## 13. 「登录设备与授权」的 empty 态

- **说明**：5.6 节称列表恒非空（至少有当前会话）。实现仍保留 empty 分支（后端异常返回空数组时
  不出现纯空白），并在「除当前设备外没有其他设备」时隐藏「退出所有其他设备」按钮。

## 14. 账户中心的「账号 ID」展示

- **说明**：规范未要求在账户中心展示注册号，但 `GET /me` 返回 `id_display`（`U-100986`），
  客服排查时很有用。实现把它作为邮箱下方的次要 hint 展示，不占主视觉。

---

## 环境限制（不是规范矛盾，但影响验收方式）

本机无法启动 PostgreSQL（macOS SysV 共享内存上限过低，colima 的 lima 架构不匹配，且无 sudo），
因此 M1 控制面跑不起来，本轮**全部通过 MSW mock 开发与测试**。mock 的响应结构逐字段对齐
`services/cchaven-control/internal/api` 与 `internal/service` 的真实结构（见
`src/mocks/handlers.ts` 与 `src/api/types.ts` 的注释）。真机联调仍需一次人工验收，重点核对：

1. cookie 的 `SameSite` 已由控制面 `CCHAVEN_COOKIE_SAMESITE` 决定（默认 `lax`）：同站部署
   （`cchaven.cn` + `api.cchaven.cn`）保持 `lax`，只有控制面跨站部署才配 `none`。需确认部署形态与该配置一致。
2. `POST /oauth/authorize` 的 CSRF 校验依赖 `Origin` 头，官网域名需在控制面可信来源集合
   （`CCHAVEN_PUBLIC_URL` + `CCHAVEN_ADMIN_URL`）中。
3. `GET /invites/{code}` 的 `cch_ref` cookie 是否成功随后续 `GET /invites/current` 与
   `POST /auth/register` 带出（mock 用状态位代替 cookie，这条只能真机验证）。
