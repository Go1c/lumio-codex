# CC避风港（CCHaven）管理员运营后台

M4 交付物：面向内部运营的管理后台，独立入口（部署为 `admin.cchaven.cn`），与官网、桌面 APP 完全隔离。
React 19 + Vite + TypeScript，界面语言简体中文（zh-CN），文案已抽成 i18n 字典并预留 zh-HK。

**生产部署与发版**见 [`docs/ops/`](../../docs/ops/README.md)。  
**后台文档如何持续维护**见 [`docs/ops/05-maintain-docs.md`](../../docs/ops/05-maintain-docs.md)（改管理 API 时必读）。

对应规范：`docs/design/interaction-design.md` 第 7 章（7.1 仪表盘 / 7.2 用户 / 7.3 订单与付款 / 7.4 运营配置 / 7.5 后台通用规范），
以及 6.1 表单校验、6.2 固定安全文案、6.4 反馈组件、6.5 语言与本地化、6.6 可访问性。
UI 参照 `design/prototype/src/admin/Admin.tsx`。接口以 `services/cchaven-control` 的代码为准。

## 快速开始

```bash
cd apps/admin
npm install
npm run dev          # http://localhost:5183 ，默认带 MSW mock 数据
```

mock 环境下的登录凭据：

| 项 | 值 |
| --- | --- |
| 邮箱 | `admin@cchaven.cn` |
| 密码 | `admin12345` |
| 两步验证码 | `123456`（mock 固定值，真实环境由 TOTP 算法校验） |

首次登录会进入「启用两步验证」强制引导页（扫码 / 手动录入密钥 → 输入 `123456`），完成后才进仪表盘。
想直接复现「已启用 2FA」的登录路径，先启用一次再退出登录即可（mock 状态存在内存里，刷新页面会重置）。

其他脚本：

```bash
npm run build        # tsc --noEmit + vite build
npm run lint         # eslint
npm test             # vitest run（48 个用例）
npm run test:watch
```

## Mock 说明

本机起不了控制面依赖的 PostgreSQL（macOS SysV 共享内存上限过低，colima 的 lima 又是 x86_64 装法），
因此**开发与测试全部跑在 MSW mock 上**，没有对真实 `services/cchaven-control` 做过联调。

- mock 定义在 `src/mocks/`：`data.ts` 是内存数据与会话状态，`handlers.ts` 是请求处理器。
- 响应结构逐字段对齐 `internal/service/admin.go`、`internal/api/handler_admin.go` 与 `internal/store/*`：
  成功一律 `{"data": ...}`，失败一律 `{"error":{"code","message","details"}}`，错误 `message` 直接取自 `internal/i18n/i18n.go`。
- mock 同样实现了会话语义：未登录 → `401 unauthorized`；**半会话（未过两步验证）访问任何业务接口 → `401 mfa_required`**。
- mock 会随操作变更内存状态（禁用/解禁、退款、改配置、查看用户详情都会写入审计日志），便于验证完整流转。
- mock 的行操作路径参数只接受纯数字（与后端 `strconv.ParseInt` 一致），前端若误传展示号 `U-100986` 会立刻拿到 400。
- mock 登录的管理员角色默认 `owner`。想看 support 的表现（详情入口禁用 + 直接访问详情吃 403），
  改 `src/mocks/data.ts` 里 `freshState()` 的 `admin.role` 即可。
- 开发期 mock 由 `src/mocks/browser.ts` 的 Service Worker 提供，worker 脚本在 `public/mockServiceWorker.js`（`npx msw init public` 生成）。
- 生产构建不含 mock：`main.tsx` 里用 `import.meta.env.DEV` 把这段代码摇掉。

### 连真实后端

```bash
VITE_USE_MOCK=false VITE_API_ORIGIN=http://localhost:8080 npm run dev
```

Vite 会把 `/api/*` 代理到 `VITE_API_ORIGIN`（默认 `http://localhost:8080`），这样浏览器与后端同源，
HttpOnly cookie `cch_admin` 才能正常带上。生产部署时由网关把 `admin.cchaven.cn/api` 转到控制面，
并把 `admin.cchaven.cn` 加入控制面的允许来源（见 `internal/api/api.go` 的 `allowedOrigin`）。

## 创建管理员账号

管理员是与普通用户完全隔离的独立体系，**没有自助注册入口**，只能由运维用 `cmd/admin-bootstrap` 创建：

```bash
cd services/cchaven-control
export CCHAVEN_DATABASE_URL='postgres://...'
export CCHAVEN_ADMIN_PASSWORD='至少 8 位且含字母与数字'
go run ./cmd/admin-bootstrap -email ops@cchaven.cn -name 运营管理员 -role owner
```

`-role` 可选 `owner` / `ops` / `support`（侧栏会显示为超级管理员 / 运营 / 客服）。
创建完成后用该邮箱登录后台，前端会强制走完 `POST /auth/totp/setup` → `POST /auth/totp/enable` 才放行。

## 与 M1 管理 API 的对接约定

前缀 `/api/admin/v1`，会话走 HttpOnly cookie `cch_admin`，前端所有请求 `credentials: 'include'`（见 `src/api/client.ts`）。

