**中文** · [English](README.en.md)

# BestCodex

**一个启动器，两种工作方式。** 下载一次、登录一次，窗口里两个 Tab：

- **Codex** —— 零配置用上**官方 Codex**。不用填 Base URL、不用贴 API Key，登录后本机配置自动写好。不捆绑、不修改官方应用。
- **Claude** —— 把**官方 Claude Code** 跑在你自己的服务器上：独立环境、固定 IP、持久会话，文件与本机双向同步。

官网 [bestcodex.app](https://bestcodex.app) · 指南 [bestcodex.app/guides](https://bestcodex.app/guides) · 帮助 [bestcodex.app/help](https://bestcodex.app/help)

## 安装

支持 **macOS（Apple 芯片 / Intel）** 与 **Windows**，一个安装包覆盖两个 Tab。到 [bestcodex.app](https://bestcodex.app) 选择平台下载。

> **当前是未签名内测包。** macOS 会以隔离标记拦截，提示「已损坏，无法打开」——应用本身没坏。把它拖进「应用程序」后执行 `xattr -cr "/Applications/BestCodex.app"` 再打开；Windows 上 SmartScreen 会提示，核对来源后继续。详见 [未签名说明](https://bestcodex.app/help/unsigned)。

官方 Codex 应用**不捆绑**在安装包里，需要时另行安装。

## 与 Codex++ 的关系

本仓库的 `codex/` 是 [`BigPizzaV3/CodexPlusPlus`](https://github.com/BigPizzaV3/CodexPlusPlus)（Codex++）的 **AGPL-3.0 fork**，上游的贡献与许可完整保留。

两者定位不同，适合的人也不同：

| | Codex++（上游） | BestCodex |
|---|---|---|
| 面向 | 想深度改装 Codex 的进阶用户 | 想尽快把官方 Codex 用起来的人 |
| 侧重 | 供应商切换、协议转换、插件解锁、界面增强 | 零配置接入、账号与余额托管、开箱即用 |
| Claude Code | 不涉及 | 内置 Claude Tab：跑在你自己的服务器上，双向同步 |

需要供应商切换与深度增强，请直接用上游 Codex++。

## 仓库地图

本仓库是 monorepo，三块业务各自独立构建、互不引用：

| 路径 | 内容 |
| --- | --- |
| `codex/` | **BestCodex 桌面启动器**（Rust workspace + Tauri 2 + React 19）。Codex++ 的 AGPL fork，含产品文档 `codex/docs/` 与旧静态站 `codex/site/` |
| `cchaven/` | **Claude Tab 的能力来源**（目录名仍叫 cchaven / CC）：Claude Code 远端运行与双向同步——Go 控制面 `services/cchaven-control`、`apps/web`、运营后台 `apps/admin`、Tauri 桌面端 `apps/desktop`、同步 agent |
| `web/` | **官网**：`apps/bestcodex`（产品站 `bestcodex.app`）+ `apps/portal`（账号中心），两站共享 `packages/ui` 与 `packages/auth` |
| `.spec/` | Agent 规范与项目知识库（全仓单一权威，见 [`AGENTS.md`](AGENTS.md)） |

`codex/` 与 `cchaven/` 是各自独立的 Rust workspace / npm 工程，构建与测试都在自己目录内执行。

## 构建官网

```bash
cd web
npm ci
npm run build                                # 两站一起构建
npm run build --workspace @lumio/bestcodex    # 或只构建产品站
```

产物在 `web/apps/bestcodex/dist/` 与 `web/apps/portal/dist/`，都是静态站点。开发与收口命令见 [`web/README.md`](web/README.md)，部署见 [`docs/ops/`](docs/ops/README.md)。

## 域名与架构

| 域名 | 用途 |
| --- | --- |
| `bestcodex.app` | 营销 apex 与产品站：`/` `/codex` Codex 落地页、`/claude` Claude 落地页、`/help` 帮助中心 |
| `api.lumio.games` | Sub2API：统一账号 / Key / 充值（存量桌面客户端硬编码，**不可变更**） |
| `api.cc.bestcodex.app` | Claude Tab 控制面 API（目录名仍叫 CC） |
| `codex.bestcodex.app`、`cc.bestcodex.app` | 已退役的旧子域，301 到 apex |
| `lumio.games`、`cchaven.cn` | 旧域名，301 到新站 |

账号、注册登录与余额统一收口 Sub2API；产品站**不做自己的登录**，账号入口跳账号中心并带 `?next=` 回跳。产品站与账号中心在 apex 上的共存选择见 [ADR 0007](.spec/decisions/0007-bestcodex-apex-portal-coexistence.md)。

## 许可

- `codex/` 是 Codex++ 的 **AGPL-3.0-only** fork：许可见 [`codex/LICENSE`](codex/LICENSE)，第三方声明见 [`codex/THIRD_PARTY_NOTICES.md`](codex/THIRD_PARTY_NOTICES.md)。
- `cchaven/` 与 `web/` 目前未单独声明许可。
- BestCodex 是独立项目，与 **OpenAI、Anthropic 无从属、赞助或认可关系**。OpenAI、ChatGPT、Codex、Claude、Anthropic 为其各自所有者的商标。官方应用需单独安装。

## 开发入口

- 桌面启动器：[`codex/README.md`](codex/README.md) · 运维 [`codex/docs/ops/`](codex/docs/ops/README.md)
- Claude Tab 能力：[`cchaven/README.md`](cchaven/README.md) · 运维 [`cchaven/docs/ops/`](cchaven/docs/ops/README.md)
- 官网：[`web/README.md`](web/README.md)
- 跨产品运维（部署 / 域名切换 / 上线验收）：[`docs/ops/`](docs/ops/README.md)
- Agent 协作规范：[`AGENTS.md`](AGENTS.md) → [`.spec/AGENTS.md`](.spec/AGENTS.md)
