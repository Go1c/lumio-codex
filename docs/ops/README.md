# Lumio Monorepo · 运维手册（跨产品）

本目录是 **跨产品** 运维的入口：统一官网三站的部署、域名与 DNS 切换、服务端前置项、
整体上线验收。**单个产品内部**的编译打包、发版、后台维护仍在各自的 `docs/ops/`。

## 哪些事看哪份手册

| 你要做的事 | 看这里 |
| --- | --- |
| 部署 `lumiogame.com` / `cc.lumiogame.com` / `codex.lumiogame.com` 三站 | [01-web-sites-deploy.md](./01-web-sites-deploy.md) |
| 加 DNS 记录、配证书、旧域名 301、切换顺序与回滚 | [02-domains-and-dns.md](./02-domains-and-dns.md) |
| Sub2API 侧 CORS、`/auth/me` 契约、充值页可达性等本仓做不了的事 | [03-service-prerequisites.md](./03-service-prerequisites.md) |
| 三站上线后逐项验收 | [04-golive-checklist.md](./04-golive-checklist.md) |
| Lumio Codex 桌面端编译、打内测包、发 Release、Sub2API 契约 | [`codex/docs/ops/`](../../codex/docs/ops/README.md) |
| CCHaven 控制面部署、管理后台、桌面端打包、存量用户迁移 | [`cchaven/docs/ops/`](../../cchaven/docs/ops/README.md) |
| 三站的本地开发、环境变量、收口命令 | [`web/README.md`](../../web/README.md) |

一句话分工：**本目录管「网站与域名」，产品目录管「客户端与服务端」。**
同一套步骤只写一处；产品手册里涉及三站的段落一律链回这里，不复制步骤。

## 目标线上拓扑

| 域名 | 承载 | 产物来源 | 状态 |
| --- | --- | --- | --- |
| `lumiogame.com` | 总门户：品牌首页 + 唯一的注册 / 登录 / 2FA / 账户中心 | `web/apps/portal/dist/` | 待启用（见下方前置条件） |
| `cc.lumiogame.com` | CC避风港产品站（介绍 / 定价 / 下载） | `web/apps/cc/dist/` | 新增 |
| `codex.lumiogame.com` | Lumio Codex 产品站（介绍 / 下载） | `web/apps/codex/dist/` | 新增 |
| `api.lumio.games` | Sub2API：账号真源 + Key + 充值页 | 本仓库之外 | 已在线，**绝不可变更** |
| `api.cc.lumiogame.com` | CCHaven 控制面 API | `cchaven/services/cchaven-control` → `bin/control` | 新增，**域名待用户最终确认** |
| `admin.cchaven.cn` | CCHaven 运营后台 | `cchaven/apps/admin/dist/` | 沿用，未随本次迁移变动 |
| `lumio.games` | 旧 Codex 官网（GitHub Pages） | `codex/site/` | 下线并 301 到 `codex.lumiogame.com` |
| `cchaven.cn` | 旧 CC 官网 | `cchaven/apps/web/dist/` | 下线并 301 到 `cc.lumiogame.com` |

```text
             ┌──────────────────────┐        ┌────────────────────────┐
 浏览器 ────►│    lumiogame.com     │───────►│       Sub2API          │
             │  门户 / 账号中心     │        │   api.lumio.games      │
             └──────────┬───────────┘        │  账号真源 + /purchase  │
                        │                    └───────────▲────────────┘
       ┌────────────────┴────────────────┐                │ Bearer 令牌校验
       ▼                                 ▼                │ GET /api/v1/auth/me
┌──────────────────┐            ┌─────────────────────┐   │
│ cc.lumiogame.com │            │ codex.lumiogame.com │   │
│   CC 产品站      │            │  Codex 产品站       │   │
└────────┬─────────┘            └─────────────────────┘   │
         ┆ /api/* 反代（规划中，现版本 CC 站不调控制面）    │
         ▼                                                 │
┌────────────────────────┐                                 │
│ api.cc.lumiogame.com   │─────────────────────────────────┘
│  cchaven-control       │  （桌面端 / 后台在用）
└────────────────────────┘
```

## 两条硬事实（改任何东西前先确认）

- **`api.lumio.games` 是存量桌面客户端硬编码的地址**（`codex/crates/codex-plus-core/src/lumio/product.rs`
  的 `API_BASE_URL`），迁移期间不做任何变更、不加跳转、不换证书链之外的东西。
- **账号只有一套**：邮箱 / 口令 / 2FA / 余额都在 Sub2API。`cchaven-control` 的自有终端用户
  认证端点已下线（返回 410），三站与桌面端都不得再引入第二套注册登录。

## 前置条件（阻塞上线）

1. **`lumiogame.com` 当前挂的是 Workflow 产品介绍页**（与本仓无关的第三个产品），
   必须先迁离，门户才能占用该域名。这是 DNS / 托管侧操作，本仓无代码可改。
2. **`api.cc.lumiogame.com` 这个域名尚待用户最终拍板**。它已经写进 Phase 3 的代码默认值
   （桌面端 `CCHAVEN_API_BASE`、运维文档），若最终改名，需同步改这些默认值后重新打包。
3. **Sub2API 侧需要为三个子域放行 CORS**，见 [03-service-prerequisites.md](./03-service-prerequisites.md)。

## 改了什么要同步哪份文档

| 改动 | 必须更新 |
| --- | --- |
| `web/` 的构建脚本、产物路径、新增环境变量 | [01-web-sites-deploy.md](./01-web-sites-deploy.md) + [`web/README.md`](../../web/README.md) 环境变量表 |
| 域名、证书、301 规则 | [02-domains-and-dns.md](./02-domains-and-dns.md) + 各产品手册中的域名表 |
| Sub2API 契约（信封、字段、错误码） | [03-service-prerequisites.md](./03-service-prerequisites.md) + [`codex/docs/ops/04-backend.md`](../../codex/docs/ops/04-backend.md) |
| 账号体系 / 身份收口相关设计 | `.spec/knowledge/features/lumio-unified-portal-and-identity.md`（决策另记 `.spec/decisions/`） |
