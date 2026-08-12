# CC避风港（CCHaven）官网 · `apps/web`

面向 cchaven.cn 的官网前端：营销页、注册登录找回、邀请落地页、账户中心与 APP 授权页。
React + Vite + TypeScript，界面语言 zh-CN（已按 6.5 节预留 zh-HK）。

**生产部署与发版**见仓库 [`docs/ops/`](../../docs/ops/README.md)（静态站构建、网关反代、版本推送）。
本 README 只覆盖本地开发与 API/mock 约定。

**权威依据**（冲突时以它们为准，本目录不复制规范内容）：

- 交互规范：[`docs/design/interaction-design.md`](../../docs/design/interaction-design.md)（重点：第 4 章官网逐页、5.6 账户中心、6.2 固定文案、6.1/6.3–6.6 全局规范）
- UI 真源：[`design/prototype/`](../../design/prototype/)（`src/site/*`、`src/styles.css`）
- API 真源：[`services/cchaven-control/internal/api/`](../../services/cchaven-control/internal/api/)，清单见 [`m1-spec.md`](../../services/cchaven-control/docs/m1-spec.md)
- 实现中发现的规范矛盾：[`docs/spec-gaps.md`](./docs/spec-gaps.md)

---

## 启动

```bash
cd apps/web
npm install
npm run dev        # http://localhost:5273，默认启用 MSW mock
```

| 命令 | 说明 |
| --- | --- |
| `npm run dev` | 开发服务器（默认走 mock） |
| `npm run build` | `tsc -b` 类型检查 + 产物构建到 `dist/` |
| `npm run preview` | 预览构建产物 |
| `npm run lint` | ESLint（flat config，TypeScript + react-hooks） |
| `npm test` | Vitest 全量跑一遍 |
| `npm run test:watch` | Vitest watch |

### 环境变量

| 变量 | 默认 | 说明 |
| --- | --- | --- |
| `VITE_API_BASE_URL` | 空 | 控制面地址。留空表示同源 `/api/v1`（生产由网关反代） |
| `VITE_ENABLE_MSW` | `true`（仅 dev） | 设为 `false` 关闭 mock，直连真实控制面 |

联调真实后端：

```bash
VITE_ENABLE_MSW=false VITE_API_BASE_URL=http://localhost:8080 npm run dev
```

---

## Mock 说明（为什么默认不连真后端）

