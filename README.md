# Lumio Codex

<p align="center">
  <img src="assets/brand/lumio-icon.png" alt="Lumio Codex 图标" width="160">
</p>

<p align="center">
  中文 | <a href="README_EN.md">English</a>
</p>

<p align="center">
  <img alt="License" src="https://img.shields.io/github/license/Go1c/lumio-codex">
  <img alt="Rust" src="https://img.shields.io/badge/Rust-2024-orange">
  <img alt="Tauri" src="https://img.shields.io/badge/Tauri-2.x-24C8DB">
</p>

Lumio Codex 是面向 LumioAPI 用户的轻量桌面客户端。它自动检测用户已经安装的官方 Codex / ChatGPT 桌面应用，完成 Lumio 账户接入、余额与套餐展示、Responses 配置和启动交接，让用户继续使用官方 Codex 的原生模型选择器。

生产 API 固定为 `https://api.lumio.games/`。官网为 `https://lumio.games/`。用户不需要手填 Base URL 或 API Key。

> **开发 / 发布状态：** `publish` 是持续集成与发版准备分支。账户、登录、配置接管、离线启动、官网、充值打开网站、GitHub 更新提醒已落地。当前对外分发仍以明确标记为 `internal-unsigned` 的内部测试制品为主。没有完成 Apple Developer ID 签名与公证、Windows 代码签名和回滚演练前，**不发布**正式公开安装包（见 CI `Public release gate`）。

### 部署 · 打包 · 发版（运维手册）

正式上线推送请按顺序阅读：

1. **[docs/ops/README.md](docs/ops/README.md)** — 总览与上线顺序  
2. [本地编译与打包](docs/ops/01-local-build.md)  
3. [官网部署](docs/ops/02-website-deploy.md)  
4. [版本发布与更新提醒](docs/ops/03-release.md)  
5. [后台 API（Sub2API）](docs/ops/04-backend.md)  
6. [日常维护与文档同步](docs/ops/05-maintenance.md)

## 产品流程

正式版本的固定流程如下：

1. 检测官方 Codex / ChatGPT 应用；检测失败时允许手动选择路径。
2. 动态显示服务条款、隐私政策、使用政策和区域声明。
3. 使用 Sub2API 的邮箱验证码、密码注册 / 登录和 2FA。
4. 自动复用或创建账号级唯一的 `Lumio Codex Desktop` API Key。
5. 配置 LumioAPI、Responses 协议、模型目录和服务端默认模型。
6. 展示余额、试用额度和套餐状态。
7. 一键启动官方 Codex，后续模型切换仍使用 Codex 原生选择器。
8. 充值时在系统浏览器打开官网 `/payment`（当前为打开网站；安全一次性交接可后续增强）。

Lumio Codex **不下载、不修改，也不捆绑官方 Codex / ChatGPT 应用**。请先从 OpenAI 官方渠道安装受支持的桌面应用。

## 支持平台

| 平台 | 架构 | 当前制品 |
| --- | --- | --- |
| Windows | x64 | NSIS 安装包、便携 ZIP |
| macOS | Apple Silicon arm64 | DMG |
| macOS | Intel x64 | DMG |

Windows 使用当前用户目录安装到 `%LOCALAPPDATA%\Programs\Lumio Codex`，不请求管理员权限。macOS DMG 只显示一个 `Lumio Codex.app`，启动辅助程序保存在应用包内部。

## 内部测试包

内部包由 GitHub Actions 的 `Internal unsigned build artifacts` 工作流生成，保留 14 天：

- `LumioCodex-<version>-windows-x64-setup-internal-unsigned.exe`
- `LumioCodex-<version>-windows-x64-portable-internal-unsigned.zip`
- `LumioCodex-<version>-macos-arm64-internal-unsigned.dmg`
- `LumioCodex-<version>-macos-x64-internal-unsigned.dmg`

这些制品没有生产级代码签名，只用于受控内测。不要将它们镜像为公开稳定版，也不要关闭系统安全机制来扩大分发。

正式发布后，[GitHub Releases](https://github.com/Go1c/lumio-codex/releases) 是版本、Tag、校验值和签名制品的唯一权威来源；S3 HTTPS 下载域名只同步同一批经过校验的制品。当前公开 Release 工作流会在签名前置条件未满足时直接阻断。

## 产品边界

Lumio 精简模式不公开 Provider、Base URL、Key、协议、多供应商、脚本、会话增强、Stepwise、Goals、MCP、Skill、Plugin 或注入配置。首版也不内置支付 UI、第三方 OAuth、邀请码或设备管理。

项目保留必要的上游实现代码以便同步，但 Lumio 产品入口只注册精简命令面；隐藏的旧模块不等于对用户承诺的功能。

## 安全与隐私

- 访问令牌与 API Key 落在 Lumio 数据目录的 owner-only 文件（见 `.spec/decisions/0001-lumio-credentials-local-file.md`）；日志与界面只显示脱敏值。系统钥匙串为后续可选项。
- 第一次接管前保存 Codex 配置快照，只合并 Lumio 负责的字段；设置中可整文件恢复接管前快照。
- 遥测 UI 默认关闭且本期未接真实上报通道。
- 服务临时不可用时，已有有效本机配置的登录用户仍可启动 Codex；注册、账户刷新在离线时明确不可用。

生产秘密、签名凭据、S3 凭据和部署配置不得进入仓库。

## 本地开发

需要 Node.js 22、稳定版 Rust、Tauri 2 平台依赖，以及用于端到端测试的官方桌面应用。不要把真实凭据写入测试或提交。完整命令与打安装包步骤见 **[docs/ops/01-local-build.md](docs/ops/01-local-build.md)**。

```bash
git clone https://github.com/Go1c/lumio-codex.git
cd lumio-codex/apps/codex-plus-manager
npm ci
npm run check
npm test
npm run vite:build

cd ../../
cargo fmt --all -- --check
cargo test -p codex-plus-core --lib lumio
cargo test -p codex-plus-manager
cargo check -p codex-plus-manager -p codex-plus-launcher
```

构建本机内部二进制：

```bash
cd apps/codex-plus-manager
npm run build
```

仓库结构：

```text
apps/codex-plus-manager/       Lumio Codex Tauri 与 React 客户端
apps/codex-plus-launcher/      内部启动辅助程序
crates/codex-plus-core/        跨平台检测、配置、账户与启动
crates/codex-plus-data/        本地数据层
site/                          官网静态站（lumio.games）
docs/ops/                      部署 / 打包 / 发版 / 后台维护
assets/brand/                  品牌源图
scripts/installer/windows/     Windows NSIS 内测安装脚本
scripts/installer/macos/       macOS DMG 内测打包脚本
.spec/                         Agent 规则与知识库
```

## 开源、上游与第三方声明

本项目以 `AGPL-3.0-only` 公开源码，完整条款见 [LICENSE](LICENSE)。分发修改版本或通过网络向用户提供修改后的版本时，必须按 GNU AGPL v3.0 提供对应源代码。

Lumio Codex 是 [`BigPizzaV3/CodexPlusPlus`](https://github.com/BigPizzaV3/CodexPlusPlus) 的 AGPL Fork，保留上游同步关系和历史归属。第三方代码与素材声明见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。品牌替换不会移除原许可证或第三方义务。

Lumio Codex 是独立项目，与 OpenAI 无隶属、赞助或认可关系。OpenAI、ChatGPT、Codex 及相关名称和标识是其各自权利人的商标。本项目不授予官方应用、商标或第三方内容的任何权利。
