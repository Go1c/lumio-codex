---
name: ops-release
description: 部署/打包/发版/后台维护入口——准备上线推送、打安装包、发 Release 或改运维流程时查
metadata:
  type: doc
  status: 已交付
---

# 运维与发布（索引）

权威操作手册在仓库的三份 `docs/ops/`，本文件只指路，不展开步骤。

| 手册 | 管什么 |
|------|--------|
| [根 `docs/ops/`](../../../docs/ops/README.md) | **跨产品**：门户 + BestCodex 产品站部署、域名与 DNS 切换、服务端前置项、上线验收 |
| [`codex/docs/ops/`](../../../codex/docs/ops/README.md) | Lumio Codex：本机编译、打内测包、发版、Sub2API 契约、旧官网下线 |
| [`cchaven/docs/ops/`](../../../cchaven/docs/ops/README.md) | CCHaven：控制面部署、运营后台、桌面端打包、存量用户迁移 |

## 何时查哪份

- 部署门户与 `bestcodex.app` 产品站、加 DNS 记录、配旧域名 301 → 根 `docs/ops/`
- 本机或 CI 编译、打内部包（未签名或 SignPath 已签的 Windows）、打 tag / GitHub Release、验证更新提醒、Microsoft Store unsigned MSIX → `codex/docs/ops/`（商店与双轨签名见 `codex/docs/ops/07-microsoft-store.md`）
- 起 PostgreSQL / 控制面 / 网关、建首个管理员、跑身份迁移 → `cchaven/docs/ops/`
- 对接或验收 `api.lumio.games`（Sub2API）→ `codex/docs/ops/04-backend.md` +
  根 `docs/ops/03-service-prerequisites.md`

## 硬事实（防漂移）

- 公开签名发布闸门未开前，不写 `latest.json`、不分发正式公开通道。内部指针仍是 `latest-internal.json`；Windows 在 SignPath token 可用时出已签名文件名，macOS 仍为 `-internal-unsigned`。
- **改产物命名会同时打断三处消费者**：下载区的匹配正则
  （`web/packages/ui/src/lib/releases.ts`）、命令行安装脚本
  （`web/apps/bestcodex/public/install.sh`、`install.ps1`）、以及各处文档里的
  `xattr -cr "/Applications/BestCodex.app"` 路径。脚本靠
  `latest-internal.json` + 同目录的 `SHA256SUMS.txt` 工作，两者缺一即中止安装；
  `src/__tests__/install-scripts.node.test.ts` 会把这些约定钉住。
- **Homebrew cask 暂时走不通**，不是没做，是被签名闸门挡着：Homebrew 自
  2026-09-01 起停止支持通不过 Gatekeeper 检查的 cask，并移除了
  `--no-quarantine`。未签名公证的包上不了 cask（自建 tap 也一样），所以命令行
  安装只能走自己的脚本。签名公证落地后再评估。
- Windows 两条分发轨并存：官网 / GitHub Release 用 NSIS + ZIP（SignPath 签 PE 和 setup）；Microsoft Store 用 unsigned MSIX，上架后微软重签。CI 额外产出 `LumioCodex-*-windows-x64-store-unsigned.msix`；`Identity.Name` / `Identity.Publisher` / `PublisherDisplayName` 仍为 Partner Center 占位。不得改 NSIS / 便携 ZIP 产物名，不得打开 `tauri.conf.json` 的 `bundle.active`，不得把 MSIX 接进 SignPath，也不得用第三方商店扒包 Action。步骤见 `codex/docs/ops/07-microsoft-store.md`。
- `api.lumio.games` 被存量桌面客户端硬编码，**不可变更**；账号 / 充值都在它那里。
- 桌面端生产常量以 `codex/crates/codex-plus-core/src/lumio/product.rs` 为准
  （`API_BASE_URL` 未变，`SITE_BASE_URL` 为 `https://bestcodex.app`）。
- 门户与产品站的域名与接口地址只在 `web/packages/ui/src/config.ts` 一处收敛。
- 运维步骤变更必须同步对应的 `docs/ops/`；触发场景变了要一并改本索引的 description。
- 安装包必须内嵌同步组件（本机 sidecar + 远端 linux-x86_64 组件），打包脚本与 CI 由
  `codex/scripts/sync-components/verify.mjs` 把关，缺组件必须构建失败，不得再出空壳包。
