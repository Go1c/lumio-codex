# 0007 · Windows 商店包另开 unsigned MSIX 轨，不改 NSIS / ZIP / Tauri bundle

- 日期:2026-08-18
- 状态:生效

## 背景

桌面客户端现有 Windows 分发是 cargo release 产物 + 便携 ZIP + NSIS。Tauri 2 没有原生 MSIX 目标，`tauri.conf.json` 里 `bundle.active=false`。需要一条能在 `windows-latest` 上调用 Windows SDK `makeappx` 出包的商店轨脚手架，同时 Partner Center 的 Identity 尚未批下来。

## 决策

- 沿用 `dist/windows/app` 的现有 cargo 产物，用 `scripts/installer/windows/msix/Pack-Msix.ps1` + `AppxManifest.xml.template` 打 unsigned MSIX。
- 产物名固定为 `LumioCodex-<PACKAGE_VERSION>-windows-x64-store-unsigned.msix`。`PACKAGE_VERSION` 如 `1.2.46-internal-38` 映射为 Identity.Version `1.2.46.38`，映射不了第四位则 `1.2.46.0`。
- Identity.Name / Identity.Publisher / PublisherDisplayName 先占位（`LumioGames.BestCodex`、`CN=PLACEHOLDER-PARTNER-CENTER`、`Lumio`），密钥与 Partner Center 账号不入库。
- 不打开 Tauri bundle，不加从商店扒 MSIX 的第三方 Action，不改 `LumioCodex.nsi`，不改 SignPath / `release-assets` 门闸，不改 NSIS / ZIP 产物名。

## 后果

- CI 在 staging 之后多一步打 MSIX，Windows artifact 多一个 msix；商店签名与真实 Publisher CN 仍待 Partner Center。
- 未签名 MSIX 不能侧载给普通用户当正式安装包。
