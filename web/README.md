# Lumio 统一官网门户（`web/`）

三个静态站点 + 两个共享包，npm workspaces 单仓管理。UI 基准是原 `cc.lumiogame.com`
的克制蓝白风（设计 token 抽取自 `cchaven/apps/web/src/styles.css`），三站共用同一套外壳与组件。

## 站点与域名

| 工作区 | 域名 | 职责 |
|--------|------|------|
| `apps/portal` | `lumiogame.com` | 品牌总站 + **唯一**的注册 / 登录 / 2FA / 账户中心 |
| `apps/cc` | `cc.lumiogame.com` | CC避风港产品站（介绍 / 定价 / 下载） |
| `apps/codex` | `codex.lumiogame.com` | Lumio Codex 产品站（介绍 / 下载） |

- 账号数据全部在 **Sub2API**（`https://api.lumio.games`）；本目录不引用 `cchaven/` 与 `codex/` 的任何代码。
- 产品站**不做自己的登录**：账号入口一律跳门户并带 `?next=` 回跳参数（`portalAccountLinks()`）。
- **充值统一跳 `https://api.lumio.games/purchase`**（Codex 与 CC 都是），不经过 cchaven 控制面的 billing。

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
    ├── cc/                 # CC避风港产品站
    └── codex/              # Lumio Codex 产品站
```

共享包以**源码**形式被引用（`exports` 指向 `src/`，配合 Vite alias 与 tsconfig `paths`），
不需要先构建 packages 再构建 apps。

## 本地开发

```bash
cd web
npm install

npm run dev:portal   # http://localhost:5280
npm run dev:cc       # http://localhost:5281
npm run dev:codex    # http://localhost:5282
```

三站默认互指生产域名。本地联调跨站跳转时，在**各 app 目录**放 `.env.local` 覆盖：

```dotenv
VITE_PORTAL_URL=http://localhost:5280
VITE_CC_URL=http://localhost:5281
VITE_CODEX_URL=http://localhost:5282
```

## 收口命令

```bash
cd web
npm run check   # 各工作区 tsc -b
npm test        # 各工作区 vitest run
npm run build   # 三个 app 的 tsc -b && vite build
npm run lint    # eslint（可选，不在收口门槛内）
```

## 环境变量

所有变量都有生产默认值，未配置也能正常构建；域名与接口地址只在 `packages/ui/src/config.ts`
一处收敛，页面里不出现第二份硬编码。

| 变量 | 默认值 | 作用 | 使用方 |
|------|--------|------|--------|
| `VITE_API_BASE_URL` | `https://api.lumio.games` | Sub2API base，充值页地址也由它派生 | 三站 |
| `VITE_ROOT_DOMAIN` | `lumiogame.com` | 会话 Cookie 作用域与 `next` 回跳白名单的根域 | 三站 |
| `VITE_PORTAL_URL` | `https://lumiogame.com` | 门户地址（账号入口跳转目标） | 三站 |
| `VITE_CC_URL` | `https://cc.lumiogame.com` | CC 站地址 | 三站 |
| `VITE_CODEX_URL` | `https://codex.lumiogame.com` | Codex 站地址 | 三站 |
| `VITE_CC_DOWNLOAD_ARM_URL` | 空 | CC 桌面端 Apple Silicon 安装包地址 | `apps/cc` |
| `VITE_CC_DOWNLOAD_INTEL_URL` | 空 | CC 桌面端 Intel 安装包地址 | `apps/cc` |
| `VITE_CC_VERSION` | 空 | CC 桌面端版本号（仅展示） | `apps/cc` |

未配置 CC 下载地址时，下载页显示空态而不是坏链接。

## 部署

**部署步骤的权威文档是根 [`docs/ops/`](../docs/ops/README.md)**，本文件不重复：

- 构建、静态托管方案（nginx / Cloudflare Pages）、SPA 回退、Codex 站的 `/latest-internal.json`
  指针 → [`docs/ops/01-web-sites-deploy.md`](../docs/ops/01-web-sites-deploy.md)
- DNS 记录、证书、旧域名 301、切换顺序与回滚 → [`docs/ops/02-domains-and-dns.md`](../docs/ops/02-domains-and-dns.md)
- Sub2API 侧的 CORS 与接口契约要求 → [`docs/ops/03-service-prerequisites.md`](../docs/ops/03-service-prerequisites.md)
- 上线验收清单 → [`docs/ops/04-golive-checklist.md`](../docs/ops/04-golive-checklist.md)

产物目录：`apps/portal/dist/`、`apps/cc/dist/`、`apps/codex/dist/`，都是静态 SPA
（未命中的路径必须回退 `index.html`）。

Codex 下载区先读**同源** `/latest-internal.json`（避开 S3 的 CORS），失败再读
`https://s3.lumio.games/lumio-codex/releases/latest-internal.json`，两者都不可用时退回
GitHub Releases 页。同源指针在部署时由发布流水线的产物复制到站点根目录，
本目录不生成也不复制该文件。

## 对服务端 / 运维的依赖

以下三项**必须由服务端与运维配合**，前端无法单方面解决（操作细节见
[`docs/ops/03-service-prerequisites.md`](../docs/ops/03-service-prerequisites.md)）：

1. **Sub2API CORS**：`https://api.lumio.games` 需允许来源 `https://lumiogame.com`、
   `https://cc.lumiogame.com`、`https://codex.lumiogame.com`（本地联调还需 `http://localhost:528x`），
   允许 `Authorization` 与 `Content-Type` 请求头。三站是纯静态站点，直连 Sub2API，没有同源反代。
2. **会话 Cookie 域**：令牌写在父域 Cookie（`Domain=.lumiogame.com`、`Path=/`、`SameSite=Lax`、
   https 下带 `Secure`），子站据此判断登录态。父域 Cookie 需要前端可读，因此**不是 HttpOnly**；
   风险由「短 access token 有效期 + 收紧 CORS + 三站同属一个可信根域」共同兜住。
   若日后接入统一网关，应改为 HttpOnly + 服务端下发，届时 `packages/auth/src/session.ts` 是唯一改动点。
3. **DNS 与证书**：三个子域各自指向对应的静态产物，全站 https。

## 与其他目录的边界

- 本目录**只读**引用 `cchaven/`、`codex/` 的内容作为文案与契约来源，不 import 其代码、不修改其文件。
- Sub2API 契约与错误码以 `codex/crates/codex-plus-core/src/lumio/{api,errors}.rs` 为权威；
  `packages/auth/src/errors.ts` 的映射表与之逐条对齐，改动需同步两侧。
