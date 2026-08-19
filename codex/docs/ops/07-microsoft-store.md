# 07 · Microsoft Store 与 Windows 双轨分发

两条 Windows 分发轨**并存，互不替换**。本页写清整套商店流程和签名责任；官网 / GitHub Release 的发版步骤仍以 [03-release.md](./03-release.md) 为准，SignPath Authenticode 政策仍以 [06-code-signing-policy.md](./06-code-signing-policy.md) 为准。决策见 [0011](../../../.spec/decisions/0011-windows-msix-store-scaffold.md)。

本仓只打 **unsigned MSIX** 并留下 Identity 占位。账号注册、Partner Center 提交、商店文案与密钥都归 **IT**，不进本仓库、不进 CI。

## 1. 两条轨对照

| | 轨 1 · 官网 / GitHub Release | 轨 2 · Microsoft Store |
|---|---|---|
| 产物 | NSIS `LumioCodex-<ver>-windows-x64-setup.exe`（或 `-setup-internal-unsigned.exe`）+ 便携 ZIP | `LumioCodex-<ver>-windows-x64-store-unsigned.msix` |
| 谁打包 | 现有 cargo 产物 + `LumioCodex.nsi` + `Compress-Archive` | 同一批 cargo 产物 + `scripts/installer/windows/msix/Pack-Msix.ps1` + Windows SDK `makeappx` |
| 谁签名 | SignPath Authenticode 签两个 PE 和 setup；ZIP 用已签 exe 重打 | **不走 SignPath**。交 Partner Center 后，商店上架时**微软重签 MSIX** |
| 分发 | S3 `latest-internal.json` / GitHub Release；公开 `latest.json` 闸门仍关 | Partner Center → Microsoft Store。本仓不注册商店账号 |
| 现在能不能当正式安装包 | 内部通道：有 SignPath token 时出已签名文件名；公开闸门未开 | 不能。unsigned MSIX 不能侧载给普通用户当正式包 |
| 改动边界 | **不要改**现有 SignPath / `release-assets` 门闸，不要改 `LumioCodex.nsi` | **不要**打开 `tauri.conf.json` 的 `bundle.active`，不要用第三方扒包 |

两条轨共用 `dist/windows/app` 里的 `lumio-codex.exe` 与 `lumio-codex-launcher.exe`，但签名和分发在分叉之后各走各的。

## 2. 轨 1：官网 / GitHub Release（现有路径，保持不动）

适用：受控内测、官网下载、GitHub Release。这是当前唯一对用户交付的 Windows 通道。

1. CI（`Internal unsigned build artifacts`）在 `codex/` 里 `cargo build --release`，把两个 exe 放到 `dist/windows/app/`。
2. **发布路径**（`publish` / `v*` tag / `workflow_dispatch`，且仓库有 `SIGNPATH_API_TOKEN`）时：
   - SignPath 先签两个 payload exe；
   - 用已签 exe 打便携 ZIP；
   - `makensis` 打 setup；
   - SignPath 再签 setup；
   - `Get-AuthenticodeSignature` 核三个 PE。
3. **PR 或没有 SignPath token** 时：产物带 `-internal-unsigned`，不提交 SignPath。
4. `uninstall.exe` 是用户机器上 NSIS 生成的，不签。
5. Authenticode 发行者是 **SignPath Foundation**，不是 Lumio。

完整步骤、Secrets、S3 指针、公开闸门见 [03-release.md](./03-release.md) 与 [06-code-signing-policy.md](./06-code-signing-policy.md)。

**不要改这条轨的 CI 签名逻辑。** 商店轨不得接进 SignPath artifact configuration，也不得让 SignPath 去签 `.msix`。

## 3. 轨 2：Microsoft Store（脚手架已有，Identity 未批）

### 3.1 本仓做什么

Tauri 2 没有原生 MSIX 目标，`bundle.active` 保持关闭。商店包沿用轨 1 已经 staged 的 cargo 产物，用 Windows SDK `makeappx` 打 **unsigned** MSIX。

