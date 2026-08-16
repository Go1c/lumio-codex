# 0007 · 产品站占用 bestcodex.app 营销 apex，门户本期内保持独立部署

- 日期：2026-08-17
- 状态：生效

## 背景

`web/apps/codex` 与 `web/apps/cc` 原先按子域拆成两个产品站，`siteUrl("codex")` / `siteUrl("cc")` 默认指向 `codex.bestcodex.app` 与 `cc.bestcodex.app`。设计已锁定为一个产品站：营销 apex 是 `bestcodex.app`，顶栏 `[ Codex | Claude ]` 在站内换页。

门户（`web/apps/portal`）与产品站目前都默认写到同一 apex。门户仍是 Lumio 账号中心，本期内部署 / CI 托管不得改动。两边都宣称 `bestcodex.app` 时，路径会撞车。

## 决策

1. **产品站是本期意图中的营销 apex**：`bestcodex.app` + `/codex` `/claude` `/help`。帮助规范 URL 维持 `https://bestcodex.app/help`（`helpCanonicalUrl` 已是此值）。
2. **门户保持独立应用**（`web/apps/portal`）。本期内不改门户部署、不把整站改成 BestCodex、不在产品站做 `bestcodex.app/login`。
3. **路径碰撞按用途切开，不靠改托管解决**：门户继续使用 `/login` `/signup` `/account` `/help`；产品站使用 `/` `/codex` `/claude` `/help`。apex 上的 `/help` 以产品站帮助中心为准。
4. **旧子域 301 是运维事项**，本仓不写 `codex.bestcodex.app` / `cc.bestcodex.app` 的跳转配置。
5. **环境变量仍是本地联调逃生舱**：`VITE_PORTAL_URL`、`VITE_CODEX_URL`、`VITE_CC_URL`、`VITE_ROOT_DOMAIN` 可把门户与产品站拆到不同端口；未覆盖时 `siteUrl("codex")` / `siteUrl("cc")` 默认是单站路由 `https://bestcodex.app/codex` 与 `https://bestcodex.app/claude`。

## 后果

- 生产 DNS 仍把门户指到 apex 时，门户链到 `https://bestcodex.app/codex` 会打到门户自己的 404，直到运维把营销 apex 切到产品站。这是已知的共存窗口，不在本期用改门户部署来抹平。
- 本地开发必须用环境变量把两站拆开，不能再假设三个子域各占一个静态产物。
- 历史 ADR 与旧计划标题不改写。
