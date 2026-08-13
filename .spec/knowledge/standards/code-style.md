---
name: code-style
description: 代码与文档风格——语言约定、命名、注释原则、生成物纪律;写代码/建文档时查
metadata:
  type: doc
  status: 已交付
---

# 代码与文档风格

> 能交给工具（formatter / linter）强制的，优先交给工具；本文只写工具管不了、需要人 / Agent 判断的部分。

## 语言与文件命名（通用）

- **规范主体使用中文**（`.spec/` 下全部文档）；例外：根 `CLAUDE.md`（宿主入口惯例）与 `skills/` 下允许英文技能文档（中英以该技能既有语言为准，不混写）。落地项目若改用其他语言，需全仓一致并同步 `.spec/tools/spec-lint.mjs` 里的中文枚举值。
- 文件与目录命名一律 **kebab-case**；agent 文件 `<name>.agent.md`、skill 目录 `skills/<name>/`、ADR `NNNN-<slug>.md`。

## 注释原则（通用）

- 注释只写**代码表达不了的约束**（为什么这样做、边界条件、外部依赖的坑）。
- 不写「改动说明」式注释（改了什么、为什么正确）——那是给评审人的话，进交回物或提交信息，不进代码。
- 注释密度、命名、习语向**周边既有代码**看齐。

## 生成物纪律（通用）

- 生成物不得手改，只能经生成源与生成命令更新，并与生成源一起提交（红线见 [`rules/system.md`](../../rules/system.md)）。

## 语言 / 框架特定风格（**落地必填**）

- 对话与项目规范使用中文；代码标识符可用英文，注释尽量使用中文。
- Rust workspace 使用 edition 2024，遵循 `cargo fmt` 与上游既有 Rust 风格；React + TypeScript 代码遵循 `codex/apps/codex-plus-manager/` 周边既有写法，不另立平行模式。
- 改动保持隔离且 opt-in；上下文窗口功能不得破坏现有 per-profile 单值行为。
- 不为当前需求顺手重构，也不引入任务外依赖。
- `.spec/tools/spec-lint.mjs` 使用 Node 内置模块、ESM 且零第三方依赖；修改框架文件时保持这一边界。
