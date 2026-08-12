---
name: lumio-account-and-home
description: Lumio 桌面账户与首页：注册/登录/2FA、provisioning、在线离线首页、修复与设置——改账户壳或启动编排时查
metadata:
  type: doc
  status: 已交付
---

# Lumio 桌面账户与首页

在现有 Lumio 桌面壳上落地注册、登录（含 2FA）、自动配置、首页（在线 / 离线）、needs-repair 与设置，对接 Sub2API 认证，并完成官方 Codex 配置接管与无注入启动。

## 背景 / 目标

- 用户可见表面只讲账户、余额、启动与本机偏好；不暴露 provider / base url / API Key 等内部概念。
- 服务暂不可用时，已登录且本机配置健康的用户仍可启动官方 Codex（离线降级态）。
- 绝不静默覆盖用户本机 Codex 配置；冲突必须进修复页。

## 设计

- **设计面**：状态机阶段见架构规格；错误码六域 + `UNKNOWN`；凭据本期落本机 owner-only 文件（ADR-0001），非系统钥匙串。
- **交互面**：文案与页面结构以 UX 交互规格为准；恢复动作按整文件回滚诚实描述；支付 / 开机启动 / 自动更新 / 遥测本期禁用并附说明。
- **实现面**：
  - Rust：`crates/codex-plus-core/src/lumio/`（api / credentials / secret_file / session / config_takeover / account / launch）
  - Tauri：仅 `lumio_` 命令白名单；秘密不跨 IPC
  - 前端：`apps/codex-plus-manager/src/LumioApp.tsx` 的 `planStartup` 负责探活 + 接管健康检查后再决定 provisioning / offline-ready / needs-repair
  - 配置接管以**快照存在性**判定首次，不以 manifest；敏感文件经 `secret_file::write_secret` 创建即 0600

## 待解决

- 字段级配置恢复（相对整文件回滚）
- 系统凭据库替换本地文件（需新依赖，另开 ADR）
- 安全支付交接（一次性 handoff token）；当前为打开 `https://api.lumio.games/purchase`
- 已知坑：`provision` 步骤 payload 若漏 `account`，前端 `undefined !== null` 会推进假账户并在首页读 `email` 黑屏；IPC 侧用 `normalizeOptionalAccount`，UI 用 truthy 守卫
- 真实遥测上报、开机启动、签名后的自动安装更新
- 登录后 provisioning 路径可再补一次接管冲突检查（启动有凭据路径已拦）

## 官网与更新提醒

- 可部署静态站：仓库根目录 [`site/`](../../../codex/site/)（`lumio.games`）
- 更新提醒：`lumio_check_update` 对照 GitHub Releases latest，首页横幅引导打开下载页（不自动安装）

## 相关

- [架构设计](../../../codex/docs/specs/2026-08-11-lumio-codex-branded-client-design.md)
- [交互设计](../../../codex/docs/specs/2026-08-12-lumio-ux-interaction-design.md)
- [实现计划](../../../codex/docs/plans/2026-08-12-lumio-account-and-home.md)
- [ADR-0001 凭据本地文件](../../decisions/0001-lumio-credentials-local-file.md)
- 可点击原型：`prototypes/lumio-ux/`
