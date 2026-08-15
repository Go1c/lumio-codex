---
name: lumio-account-and-home
description: Lumio 桌面账户与首页：注册/登录/2FA、provisioning、在线离线首页、修复与设置、本机缺失时首次安装官方桌面应用——改账户壳或启动编排时查
metadata:
  type: doc
  status: 已交付
---

# Lumio 桌面账户与首页

在现有 Lumio 桌面壳上落地注册、登录（含 2FA）、自动配置、首页（在线 / 离线）、needs-repair 与设置，对接 Sub2API 认证，完成官方 Codex 配置接管与无注入启动；本机没有官方桌面应用时，首页可在 Lumio 内首次安装原样官方包后再启动。

## 背景 / 目标

- 用户可见表面只讲账户、余额、启动与本机偏好；不暴露 provider / base url / API Key 等内部概念。
- 服务暂不可用时，已登录且本机配置健康的用户仍可启动官方 Codex（离线降级态）。
- 绝不静默覆盖用户本机 Codex 配置；冲突必须进修复页。

## 设计

- **设计面**：状态机阶段见架构规格；错误码七域（`AUTH_` / `ACCOUNT_` / `KEY_` / `SERVICE_` / `CODEX_` / `PAYMENT_HANDOFF_` / `UPDATE_`）+ `UNKNOWN`；凭据本期落本机 owner-only 文件（ADR-0001），非系统钥匙串。
  - 网关面（`/v1/*`，无管理 API 信封）错误体是 `{"code","message"}`：`api.rs` 的 `gateway_error_reason` 按 `code` → `reason` → `error.code` 解析后交给 `normalize_reason`；余额不足（`INSUFFICIENT_BALANCE`）归 `ACCOUNT_INSUFFICIENT_BALANCE`（用户可操作，禁止伪装成宕机），空模型目录归 `SERVICE_MODEL_CATALOG_EMPTY`（D-16）。
- **交互面**：文案与页面结构以 UX 交互规格为准；恢复动作按整文件回滚诚实描述；支付 / 开机启动 / 自动更新 / 遥测本期禁用并附说明。首页主按钮：有应用为「启动 Codex」；无应用且在线为「安装并启动官方 Codex」；安装中为「正在安装官方 Codex…」加阶段（下载 / 校验 / 安装）；无应用且离线禁用并注明「安装官方应用需要网络」。设置里的重新检测 / 手动选择只留给装过但路径异常的人。
- **实现面**：
  - Rust：`codex/crates/codex-plus-core/src/lumio/`（api / credentials / secret_file / session / config_takeover / account / launch / official_app_install）
  - 首次安装：`official_app_install/` 按计划 → 下载 → 校验 → Windows / macOS 适配切开；默认镜像，官方直链 / FE3 备用；进度可轮询，不堵 UI
  - Tauri：仅 `lumio_` 命令白名单（含 `lumio_install_official_app` / `lumio_official_app_status` / `lumio_cancel_official_app`）；秘密不跨 IPC
  - 前端：`codex/apps/codex-plus-manager/src/LumioApp.tsx` 的 `planStartup` 负责探活 + 接管健康检查后再决定 provisioning / offline-ready / needs-repair；安装进度挂在 ready 首页，不新增全屏阶段
  - 配置接管以**快照存在性**判定首次，不以 manifest；敏感文件经 `secret_file::write_secret` 创建即 0600
  - 启动仍走 `launch::launch_official_codex`（macOS `open -a`，Windows 直接拉官方可执行文件），无注入 / CDP

## 待解决

- 字段级配置恢复（相对整文件回滚）
- 系统凭据库替换本地文件（需新依赖，另开 ADR）
- 安全支付交接（一次性 handoff token）；当前为打开 `https://api.lumio.games/purchase`
- 已知坑：`provision` 步骤 payload 若漏 `account`，前端 `undefined !== null` 会推进假账户并在首页读 `email` 黑屏；IPC 侧用 `normalizeOptionalAccount`，UI 用 truthy 守卫
- 真实遥测上报、开机启动、签名后的自动安装更新
- 登录后 provisioning 路径可再补一次接管冲突检查（启动有凭据路径已拦）
- 官方应用镜像清单尚未经 Sub2API `GET /api/v1/desktop/config` 转发，源常量仍在客户端 `sources.rs`（ADR-0005）
- 首次安装的 Windows / macOS 真机验收（干净机下载、校验、安装并启动）尚未在本环境跑通

## 官网与更新提醒

- 可部署静态站：[`codex/site/`](../../../codex/site/)（`lumio.games`）
- 更新提醒：`lumio_check_update` 对照 GitHub Releases latest，首页横幅引导打开下载页（不自动安装）

## 相关

- [架构设计](../../../codex/docs/specs/2026-08-11-lumio-codex-branded-client-design.md)
- [交互设计](../../../codex/docs/specs/2026-08-12-lumio-ux-interaction-design.md)
- [实现计划](../../../codex/docs/plans/2026-08-12-lumio-account-and-home.md)
- [ADR-0001 凭据本地文件](../../decisions/0001-lumio-credentials-local-file.md)
- [ADR-0005 首次安装官方桌面应用](../../decisions/0005-lumio-first-official-app-install.md)
- 可点击原型：`prototypes/lumio-ux/`