| 页面 / 能力 | 接口 |
| --- | --- |
| 登录 | `POST /auth/login` → `{mfa_required, mfa_enrolled}` |
| 两步验证 | `POST /auth/login/totp`、`POST /auth/totp/setup`、`POST /auth/totp/enable` |
| 身份与登出 | `GET /auth/me`、`POST /auth/logout` |
| 仪表盘 | `GET /metrics/overview`、`GET /metrics/dau?days=7`、`GET /metrics/distributions?days=30` |
| 用户 | `GET /users?query=&status=all\|sub\|trial\|none\|banned&page=&page_size=`、`POST /users/{id}/disable`、`POST /users/{id}/enable` |
| 用户详情 | `GET /users/{id}`（`id` 是数字 `user_id`；含明文邮箱，仅 owner / ops） |
| 订单 | `GET /orders?status=...`、`GET /orders/export`、`POST /orders/{orderNo}/refund` |
| 运营配置 | `GET /configs`、`PUT /configs` |
| 审计日志 | `GET /audit-logs?actor=&action=&page=&page_size=`（两个筛选空串表示不筛选，可组合） |

约定要点：

1. **响应信封**：成功 `{"data": ...}`，失败 `{"error":{"code","message","details"}}`。
   `message` 已是 6.2 节规范文案（如「邮箱或密码不正确。」「尝试次数过多，请 {n} {unit}后再试。」），前端**原样展示，不重写**。
   分支判断一律用稳定的 `code`。
2. **缺数语义**：`MetricCard.value` 为 `null` 表示没数据，卡片显示「—」而不是 0；`delta` / `secondary` 同理。
3. **半会话**：登录后若 `mfa_required`，此时只是半会话，访问任何业务接口都会返回 `401 mfa_required`；
   前端由 `AuthProvider.handleApiError` 统一接管，整屏退回两步验证页。
4. **权限不足**：整屏级的 `403` 切到 403 页（`src/pages/ForbiddenPage.tsx`）。
   用户详情的 `403` 是资源级的（support 角色），会话仍然有效，因此就地渲染 403 而不退回登录页。
5. **用户 ID**：行数据同时带展示号 `id`（`U-100986`）与数字主键 `user_id`。
   **所有接口调用一律用 `user_id`，`id` 只用于显示**；前端不从展示号反解主键——那是后端的呈现约定，随时可能变。
6. **金额单位**：接口一律用「分」（`amount_cents`），展示层负责转元。
7. **运营配置写入**：请求体是 key→value 映射且 key 用点号形式，如
   `{"invite.reward_days": 14, "pricing.monthly": {"amount_cents": 9900, "currency": "CNY"}}`；
   前端只提交改动过的项，避免审计日志被无变化写入淹没。
8. **审计**：禁用用户、退款、改配置由后端自动记录操作人 + 时间 + 前后值，前端不重复写。
9. **邮箱脱敏**：列表里的邮箱由后端返回 `email_masked`。
   全后台只有用户详情页能拿到明文邮箱（`GET /users/{id}` 的 `user.email`），
   后端为每次访问写审计（`user.view_detail`），越权访问也写（`user.view_detail_denied`）；
   页面顶部常驻提示告知操作者这一点。邀请进度里被邀请者的邮箱仍是打码值。

## 目录结构

```
apps/admin/
├── README.md
├── docs/spec-gaps.md          # 规范矛盾与取舍记录
├── eslint.config.js
├── index.html
├── public/mockServiceWorker.js
├── vite.config.ts             # 含 vitest 配置与 /api 代理
└── src/
    ├── main.tsx               # 入口，按需启动 MSW
    ├── App.tsx                # 会话门禁 + 路由
    ├── styles.css             # 取自原型的后台样式
    ├── api/
    │   ├── client.ts          # fetch 封装、信封解包、ApiError
    │   ├── endpoints.ts       # 逐个接口的类型化调用
    │   └── types.ts           # 与 Go 结构体一一对应的类型
    ├── auth/
    │   ├── AuthProvider.tsx   # loading→anonymous→mfa_challenge/enroll→ready 状态机
    │   ├── AuthCard.tsx
    │   ├── LoginPage.tsx
    │   ├── TotpChallenge.tsx  # 半会话补交 TOTP
    │   └── TotpEnroll.tsx     # 首次强制启用 2FA（二维码 + 密钥）
    ├── components/
    │   ├── Sidebar.tsx        # 深色侧栏，品牌 +「运营后台 · 内部系统」
    │   ├── ConfirmDialog.tsx  # 二次确认模态，focus trap + Esc
    │   ├── ToastProvider.tsx
    │   └── common.tsx         # 错误条 / 骨架 / 徽标 / chips / 分页
    ├── i18n/
    │   ├── index.ts           # t() 与语言切换，zh-HK 缺条目回落 zh-CN
    │   └── zh-CN.ts
    ├── lib/
    │   ├── format.ts          # 日期、金额、比率、「—」占位
    │   └── orderLabels.ts     # 订单状态/渠道文案，订单页与用户详情页共用
    ├── mocks/                 # MSW：data.ts / handlers.ts / browser.ts / server.ts
    ├── pages/
    │   ├── DashboardPage.tsx
    │   ├── UsersPage.tsx
    │   ├── UserDetailPage.tsx # 明文邮箱、订阅、设备、邀请、最近订单
    │   ├── OrdersPage.tsx
    │   ├── SettingsPage.tsx
    │   ├── AuditLogSection.tsx
    │   └── ForbiddenPage.tsx
    └── test/                  # vitest + @testing-library/react
```

## 五态与可访问性

每个页面都覆盖：loading（骨架屏）、empty（筛选无结果的行内提示）、error（错误条 + 重试）、
操作进行中禁用（按钮 spinner + 表单/行操作禁用）、权限不足（403 页）。

- 全部交互元素可 Tab 聚焦，`:focus-visible` 有可见焦点环。
- 二次确认模态 `role="dialog" aria-modal="true"`，焦点圈定在模态内，Esc 可关闭。
- 筛选 chips 用 `aria-pressed` 表达选中态；状态徽标带文字标签，不只靠颜色。
- toast 区域 `role="status" aria-live="polite"`；错误条 `role="alert"`。
