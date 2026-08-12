---
name: workflow
description: 开发工作流——分支/提交/合并·PR 与知识同步义务;动手改代码、开 PR 前查
metadata:
  type: doc
  status: 已交付
---

# 开发工作流（分支 / 提交 / 合并）

> 本文是“开发这件事**怎么做**”的手册。Agent 之间**怎么协作**（拆解 → 实现 → reviewer 对抗审查 → 收口）在 [`AGENTS.md`](../../AGENTS.md) 的「调度核心」与「编码约定」里，不在这里。
> “禁止碰什么”的硬性护栏在 [`rules/system.md`](../../rules/system.md)；本文只描述流程，遇到护栏处**引用**它，不重复定义。

## 分支策略（**落地必填**）

- `origin`：`https://github.com/Go1c/lumio-codex.git`。持续集成分支为 **`publish`**（发版与内部 CI 主线）。
- `upstream`（可选）：`https://github.com/BigPizzaV3/CodexPlusPlus.git`，用于偶发同步上游；合并前遵守确认规则。
- 功能分支从 `publish` 拉出，命名 `feat/…` / `fix/…`；完成后 PR 回 `publish`。
- **部署 / 打包 / Release 步骤**不在本文展开，一律以 [`docs/ops/`](../../../docs/ops/README.md) 为准（知识索引见 [`ops-release.md`](./ops-release.md)）。

## 提交规范（通用）

- 格式：`type(scope): subject`，例如 `feat(agents): 新增 reviewer`、`fix(skills): 修复 TDD 步骤`。
- 常用 type：`feat` / `fix` / `refactor` / `chore` / `docs` / `ci`。scope 可省略。
- **一次提交只做一类事**；文档、脚手架、功能、测试修复不混在一起。
- 提交前自检：验证命令通过（见 `AGENTS.md`「收口门槛」与 `rules/system.md`）、无调试残留、知识已同步（见下节）。
- 机器兜底：Claude Code 宿主经入库的 `.claude/settings.json` hooks 在 `git commit` 前自动跑结构校验，未过即阻断（known gap：仅 Claude Code 生效，Codex 等宿主无机器兜底，依赖上一条自检自觉执行；「reviewer 通过前不得提交」机器不可判，同属自觉项，红线见 `rules/system.md`）。

## 合并 / PR 流程（**落地必填**）

- 全栈功能完成并通过收口门槛后，目标是向上游主仓提交 PR。
- PR 必须包含 Summary、改动清单、验证命令与关键输出、known gaps，并说明兼容性与 opt-in 边界。
- push、创建 PR、合并等对外动作必须遵守 [`rules/system.md`](../../rules/system.md) 的确认要求。

## 改动完成 = 知识已同步

一处改动只有在**知识沉淀完成**后才算 Done：用 `spec-steward` 技能更新对应 `knowledge/` 文档、`status` 与 `knowledge/README.md` 导航（交付历史在 git，不进文档）。豁免口径与 `AGENTS.md`「编码约定」的交付标准一致：纯修复 / 机械套用既有模式可豁免，但**豁免必须在交回物里声明**，不得静默跳过。

## 相关

- 验收与测试：[`testing.md`](./testing.md)
- 注释与命名：[`code-style.md`](./code-style.md)
- 护栏（禁止项）：[`rules/system.md`](../../rules/system.md)
- 沉淀方法：[`skills/spec-steward`](../../skills/spec-steward/SKILL.md)
