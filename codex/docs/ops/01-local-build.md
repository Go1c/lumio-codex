# 01 · 本地编译与打包

## 1. 环境要求

| 工具 | 版本 / 说明 |
|------|-------------|
| Node.js | **22**（与 CI 一致） |
| npm | 随 Node 自带；本项目用 npm，不用 pnpm |
| Rust | stable（edition 2024 workspace） |
| 平台依赖 | [Tauri 2 前置条件](https://v2.tauri.app/start/prerequisites/)（macOS Xcode CLT；Windows WebView2） |
| 官方 Codex | 端到端手测时需已安装官方桌面应用 |
| Windows 打安装包 | NSIS（`makensis`）；CI 用 `choco install nsis` |
| Windows 打 MSIX | Windows SDK `makeappx`；CI 在 `Windows Kits\10\bin\*\x64\makeappx.exe` 查找 |
| macOS 打 DMG | 自带 `hdiutil` |

克隆：

```bash
git clone https://github.com/LumioGames/lumio-codex.git
cd lumio-codex
git checkout publish   # 当前持续集成分支
```

## 2. 前端依赖与校验

```bash
cd apps/codex-plus-manager
npm ci
npm run check          # tsc
npm test               # Node 原生 test runner
npm run vite:build     # 产出 dist/，供 Tauri 嵌入
```

开发热重载（需本机平台依赖）：

```bash
cd apps/codex-plus-manager
npm run vite:dev       # 终端 A：Vite :1420
npm run dev            # 终端 B：tauri dev（会起桌面窗）
```

## 3. Rust 校验（推荐范围）

全量 `cargo test -p codex-plus-core` 可能撞上**遗留** launcher lifecycle 超时（与 BestCodex 账户功能无关）。发版前用：

```bash
# 在仓库根目录
cargo fmt --all -- --check
cargo test -p codex-plus-core --lib lumio
cargo test -p codex-plus-manager
cargo check -p codex-plus-manager -p codex-plus-launcher
```

需要安装器相关测试时再加：

```bash
cargo test -p codex-plus-core --test installers
```

Agent 规范校验（改过 `.spec/` 时必跑）：

```bash
node .spec/tools/spec-lint.mjs
```

## 4. 本机一键桌面构建（Tauri）

在 `apps/codex-plus-manager`：

```bash
npm run build
```

等价于：先 `cargo build -p codex-plus-launcher --release`，再 `tauri build`。  
产物位置依平台落在 `apps/codex-plus-manager/src-tauri/target/release/`（及 bundle 子目录）。用于开发者自测，**不等于** CI 的四平台 `internal-unsigned` 命名制品。

## 同步组件

Claude 四步 sheet 的「装组件」和「首次同步」依赖两套构建产物，**不入库**：

| 套件 | 用途 | 构建期暂存位置 |
|------|------|----------------|
| 本机 sidecar | 首次同步 | `apps/codex-plus-manager/src-tauri/binaries/` |
| 远端 linux-x86_64 组件 | 上传到服务器 | `apps/codex-plus-manager/src-tauri/resources/remote/linux-x86_64/` |

真文件由 `scripts/sync-components/stage.mjs` 写入；缺失时 `src-tauri/build.rs` 生成占位（运行时会拒绝）。git 里没有真二进制是预期。

`apps/codex-plus-manager` 的 `npm run dev` 会先跑 `stage.mjs --dev`：sidecar 必须到位；远端组件缺了会告警，四步 sheet 第三步会诚实失败。补齐远端组件（需要本机 Go；源在仓内 `cchaven/services/fns-server`，不擅装 Go）：

```bash
# 在 codex/ 目录
node scripts/sync-components/stage.mjs --build-remote
```

打内部包（§5）之前先严格暂存（两边都必须到位，缺了失败）：

```bash
# 在 codex/ 目录
node scripts/sync-components/stage.mjs
```

`BESTCODEX_CLAUDE_REMOTE_DIR` / `BESTCODEX_CLAUDE_SIDECAR` **仅排障，不是安装步骤**。

## 5. 打与 CI 同形的内部未签名包

版本号建议用 semver，例如 `1.2.46`。先改齐版本（见 [03-release.md](./03-release.md) §1），再构建。

先严格暂存同步组件（本机 sidecar + 远端 linux-x86_64 都必须到位）：

```bash
# 在 codex/ 目录
node scripts/sync-components/stage.mjs
```

### 5.1 通用：先编前端 + release 二进制

```bash
cd apps/codex-plus-manager
npm ci && npm run vite:build
cd ../..

# 本机架构
cargo build --release
# 或交叉（示例）：
# rustup target add aarch64-apple-darwin
# cargo build --release --target aarch64-apple-darwin
```

产物二进制名：

- macOS / Linux 风格：`target/release/lumio-codex`、`lumio-codex-launcher`
- Windows：`target/release/lumio-codex.exe`、`lumio-codex-launcher.exe`

### 5.2 macOS DMG

```bash
# Apple Silicon 本机示例
BINARY_DIR="$PWD/target/release" \
  bash scripts/installer/macos/package-dmg.sh "1.2.46" "arm64"

# Intel 交叉产物示例
BINARY_DIR="$PWD/target/x86_64-apple-darwin/release" \
  bash scripts/installer/macos/package-dmg.sh "1.2.46" "x64"
```

输出：`dist/macos/LumioCodex-<version>-macos-<arch>-internal-unsigned.dmg`  
（**文件名**第一期可不动；安装包**显示名** BestCodex。）

### 5.3 Windows 便携 ZIP + NSIS

在 **Windows** 上（或 CI）：

```powershell
New-Item -ItemType Directory -Force dist/windows/app | Out-Null
Copy-Item target/release/lumio-codex.exe dist/windows/app/
Copy-Item target/release/lumio-codex-launcher.exe dist/windows/app/

$version = "1.2.46"
Compress-Archive -Path dist/windows/app/* `
  -DestinationPath "dist/windows/LumioCodex-$version-windows-x64-portable-internal-unsigned.zip" -Force

Push-Location scripts/installer/windows
makensis /INPUTCHARSET UTF8 /DVERSION=$version LumioCodex.nsi
# 已签名文件名：再加 /DOUT_SUFFIX= /DPRODUCT_VERSION_QUAD=1.2.46.0
Pop-Location
```

输出：

- `dist/windows/LumioCodex-<version>-windows-x64-portable-internal-unsigned.zip`
- `dist/windows/LumioCodex-<version>-windows-x64-setup-internal-unsigned.exe`

安装目录：`%LOCALAPPDATA%\Programs\BestCodex`（当前用户，不要求管理员）。安装包显示名 BestCodex。

### 5.4 Windows MSIX（商店轨脚手架，未签名）

另开一条轨：沿用 §5.3 已 staged 的 `dist/windows/app` cargo 产物 + Windows SDK `makeappx`。
**不改** NSIS / 便携 ZIP 产物名，也不打开 `tauri.conf.json` 的 `bundle.active`。

```powershell
./scripts/installer/windows/msix/Pack-Msix.ps1 -PackageVersion "1.2.46-internal-38"
```

输出：`dist/windows/LumioCodex-<version>-windows-x64-store-unsigned.msix`

`PACKAGE_VERSION` `1.2.46-internal-38` 映射为 Identity.Version `1.2.46.38`；映射不了第四位则用 `1.2.46.0`。
`Identity.Name` / `Identity.Publisher` / `PublisherDisplayName` 仍是 Partner Center 占位，拿到商店身份后再改模板。本机找不到 `makeappx.exe` 会明确报错。

完整商店流程、与 SignPath 轨如何并存，见 [07-microsoft-store.md](./07-microsoft-store.md)。

## 6. 用 GitHub Actions 打包（推荐）

推送到 `publish`、对 PR，或手动 `workflow_dispatch`：

- Workflow 名：**Internal unsigned build artifacts**（[`.github/workflows/pr-build.yml`](../../.github/workflows/pr-build.yml)）
- 产出四个 artifact，保留 **14 天**
- 分支名为 `publish` 时版本会变成 `0.0.0-internal-<run_number>`；发版请用 tag 触发或在本地指定版本再上传

下载：仓库 Actions → 对应 run → Artifacts。

## 7. 本地冒烟清单（装包后）

- [ ] 冷启动进入未登录 / 或凭据续跑  
- [ ] 注册或登录 → provisioning → 首页在线  
- [ ] Codex 首页是问候 + 一行余额 + 一张启动卡；「启动 Codex」能拉起官方应用  
- [ ] 「充值」打开浏览器到 `https://api.lumio.games/purchase`  
- [ ] 帮助入口打开 `https://bestcodex.app/help` 
- [ ] 断网后若本机配置健康，可进离线首页并仍能启动  
- [ ] 设置页：官方应用路径检测、配置恢复二次确认文案诚实  
- [ ] 有 GitHub Release 且版本更高时，首页出现更新提醒  

## 8. 常见问题

| 现象 | 处理 |
|------|------|
| `zstd-sys` / 沙箱构建失败 | 在完整权限环境编译；勿在过度受限沙箱里 `cargo build` |
| Tauri 找不到前端 | 先 `npm run vite:build` |
| DMG 脚本报 binary not found | 设对 `BINARY_DIR`，确认 `lumio-codex` 与 `lumio-codex-launcher` 可执行 |
| macOS「已损坏」/ 无法打开 | 未签名内测包：图标上 **右键 → 打开** |
| Windows SmartScreen | 未签名预期行为；内测核对哈希后继续 |
| `cargo test` 全量挂在 launcher | 用 §3 的过滤命令，勿据此阻断 BestCodex 发版 |
| DMG 打包报「不是 Linux x86_64 ELF」 | 远端组件没暂存。先跑 `node scripts/sync-components/stage.mjs --build-remote` |
