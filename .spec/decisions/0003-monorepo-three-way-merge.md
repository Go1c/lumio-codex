# 0003 · 双仓合并为 codex/ + cchaven/ + web/ 三块并列的 monorepo，subtree 保留历史

- 日期：2026-08-13
- 状态：生效

## 背景

Lumio 产品族原本是两个独立仓库：Lumio Codex 桌面客户端（Rust workspace + Tauri 2 + React）
与 CC避风港全栈仓（Go 控制面 + 官网 + 运营后台 + Tauri 桌面端 + 同步 agent，原 FNS-workspace）。

两个产品要合成「一个门户 + 两个子产品 + 一套账号」的形态（见 ADR-0002），
统一官网必然同时引用两边的文案与契约，账号收口也要求两边的运维口径一致。
分仓意味着跨仓 PR、跨仓对齐域名与契约、以及两份互相打架的运维手册。

技术选项有三个：合成单一构建体系的 monorepo（统一 workspace / 统一依赖）、
三块并列各自独立构建的 monorepo、或维持分仓靠文档对齐。

## 决策

合并为**一个仓库、三块并列**，各自独立构建、互不引用：

- `codex/`：原 lumio-codex 仓整体下沉（Rust workspace + Tauri 2 + React，含旧静态官网
  `codex/site/` 与运维手册 `codex/docs/ops/`）。
- `cchaven/`：原 FNS-workspace 以 **git subtree** 合入，**保留完整提交历史**
  （合并提交 `Add 'cchaven/' from commit d08f164…`），运维手册 `cchaven/docs/ops/`。
- `web/`：新建的统一官网门户，npm workspaces——`packages/ui`、`packages/auth`
  与 `apps/portal` / `apps/cc` / `apps/codex` 三个 Vite + React SPA。

配套约定：

- **不建统一构建体系**：Rust 与 Go 的构建、测试在各自目录内执行；`web/` 是独立的 npm
  workspaces，只以**只读**方式引用另两块的文案与契约，不 import 其代码、不改其文件。
- **文档分层**：功能级设计与计划落所属产品目录的 `docs/specs/` 与 `docs/plans/`；
  跨产品的仓库级文档落根 `docs/`（三站部署与域名切换即根 `docs/ops/`）。
- `.spec/` 是全仓单一的 Agent 规范与知识库落点，不按产品分叉。

## 后果

- **历史可追溯**：subtree 保住了 CC 侧的逐提交历史，`git log` / `git blame` 仍然有效；
  代价是仓库体积与提交图变复杂，合并提交后的历史里有两条根。
- **上游同步成本**：`codex/` 是 `BigPizzaV3/CodexPlusPlus` 的 AGPL fork，下沉一层目录后，
  从上游拉取变更需要额外处理路径前缀。
- **单仓无统一构建 = 需要纪律**：三块之间没有工具强制隔离，只有约定。
  跨块引用一旦出现，工具链不会报错，只能靠评审与本 ADR 拦。
- **CI 需按路径分流**：全仓跑 Rust + Go + 三套前端的成本过高，工作流应按改动路径挑执行。
- **收口门槛按目录分别执行**：Rust 在 `codex/` 或 `cchaven/` 跑 `cargo fmt --check` 与
  `cargo test`，Go 在 `cchaven/services/cchaven-control` 跑 `go vet ./...` 与 `go test ./...`，
  前端在所属工程跑 `npm run check` / `npm test` / 构建，`.spec/` 改动跑 spec-lint。
- **运维手册一分为三**：产品内的部署 / 打包 / 发版留在各自 `docs/ops/`，跨产品的三站与域名
  归根 `docs/ops/`。边界不清就会出现两份互相矛盾的步骤，因此每份手册都必须写明「哪些事不归我管」。
