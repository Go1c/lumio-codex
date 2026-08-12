---
name: testing
description: 测试与验收——测试分层政策、TDD 时机、验收 DoD 与验证证据;实现功能/修 bug 时查
metadata:
  type: doc
  status: 已交付
---

# 测试与验收（含 TDD 政策）

> 本文定**政策**（测什么、何时测、怎么算过）；“先写失败测试再实现”的**方法**在技能 [`skills/test-driven-development`](../../skills/test-driven-development/SKILL.md)。

## 测试分层（通用政策）

- **单元测试**：默认层，随项目验证命令（`AGENTS.md`「收口门槛」）每次跑，快、无外部依赖。
- **集成测试**（真库 / 真服务）：显式触发，不进默认验证命令，保持收口快。
- **端到端 / E2E**：显式触发；关键主链路至少一条。

## 何时走 TDD

- 必须走：新功能、修 bug（先写能复现的失败测试，修完留作回归测试）、改无测试保护的关键逻辑。
- 可不走：纯文档改动、一次性脚本。豁免在交回物里声明。
- 写测试、加 mock、想给生产类加 test-only 方法前，先查反模式清单：[`testing-anti-patterns.md`](../../skills/test-driven-development/testing-anti-patterns.md)——测 mock 行为、test-only 方法入生产、不理解依赖就 mock、不完整 mock，一律禁止。

## 验证证据

形式要求以 `AGENTS.md`「交回物格式」为单一权威——「已通过」三个字不是证据。

## 验收标准（Definition of Done）

- [ ] 收口门槛命令全绿（种子默认 `node .spec/tools/spec-lint.mjs` + `node --test .spec/tools/spec-lint.test.mjs`）。
- [ ] 新增 / 修改行为有测试覆盖；bug 修复留有回归测试。
- [ ] 无 lint / 类型错误、无调试残留。
- [ ] 相关知识文档已更新（见 [`workflow.md`](./workflow.md)）。

## 项目测试栈与命令（**落地必填**）

- Rust 默认验证使用 `cargo test`，格式检查使用 `cargo fmt --all -- --check`，不得另起工具链。
- Rust 测试沿用上游 `#[test]` + `tempfile` 风格（参考 `crates/codex-plus-core/tests/relay_config.rs`）。配置生成相关测试读取 `config.toml` 文本断言，例如 `assert!(config.contains("model_catalog_json"))`。
- 改变行为必须同步新增或更新对应测试；bug 修复必须保留回归测试。
- 前端改动在 `apps/codex-plus-manager/` 使用 `npm run check`、`npm test` 与 `npm run vite:build`；不得为运行验证擅自安装依赖。
- `.spec/` 框架验证使用 `node .spec/tools/spec-lint.mjs`，自测使用 `node --test .spec/tools/spec-lint.test.mjs`。
