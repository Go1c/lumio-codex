# 0011 · Windows 商店包另开 unsigned MSIX 轨，不改 NSIS / ZIP / Tauri bundle

- 日期:2026-08-18
- 状态:生效

> 本条从 `dev` 的 0007 改号并入 `publish`。`publish` 上 0007 已是 bestcodex.app apex 共存，故本决策记为 0011。

## 背景

桌面客户端现有 Windows 分发是 cargo release 产物 + 便携 ZIP + NSIS。Tauri 2 没有原生 MSIX 目标，`tauri.conf.json` 里 `bundle.active=false`。需要一条能在 `windows-latest` 上调用 Windows SDK `makeappx` 出包的商店轨脚手架，同时 Partner Center 的 Identity 尚未批下来。

两条分发必须写清签名责任：官网 / GitHub Release 已经用 SignPath Authenticode 签 PE 和 setup；商店上架后是微软重签 MSIX。两件事不能并成一条 CI 签名链。

## 决策

- 沿用 `dist/windows/app` 的现有 cargo 产物，用 `scripts/installer/windows/msix/Pack-Msix.ps1` + `AppxManifest.xml.template` 打 unsigned MSIX。
- 产物名固定为 `LumioCodex-<PACKAGE_VERSION>-windows-x64-store-unsigned.msix`。`PACKAGE_VERSION` 如 `1.2.46-internal-38` 映射为 Identity.Version `1.2.46.38`，映射不了第四位则 `1.2.46.0`。
- Identity.Name / Identity.Publisher / PublisherDisplayName 先占位（`LumioGames.BestCodex`、`CN=PLACEHOLDER-PARTNER-CENTER`、`Lumio`），密钥与 Partner Center 账号不入库。
- **两条 Windows 分发轨并存，互不替换：**
  1. 官网 / GitHub Release：NSIS setup.exe + 便携 ZIP。SignPath Authenticode 签两个 PE 和 setup（现有 CI，不改签名逻辑）。
  2. Microsoft Store：本仓 cargo 产物 + `makeappx` 打 unsigned MSIX，交 Partner Center。商店上架后微软重签 MSIX。这不是 SignPath 的活。
- 个人开发者账号由 IT 在 <https://aka.ms/microsoftstoredeveloper> 注册（[2025-05-19 Windows 博客](https://blogs.windows.com/windowsdeveloper/2025/05/19/microsoft-store-expands-opportunities-for-windows-app-developers/)：个人开发者免费、不用信用卡）。本仓不注册、不写账号。
- 不打开 Tauri bundle，不加从商店扒 MSIX 的第三方 Action（含 `store.rg-adguard.net`），现在不加把商店已签包回传到 GitHub Release 的 workflow，不改 `LumioCodex.nsi`，不改 SignPath / `release-assets` 门闸，不改 NSIS / ZIP 产物名。

## 后果

- CI 在 SignPath 之前多一步打 MSIX，Windows artifact 多一个 unsigned msix；商店签名与真实 Publisher CN 仍待 Partner Center。
- 未签名 MSIX 不能侧载给普通用户当正式安装包。
- 上架后可选：从 Partner Center 下载商店重签包，或再评估把已签 MSIX 挂到 GitHub Release。有商店链接之前不接 Web Installer / `apps.microsoft.com` badge。
- 操作手册：[07-microsoft-store.md](../../codex/docs/ops/07-microsoft-store.md)。轨 1 签名政策：[06-code-signing-policy.md](../../codex/docs/ops/06-code-signing-policy.md)。发版：[03-release.md](../../codex/docs/ops/03-release.md)。

## 相关

- [07-microsoft-store.md](../../codex/docs/ops/07-microsoft-store.md) — 整套商店流程与双轨签名
- [06-code-signing-policy.md](../../codex/docs/ops/06-code-signing-policy.md) — SignPath Authenticode（轨 1）
- [03-release.md](../../codex/docs/ops/03-release.md) — 版本、Release、公开闸门