本机**跑不起 PostgreSQL**（macOS SysV 共享内存上限过低，`shmget` 连 56 字节都 ENOMEM；
colima 的 lima 是 x86_64 无法启动；无 sudo），因此 M1 控制面无法在本地启动。
开发与测试统一走 [MSW](https://mswjs.io)：

- 浏览器端：`src/mocks/browser.ts`，由 `src/main.tsx` 在 dev 且 `VITE_ENABLE_MSW !== "false"` 时启动，
  worker 脚本在 `public/mockServiceWorker.js`（由 `npx msw init public` 生成）。
- 测试端：`src/mocks/server.ts`，在 `src/test/setup.ts` 里 `listen`，每个用例后 `resetHandlers()` + `resetDb()`。
- 处理器：`src/mocks/handlers.ts`，**响应结构逐字段对齐**
  `services/cchaven-control/internal/service` 的导出类型；成功包 `{"data": ...}`，
  失败包 `{"error":{"code","message","details"}}`，`message` 逐字取自服务端 `internal/i18n`。

### mock 里的固定账号与数据（手工验收各分支用）

| 场景 | 输入 | 结果 |
| --- | --- | --- |
| 正常登录 | `mary@example.com` / `Password123` | 成功，已订阅 27 天 |
| 凭据错误 | 任意邮箱 + 其他密码 | `401 invalid_credentials` |
| 邮箱未验证 | `unverified@example.com` | `403 email_unverified` |
| 账号锁定 | `locked@example.com` | `423 account_locked`，`retry_after_seconds=900` |
| 账号停用 | `disabled@example.com` | `403 account_disabled` |
| 邮箱已注册 | 注册 `taken@example.com` | `409 email_taken` |
| 注册限频 | 注册 `limited@example.com` | `429 rate_limited` |
| 验证码 | `123456` 正确，其他递减剩余次数至 0 后转过期 | 对应 6.2 节文案 |
| 重设密码链接 | `?token=valid-token` 有效，其余失效 | `410 reset_link_invalid` |
| 邀请码 | `/i/mary8k2f` 有效（邀请人 Alex），其余 `valid:false` | 失效不阻断注册 |
| 邀请归因 | 打开一次有效邀请链接后 `GET /invites/current` 返回 `attributed:true` | mock 用状态位 `db.inviteAttributed` 代替 HttpOnly cookie，整页刷新会重置 |
| 授权页 | `client_id=cchaven-desktop` + `code_challenge_method=S256` 合法 | 其余 `400 invalid_request` |

---

## 目录结构

```
apps/web/
├── public/mockServiceWorker.js   # MSW 浏览器 worker
├── docs/spec-gaps.md             # 规范矛盾与处理方式
└── src/
    ├── api/                      # 控制面接口：types.ts（响应类型）+ endpoints.ts（调用封装）
    ├── components/               # 站点框架与通用组件
    │   ├── SiteLayout.tsx        # 顶栏 / 页脚 / 跳转主内容
    │   ├── PlanCard.tsx          # 单一套餐卡片（首页摘要与 /pricing 共用）
    │   ├── CodeInput.tsx         # 6 格验证码（跳位/粘贴/自动提交/逐格播报）
    │   ├── Modal.tsx             # focus trap + Esc 关闭
    │   ├── Toast.tsx             # 4 秒（带撤销 10 秒）通知
    │   ├── fields.tsx            # TextField / PasswordField（强度条 + 规则打勾）
    │   └── ui.tsx                # Banner / Skeleton / LoadingBlock / ErrorBlock / EmptyBlock / Truncated / StatusDot
    ├── hooks/                    # useResource（五态加载）、useCountdown（倒计时）
    ├── i18n/                     # zh-CN 字典（含 6.2 固定文案区块）、zh-HK 占位、t()/useT()
    ├── lib/                      # api.ts（fetch 客户端）、format.ts、validation.ts
    ├── mocks/                    # MSW handlers / db / browser / server
    ├── pages/
    │   ├── Home / Pricing / Download / InviteLanding / NotFound
    │   ├── Signup / VerifyEmail / Login / ForgotPassword / ResetPassword
    │   ├── Authorize.tsx         # APP 授权页（原型缺失，按 3.4 / 5.1 设计）
    │   └── account/              # 账户中心五个分区 + 危险区
    ├── state/                    # session（会话）/ publicConfig（后台配置）/ inviteAttribution（邀请归因，共享一次请求）
    ├── test/                     # setup.ts、utils.tsx（renderApp / renderWithProviders）
    ├── App.tsx / main.tsx / styles.css
```

---

## 与 M1 API 的对接约定

### 请求与响应

- 前缀 `${VITE_API_BASE_URL}/api/v1`；成功响应解包 `data`，失败抛 `ApiError`（带 `status` / `code` / `details`）。
- **错误展示**：`message` 已是规范文案，直接展示；**交互分支按 `code` 走**，例如
  `email_taken` → inline + 登录/找回链接，`email_unverified` → 「重新发送验证邮件」按钮，
  `account_locked` → 按钮禁用到 `details.retry_after_seconds` 倒计时结束，
  `code_invalid` → 抖动 + 剩余次数，`code_expired` → 格子禁用、只留重发。
- **6.2 节七条固定文案**由服务端下发；前端字典 `src/i18n/zh-CN.ts` 保留同一份，
  并由 `src/i18n/__tests__/messages.test.ts` 逐字锁定（与服务端 `i18n_test.go` 互为镜像）。

### 会话

- 官网会话是 HttpOnly cookie（`cch_sess` / `cch_refresh`），所有请求 `credentials: "include"`；
  写操作的 `Origin` 由浏览器自动带上，服务端据此做 CSRF 校验。
- access token 15 分钟过期：收到 `401 session_expired` 时自动 `POST /auth/refresh` 并**重试一次**，
  仍失败才清空会话（页面据此回到未登录态 / 引导登录）。见 `src/lib/api.ts`。

### 后台可配置项（前端一律不写死）

`GET /api/v1/config/public` 提供价格与币种、`invite.reward_days`、`invite.trial_days`、下载版本与地址。
`invite.reward_days === 0` 时隐藏全部「订阅延长 X 天」文案（定价页、账户中心邀请分区）。
账户中心的套餐价格与支付渠道另取自 `GET /api/v1/billing/plan`。

### 付款

只在官网：`POST /api/v1/billing/checkout` → 跳返回的 `pay_url`（支付服务商托管页），站内不收集任何卡号。

### 邀请归因

归因载体是服务端下发的 HttpOnly cookie `cch_ref`，前端读不到，也**不缓存任何展示副本**
（副本不随 cookie 过期、也不随邀请码停用失效，会造成错误承诺，见 spec-gaps 第 3 条）。

| 接口 | 用途 |
| --- | --- |
| `GET /api/v1/invites/{code}` | 落地页 `/i/{code}`：服务端在此下发 `cch_ref` 并记 `referral_visits` |
| `GET /api/v1/invites/current` | 邀请横幅的唯一判据：读 `cch_ref` 返回 `{attributed}`（有效时带 `inviter` / `trial_days`）；只读，不下发 cookie、不记访问 |

前端约定（`src/state/inviteAttribution.tsx`）：

- 一次页面会话内**只请求一次** `/invites/current`，首页与注册页共用同一份结果，**不轮询**（接口目前无限频）。
- 归因未确定（加载中或请求失败）→ 不渲染邀请横幅、**不弹错误条**：横幅是营销元素，挂了不该打断注册。
- 落地页拿到的响应与 `/invites/current` 同口径，`valid:true` 时就地写入共享状态，同一次会话跳注册页无需重复请求；
  `valid:false` 不写入——失效的邀请码不会清掉浏览器上原有的 cookie，该不该显示仍由服务端裁决。

### 部署相关的控制面配置

| 配置项 | 与官网的关系 |
| --- | --- |
| `CCHAVEN_PUBLIC_URL` / `CCHAVEN_ADMIN_URL` | 控制面的可信来源集合，CORS 与写操作的 CSRF（`Origin`）校验共用。官网域名必须在 `CCHAVEN_PUBLIC_URL` 里，否则所有写操作 403；`CCHAVEN_ADMIN_URL` 是后台域名，与官网无关但漏配会让后台全线 403 |
| `CCHAVEN_COOKIE_SAMESITE` | 会话 cookie 的 `SameSite`，默认 `lax`。同站部署（`cchaven.cn` + `api.cchaven.cn`）保持 `lax`；只有控制面与官网不同站（eTLD+1 不同）时才配 `none`，此时服务强制 `Secure`。不提供 `strict`（会断掉「邮件点重设密码链接跳回」） |

---

## 五态与可访问性

- 每个页面/分区覆盖 loading（骨架 + `aria-busy`）、empty（图标 + 说明 + 行动）、
  error（错误条 + 重试）、disabled（提交中禁用整表单/按钮）、无权限（未登录 → 引导登录）。
- 全部交互元素可 Tab 聚焦且焦点环可见；模态 focus trap + Esc 关闭并恢复焦点；
  验证码逐格 `aria-label`（「第 N 位，共 6 位」）+ `aria-live` 汇报进度；
  状态点一律配文字标签；长邮箱/长设备名中间省略号截断并 `title` 显示全文；
  响应式最小宽度 375px 不横向溢出。

## 测试

`npm test` 共 10 个文件 97 条用例，覆盖：

| 文件 | 覆盖 |
| --- | --- |
| `i18n/__tests__/messages.test.ts` | 6.2 节七条文案逐字断言 + 插值 + zh-HK 回落 |
| `lib/__tests__/api.test.ts` | `{"data"}` 解包、`ApiError`、401 → refresh → 重试一次、刷新失败不重试 |
| `pages/__tests__/signup-verify.test.tsx` | 注册五态 → 验证码递减/过期/禁用 → 60 秒冷却 → 成功页 |
| `pages/__tests__/login.test.tsx` | 凭据错误 / 未验证 / 锁定倒计时 / 停用 / `next` 回跳 |
| `pages/__tests__/password-reset.test.tsx` | 恒定回执文案、重复提交冷却、链接失效态、重设成功 |
| `pages/__tests__/invite.test.tsx` | 邀请落地有效/失效两态 + 横幅传递 |
| `pages/__tests__/invite-banner.test.tsx` | `/invites/current` 的 `attributed` 真/假、接口失败与网络故障静默降级、不闪横幅、一次会话只请求一次、无 localStorage 读写 |
| `pages/__tests__/marketing.test.tsx` | 首页/定价/下载的价格与版本来自后台配置 + 五态 |
| `pages/account/__tests__/account.test.tsx` | 账户中心五个分区 × 五态 + 危险区 |
| `pages/__tests__/authorize.test.tsx` | 未登录 / 已登录 / 授权成功（含授权码兜底）/ 取消 / 参数非法 |

### 测试环境的两处 shim（`src/test/setup.ts`）

1. 当前 Node 下 jsdom 的 `window.localStorage` 不可用，用内存实现顶上。应用代码本身不读写
   localStorage，这个 shim 只是给「不再有任何 localStorage 读写」那条防回归断言一个可监听的对象。
2. jsdom 的 `AbortSignal` 与 Node 的 `fetch` 不同源，直接传会被
   「Expected signal to be an instance of AbortSignal」拒绝，测试中剥掉 `signal`；
   浏览器无此问题，生产代码照常用 `AbortController` 取消在途请求。