本机（在 `codex/`，且已按 [01-local-build.md](./01-local-build.md) §5.3 备好 `dist/windows/app`）：

```powershell
./scripts/installer/windows/msix/Pack-Msix.ps1 -PackageVersion "1.2.46-internal-38"
```

输出：`dist/windows/LumioCodex-<PACKAGE_VERSION>-windows-x64-store-unsigned.msix`

CI 在 SignPath 之前打这份 MSIX，payload 是**未签**的 cargo exe。不要改成「先 SignPath 再打 MSIX」——商店包的信任来自上架后的微软重签，不来自 SignPath。

`PACKAGE_VERSION` → `Identity.Version` 由 `scripts/installer/windows/msix/map-package-version.mjs` 映射：

| `PACKAGE_VERSION` | `Identity.Version` |
|---|---|
| `1.2.46-internal-38` | `1.2.46.38` |
| `1.2.46` / `1.2.46-internal` | `1.2.46.0` |
| `1.2.46.7` | `1.2.46.7` |

映射不到前三位、或某一节超出 `0–65535`，脚本直接失败。商店每次更新必须升高 `Identity.Version`。

清单模板：`scripts/installer/windows/msix/AppxManifest.xml.template`。当前是 Desktop + `runFullTrust` 的 Win32 包装，入口 `lumio-codex.exe`，展示名 BestCodex。

### 3.2 Identity 占位（Partner Center 批下来之前不要改）

| 字段 | 当前占位 | 拿到后填哪里 |
|---|---|---|
| `Identity.Name` | `LumioGames.BestCodex` | `AppxManifest.xml.template` |
| `Identity.Publisher` | `CN=PLACEHOLDER-PARTNER-CENTER` | 同上，必须与 Partner Center 的 Publisher CN **逐字一致** |
| `PublisherDisplayName` | `Lumio` | 同上 |

密钥、Partner Center token、开发者账号、信用卡、Store ID **不进仓库、不进 CI**。占位 Publisher 交审会被拒，第一次真提交要等 IT 把三项填进模板。

### 3.3 账号注册（IT，本仓不注册）

