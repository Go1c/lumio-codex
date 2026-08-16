# BestCodex

**BestCodex** 是一个启动器：下载一次、登录一次，窗口里两个 Tab——**Codex**（配好并启动官方 Codex）和 **Claude**（把 Claude 跑在自己的服务器上）。站点 [`https://bestcodex.app`](https://bestcodex.app)，帮助 [`https://bestcodex.app/help`](https://bestcodex.app/help)。账号、注册登录与用户数据统一收口到 Sub2API（`api.lumio.games`，不可变更）。

本仓库是 monorepo：桌面启动器、Claude Tab 能力来源与官网各自独立构建、互不引用。仓库路径仍可叫 `cchaven` / CC；用户看见的第二个 Tab 是 Claude。

## 仓库地图

| 路径 | 内容 |
| --- | --- |
| `codex/` | **BestCodex** 桌面启动器（Rust workspace + Tauri 2 + React 19），含旧静态官网 `codex/site/` 与产品文档 `codex/docs/` |
| `cchaven/` | Claude Tab 的能力来源（仓库仍可叫 **cchaven / CC**）：Claude Code 远端运行与双向同步——Go 控制面 `services/cchaven-control`、官网 `apps/web`、运营后台 `apps/admin`、Tauri 桌面端 `apps/desktop`、同步 agent |
| `web/` | 统一官网门户：`bestcodex.app`（总站 + 账号中心）、`cc.bestcodex.app`、`codex.bestcodex.app` 三站共享 UI |
| `.spec/` | Agent 规范与项目知识库（全仓单一权威，见 [`AGENTS.md`](AGENTS.md)） |

`codex/` 与 `cchaven/` 各自是独立的 Rust workspace / npm 工程，互不引用；构建与测试在各自目录内执行。

## 构建统一官网三站

```bash
cd web
npm ci
npm run build                              # 三站一起构建
npm run build --workspace @lumio/portal    # 或只构建一站（@lumio/cc / @lumio/codex 同理）
```

产物分别在 `web/apps/portal/dist/`、`web/apps/cc/dist/`、`web/apps/codex/dist/`，
都是静态 SPA。部署与域名切换见 [`docs/ops/`](docs/ops/README.md)。

## 域名与架构

| 域名 | 用途 |
| --- | --- |
| `bestcodex.app` | 用户可见站点：品牌首页、注册 / 登录 / 2FA、账户中心（对接 Sub2API）；帮助 `https://bestcodex.app/help` |
| `cc.bestcodex.app` | Claude 产品站（介绍 / 定价 / 下载；仓库仍可叫 CC） |
| `codex.bestcodex.app` | BestCodex / Codex 产品站（介绍 / 下载） |
| `api.lumio.games` | Sub2API：统一账号 / Key / 充值（存量桌面客户端硬编码，不可变更） |
| `api.cc.bestcodex.app` | Claude Tab 控制面 API（仓库仍可叫 CC；域名待最终确认，已写进代码默认值） |
| `lumio.games`、`cchaven.cn` | 旧域名，301 跳转到新站 |

Sub2API CORS 放行 `https://bestcodex.app`（及子域）与 DNS / 证书仍待运维，本仓不改生产。三站部署、DNS 切换与上线验收见跨产品运维手册 [`docs/ops/`](docs/ops/README.md)。

## 许可

- `codex/` 是 [`BigPizzaV3/CodexPlusPlus`](https://github.com/BigPizzaV3/CodexPlusPlus) 的 AGPL fork，许可见 [`codex/LICENSE`](codex/LICENSE) 与 [`codex/THIRD_PARTY_NOTICES.md`](codex/THIRD_PARTY_NOTICES.md)。
- `cchaven/` 与 `web/` 的许可以各自目录内声明为准。
- BestCodex 与 OpenAI、Anthropic 无从属、赞助或认可关系。

## 开发入口

- 跨产品运维（三站部署 / 域名切换 / 上线验收）：[`docs/ops/`](docs/ops/README.md)
- BestCodex：[`codex/README.md`](codex/README.md)、运维手册 [`codex/docs/ops/`](codex/docs/ops/README.md)
- Claude Tab 能力（仓库仍可叫 cchaven / CC）：[`cchaven/README.md`](cchaven/README.md)、运维手册 [`cchaven/docs/ops/`](cchaven/docs/ops/README.md)
- 统一官网开发：[`web/README.md`](web/README.md)
- Agent 协作规范：[`AGENTS.md`](AGENTS.md) → [`.spec/AGENTS.md`](.spec/AGENTS.md)
