# BestCodex 官网门户（`web/`）

两个静态站点 + 两个共享包，npm workspaces 单仓管理。用户可见产品品牌是 **BestCodex**，站点
[`https://bestcodex.app`](https://bestcodex.app)，帮助 [`https://bestcodex.app/help`](https://bestcodex.app/help)。
门户本身仍是 **Lumio** 账号中心，不整站改成启动器。UI 基准是原产品站的克制蓝白风（设计 token
抽取自 `cchaven/apps/web/src/styles.css`），两站共用同一套外壳与组件。

## 站点与域名

| 工作区 | 域名 | 职责 |
|--------|------|------|
| `apps/portal` | 独立门户（本期部署不变） | 品牌总站 + **唯一**的注册 / 登录 / 2FA / 账户中心 + Claude / CC 桌面端授权页 `/authorize` |
| `apps/bestcodex` | `bestcodex.app` | 产品站：`/` `/codex` 为 Codex 落地页，`/claude` 为 Claude 落地页，帮助 `/help` |

- 账号数据全部在 **Sub2API**（`https://api.lumio.games`）；本目录不引用 `cchaven/` 与 `codex/` 的任何代码。
- 门户的 `/authorize` 是 **CC 桌面端浏览器登录的确认页**：桌面端带 PKCE 参数打开它，用户在账号中心
  会话下确认后，门户带 Sub2API 令牌调 **CC 控制面**（`https://api.cc.bestcodex.app`）的
  `POST /api/v1/oauth/authorize` 换授权码，再按响应的 `redirect_to` 跳回本机回环端口。
  控制面仍是桌面端的 OAuth token issuer，但已不是身份提供方；令牌交换（`/oauth/token`）在桌面端完成，门户不参与。
- 产品站**不做自己的登录**：账号入口一律跳门户并带 `?next=` 回跳参数（`portalAccountLinks()`）。
- **充值统一跳 `https://api.lumio.games/purchase`**（Codex 与 Claude 都是），不经过 cchaven 控制面的 billing。
- 页脚须声明与 OpenAI、Anthropic 无从属。CORS 放行 `https://bestcodex.app` 与 DNS 仍待运维。
- 旧子域 `codex.bestcodex.app` / `cc.bestcodex.app` 的 301 是运维事项，本仓不配置。共存选择见
  [ADR 0007](../.spec/decisions/0007-bestcodex-apex-portal-coexistence.md)。

## 目录结构

```
web/
├── package.json            # workspaces + 递归 check / test / build
├── tsconfig.base.json      # 共享编译选项与 @lumio/* 路径别名
├── eslint.config.js        # 全仓一份 flat config（根目录 npm run lint）
├── packages/
│   ├── ui/                 # @lumio/ui：设计 token、基础组件、站点外壳、跨站配置
│   │   └── src/styles/     # tokens.css（变量）/ base.css（重置+外壳）/ components.css
│   └── auth/               # @lumio/auth：Sub2API 客户端、错误码映射、跨子域会话
└── apps/
    ├── portal/             # 门户（含账号中心）
    └── bestcodex/          # BestCodex 产品站（Codex / Claude 站内换页）
```

共享包以**源码**形式被引用（`exports` 指向 `src/`，配合 Vite alias 与 tsconfig `paths`），
不需要先构建 packages 再构建 apps。

## 本地开发

```bash
cd web
npm install

npm run dev:portal      # http://localhost:5280
npm run dev:bestcodex   # http://localhost:5282
```

两站默认互指生产域名。本地联调跨站跳转时，在**各 app 目录**放 `.env.local` 覆盖：

```dotenv
VITE_PORTAL_URL=http://localhost:5280
VITE_CODEX_URL=http://localhost:5282/codex
VITE_CC_URL=http://localhost:5282/claude
```

## 收口命令

```bash
cd web
npm run check   # 各工作区 tsc -b
npm test        # 各工作区 vitest run
npm run build   # portal 与 bestcodex 的 tsc -b && vite build
npm run lint    # eslint（可选，不在收口门槛内）
```

## 环境变量

所有变量都有生产默认值，未配置也能正常构建；域名与接口地址只在 `packages/ui/src/config.ts`
一处收敛，页面里不出现第二份硬编码。

| 变量 | 默认值 | 作用 | 使用方 |
|------|--------|------|--------|
| `VITE_API_BASE_URL` | `https://api.lumio.games` | Sub2API base，充值页地址也由它派生 | 两站 |
| `VITE_CC_CONTROL_URL` | `https://api.cc.bestcodex.app` | CC 控制面 base，`/authorize` 的授权端点由它派生 | `apps/portal` |
| `VITE_ROOT_DOMAIN` | `bestcodex.app` | 会话 Cookie 作用域与 `next` 回跳白名单的根域 | 两站 |
| `VITE_PORTAL_URL` | `https://bestcodex.app` | 门户地址（账号入口跳转目标） | 两站 |
| `VITE_CC_URL` | `https://bestcodex.app/claude` | Claude 落地页地址 | 两站 |
| `VITE_CODEX_URL` | `https://bestcodex.app/codex` | Codex 落地页地址 | 两站 |
| `VITE_CC_DOWNLOAD_ARM_URL` | 空 | 历史 Claude 独立包地址（现已并入同一启动器） | 保留兼容 |
| `VITE_CC_DOWNLOAD_INTEL_URL` | 空 | 同上 | 保留兼容 |
| `VITE_CC_VERSION` | 空 | 历史版本号展示 | 保留兼容 |
| `VITE_SUPPORT_QQ_NUMBER` | `1073671738` | 客服气泡的 QQ 群号（点击复制）；写 `off` 可关掉 | 两站 |
| `VITE_SUPPORT_FEISHU_URL` | Lumio 飞书客服群 | 客服气泡的飞书群加入链接；写 `off` 可关掉 | 两站 |

未配置下载清单时，下载区回退 GitHub 发布页而不是坏链接。

## 部署

**部署步骤的权威文档是根 [`docs/ops/`](../docs/ops/README.md)**，本文件不重复：

- 构建、静态托管方案（nginx / Cloudflare Pages）、SPA 回退、产品站的 `/latest-internal.json`
  指针 → [`docs/ops/01-web-sites-deploy.md`](../docs/ops/01-web-sites-deploy.md)
- DNS 记录、证书、旧域名 301、切换顺序与回滚 → [`docs/ops/02-domains-and-dns.md`](../docs/ops/02-domains-and-dns.md)
- Sub2API 侧的 CORS 与接口契约要求 → [`docs/ops/03-service-prerequisites.md`](../docs/ops/03-service-prerequisites.md)
- 上线验收清单 → [`docs/ops/04-golive-checklist.md`](../docs/ops/04-golive-checklist.md)

产物目录：`apps/portal/dist/`、`apps/bestcodex/dist/`，都是静态 SPA
（未命中的路径必须回退 `index.html`）。本期不改门户托管。

产品站下载区先读**同源** `/latest-internal.json`（避开 S3 的 CORS），失败再读
`https://s3.lumio.games/lumio-codex/releases/latest-internal.json`，两者都不可用时退回
GitHub Releases 页。同源指针在部署时由发布流水线的产物复制到站点根目录，
本目录不生成也不复制该文件。

## 对服务端 / 运维的依赖

以下四项**必须由服务端与运维配合**，前端无法单方面解决（操作细节见
[`docs/ops/03-service-prerequisites.md`](../docs/ops/03-service-prerequisites.md)）。
**CORS 与 DNS 仍待运维**，本仓不改生产：

1. **Sub2API CORS**：`https://api.lumio.games` 需允许来源 `https://bestcodex.app`
   （本地联调还需 `http://localhost:528x`），允许 `Authorization` 与 `Content-Type` 请求头。
   两站是纯静态站点，直连 Sub2API，没有同源反代。
2. **CC 控制面 CORS**：`/authorize` 是跨源请求——页面在门户，授权端点在
   `api.cc.bestcodex.app`。控制面须放行门户来源、允许 `Authorization` 与
   `Content-Type` 请求头，并正确响应 `OPTIONS` 预检。可信来源来自 `CCHAVEN_PORTAL_URL`
   （`config.Config.TrustedOrigins`），**漏配则 CC 桌面端在生产完全无法登录**，而 dev 放行
   localhost 任意端口，本地联调看不出来。
3. **会话 Cookie 域**：令牌写在父域 Cookie（`Domain=.bestcodex.app`、`Path=/`、`SameSite=Lax`、
   https 下带 `Secure`）。父域 Cookie 需要前端可读，因此**不是 HttpOnly**；
   风险由「短 access token 有效期 + 收紧 CORS + 同属一个可信根域」共同兜住。
   若日后接入统一网关，应改为 HttpOnly + 服务端下发，届时 `packages/auth/src/session.ts` 是唯一改动点。
4. **DNS 与证书**：营销 apex 指向产品站静态产物，门户保持现有托管。全站 https。
   旧子域 301 不在本仓。

## 与其他目录的边界

- 本目录**只读**引用 `cchaven/`、`codex/` 的内容作为文案与契约来源，不 import 其代码、不修改其文件。
- Sub2API 契约与错误码以 `codex/crates/codex-plus-core/src/lumio/{api,errors}.rs` 为权威；
  `packages/auth/src/errors.ts` 的映射表与之逐条对齐，改动需同步两侧。
