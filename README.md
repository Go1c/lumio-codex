# Lumio Monorepo

Lumio 产品族的统一仓库：一个总门户（`lumiogame.com`）分发两个子产品，账号、注册登录与用户数据统一收口到 Sub2API（`api.lumio.games`）。

## 仓库地图

| 路径 | 内容 |
| --- | --- |
| `codex/` | **Lumio Codex**：Codex 桌面客户端（Rust workspace + Tauri 2 + React 19），含旧静态官网 `codex/site/` 与产品文档 `codex/docs/` |
| `cchaven/` | **CC避风港（CCHaven）**：Claude Code 远端运行与双向同步——Go 控制面 `services/cchaven-control`、官网 `apps/web`、运营后台 `apps/admin`、Tauri 桌面端 `apps/desktop`、同步 agent |
| `web/` | 统一官网门户：`lumiogame.com`（总站 + 账号中心）、`cc.lumiogame.com`、`codex.lumiogame.com` 三站共享 UI |
| `.spec/` | Agent 规范与项目知识库（全仓单一权威，见 [`AGENTS.md`](AGENTS.md)） |

`codex/` 与 `cchaven/` 各自是独立的 Rust workspace / npm 工程，互不引用；构建与测试在各自目录内执行。

## 域名与架构

| 域名 | 用途 |
| --- | --- |
| `lumiogame.com` | 总门户：品牌首页、注册 / 登录 / 2FA、账户中心（对接 Sub2API） |
| `cc.lumiogame.com` | CCHaven 产品站（介绍 / 定价 / 下载） |
| `codex.lumiogame.com` | Lumio Codex 产品站（介绍 / 下载） |
| `api.lumio.games` | Sub2API：统一账号 / Key / 充值（存量桌面客户端硬编码，不可变更） |
| `lumio.games`、`cchaven.cn` | 旧域名，301 跳转到新站 |

## 许可

- `codex/` 是 [`BigPizzaV3/CodexPlusPlus`](https://github.com/BigPizzaV3/CodexPlusPlus) 的 AGPL fork，许可见 [`codex/LICENSE`](codex/LICENSE) 与 [`codex/THIRD_PARTY_NOTICES.md`](codex/THIRD_PARTY_NOTICES.md)。
- `cchaven/` 与 `web/` 的许可以各自目录内声明为准。

## 开发入口

- Lumio Codex：[`codex/README.md`](codex/README.md)、运维手册 [`codex/docs/ops/`](codex/docs/ops/README.md)
- CCHaven：[`cchaven/README.md`](cchaven/README.md)、运维手册 [`cchaven/docs/ops/`](cchaven/docs/ops/README.md)
- 统一官网：`web/README.md`
- Agent 协作规范：[`AGENTS.md`](AGENTS.md) → [`.spec/AGENTS.md`](.spec/AGENTS.md)