[2025-05-19 Windows 博客](https://blogs.windows.com/windowsdeveloper/2025/05/19/microsoft-store-expands-opportunities-for-windows-app-developers/) 宣布：个人开发者免费注册，不用信用卡。入口：<https://aka.ms/microsoftstoredeveloper>。

- 账号、身份核验、Publisher 与名称预留、年龄分级、上架资料，全部归 IT。
- 本仓文档只写流程，不写账号、不代注册、不保存 Partner Center 截图里的密钥。
- 公司账号若仍收费或要 D-U-N-S，由 IT 自行判断；本仓不假设已经是公司账号。

### 3.4 Partner Center 提交流程（IT 操作）

Partner Center 还没批下来时，下面只作清单，不能在占位 Identity 上交审。

1. 用 IT 的开发者账号登录 [Partner Center](https://partner.microsoft.com/dashboard)。
2. **Apps and games → 新建产品 → MSIX 应用**，预留商店名。记下 Partner Center 给出的 `Identity.Name`、`Identity.Publisher`（`CN=…`）、`PublisherDisplayName`。
3. 回到本仓改 `AppxManifest.xml.template` 三处占位（单独 PR，仍然不要提交任何 token）。
4. 用对齐后的模板重新跑 `Pack-Msix.ps1`，得到新的 `*-store-unsigned.msix`。
5. Partner Center 对该产品 **Start submission**，按页填写（名称以控制台当时为准）：
   - Pricing and availability（市场、免费/收费、发布方式）
   - Properties（分类、硬件、支持联系）
   - Age ratings
   - Packages：上传 **unsigned** MSIX（不要先用 SignPath 或自购 EV 去签这个包）
   - Store listings：简介、截图、商店图标、隐私政策 URL（用产品站已上线的政策页）
   - 认证备注：说明这是 Win32 + `runFullTrust`，不捆绑官方 Codex / ChatGPT 应用
6. 提交认证。通过后微软对 MSIX **重签**，再按提交里的发布时间上架。
7. 上架后从 Partner Center 抄下 **Store ID / 商店链接**。本仓此时才具备接 badge 或讨论回传已签包的前提。

认证失败常见原因：Publisher CN 与账号不一致、`Identity.Version` 没有升高、缺少隐私政策、`runFullTrust` 未说明、包是用第三方扒来的而不是本仓 `makeappx` 打的。

### 3.5 上架之后（现在不做）

有商店链接之前，官网 / 产品站 **不要**接 Microsoft Store badge、Web Installer 或 `apps.microsoft.com` 深链。有链接后再单独立项改产品站，现在不要为此新增 web 应用。

上架后可选（评估后再做，**现在不加 workflow**）：

- 从 Partner Center 下载商店重签后的 MSIX，供核对或归档；
- 再评估要不要把已签 MSIX 挂到 GitHub Release（与轨 1 的 NSIS / ZIP 并列，不替换它们）。

不要现在加 [JasonWei512/Upload-Microsoft-Store-MSIX-Package-to-GitHub-Release](https://github.com/JasonWei512/Upload-Microsoft-Store-MSIX-Package-to-GitHub-Release)：它需要已有 store-id。上架后再说。

**禁止**使用 [store.rg-adguard.net](https://store.rg-adguard.net/) 或任何第三方「按商店链接扒包」的服务 / Action。商店包只从 Partner Center 或本仓 `makeappx` 来。

## 4. 签名责任（谁签什么）

| 文件 | SignPath Authenticode | 微软商店重签 |
|---|---|---|
| `lumio-codex.exe` / `lumio-codex-launcher.exe`（轨 1） | 是（发布路径有 token 时） | 否 |
| `LumioCodex-*-windows-x64-setup.exe` | 是（发布路径有 token 时） | 否 |
| 便携 ZIP | 不签包；内含已签 exe | 否 |
| `uninstall.exe` | 否（用户机器生成） | 否 |
| `LumioCodex-*-windows-x64-store-unsigned.msix` | **否** | 上架后由微软签 |

macOS Developer ID / 公证、公开闸门 `latest.json`、CCHaven 签名，都不在本页范围。

## 5. 禁止项（本仓 / CI）

- 不要改 `LumioCodex.nsi`，不要改 NSIS / 便携 ZIP 产物名。
- 不要改 SignPath 门闸、artifact configuration，或让 SignPath 签 MSIX。
- 不要打开 Tauri `bundle.active` 去「官方出 MSIX」。
- 不要把 Partner Center 账号、密钥、token 写入仓库或 workflow。
- 不要接 `store.rg-adguard.net`。
- 不要在没有 store-id 之前加「商店已签包回传到 GitHub Release」的 Action。
- 不要用商店轨替换轨 1，也不要把 unsigned MSIX 标成公开稳定安装包。

## 6. 检查清单（复制用）

**现在（脚手架 / 每次 Windows CI）**

- [ ] Windows artifact 里有 `LumioCodex-<ver>-windows-x64-store-unsigned.msix`
- [ ] 同一次 run 的 NSIS / ZIP 仍按轨 1 产出（签名逻辑未动）
- [ ] 模板仍是三处占位，或已改成 Partner Center 批下来的值（只能是其中一种）
- [ ] 无商店账号 / token 进仓

**Partner Center 批下来之后（IT + 单独 PR）**

- [ ] 模板三处 Identity 已换成控制台原值
- [ ] 重新打 unsigned MSIX 再交审，而不是改旧包里的 XML
- [ ] 隐私政策 URL、截图、年龄分级已备齐
- [ ] 认证通过后确认商店页能打开；再另立项考虑 badge / 回传已签包

## 7. 相关

- [03-release.md](./03-release.md) — 版本、S3、GitHub Release、公开闸门
- [06-code-signing-policy.md](./06-code-signing-policy.md) — SignPath 公开政策（轨 1）
- [01-local-build.md](./01-local-build.md) §5.4 — 本机打 unsigned MSIX
- [0011 · Windows 商店包另开 unsigned MSIX 轨](../../../.spec/decisions/0011-windows-msix-store-scaffold.md)
