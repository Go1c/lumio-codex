# Contributing to Lumio Codex

感谢贡献。本仓库是 Lumio Codex 桌面客户端与官网源码（AGPL-3.0-only），fork 自 CodexPlusPlus。

## 开发与打包

完整步骤见运维手册，勿只依赖本页摘要：

- **[docs/ops/01-local-build.md](docs/ops/01-local-build.md)** — 环境、编译、测试、打内部安装包  
- **[docs/ops/README.md](docs/ops/README.md)** — 部署 / 发版总览  

摘要：

```bash
git clone https://github.com/Go1c/lumio-codex.git
cd lumio-codex
git checkout publish

cd apps/codex-plus-manager
npm ci && npm run check && npm test && npm run vite:build
cd ../..
cargo test -p codex-plus-core --lib lumio
cargo test -p codex-plus-manager
```

## 分支与 PR

- 持续集成分支：`publish`  
- PR 请包含：Summary、验证命令与结果、known gaps  
- 推送 / 合并 / 公开发布遵守仓库确认规则（见 `.spec/rules/system.md`）  
- 改 `.spec/` 后运行：`node .spec/tools/spec-lint.mjs`  

## 发版

见 **[docs/ops/03-release.md](docs/ops/03-release.md)**。公开签名包在 CI `Public release gate` 打开之前不要当作稳定版分发。

## 代码风格

- Rust：`cargo fmt`  
- 前端：与现有 Lumio 壳一致；用户可见文案避开禁词（见 `shell-copy.test.ts`）  
- 知识沉淀：功能行为进 `.spec/knowledge/`；运维步骤进 `docs/ops/`  

## 报告问题

使用 GitHub Issues。请勿在 Issue 中粘贴 token、API Key、`.env` 或签名证书。
