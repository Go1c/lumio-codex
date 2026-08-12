# CC避风港（CCHaven）桌面 APP

macOS 桌面客户端：Claude Code 跑在用户自己的云服务器上，文件与本机双向安全同步。
前端 React + TypeScript（Vite），后端 Rust（Tauri 2），同步引擎复用 workspace 里的
`fns-sync-core` / `fns-fs` / `fns-transport` crates。

> 仓库、crate、内部服务保留 `fns-` 前缀；对用户可见处一律「CC避风港 / CCHaven」。

**正式打包、签名发版与线上推送**见 [`docs/ops/`](../../docs/ops/README.md)
（尤其 [03 编译打包](../../docs/ops/03-build-package.md) 与 [04 发版](../../docs/ops/04-release.md)）。

## 快速开始

```bash
cd apps/desktop
npm install

# 1) 纯浏览器跑 UI（不需要 Tauri、服务器或控制面）
npm run dev            # http://localhost:1420，自动使用内存 mock 后端

# 2) 完整桌面应用（需要 Rust 工具链 + Tauri CLI）
cargo tauri dev        # 或 npx @tauri-apps/cli@2 dev

# 质量门禁
npm run typecheck
npm test               # Vitest + @testing-library/react
cd ../.. && cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

窗口最小尺寸 960×640；界面默认简体中文，文案集中在 `src/i18n/`（已预留 zh-HK 槽位，
未翻译的键自动回落到 zh-CN）。

## 架构

```
src/                        前端
  i18n/                     文案字典（zh-CN 为真源，zh-HK 增量覆盖）
  lib/api.ts                Api 接口 + Tauri 实现 + 错误归一化（ApiError）
  lib/mockApi.ts            内存 mock 后端：浏览器开发与全部前端测试都跑在它上面
  lib/types.ts              与 Rust 侧一一对应的类型
  state/                    ApiProvider（依赖注入）/ ToastProvider（4s，带撤销 10s）
  components/               登录页、侧栏、账户菜单、向导、工作区（终端/文件/冲突）
src-tauri/src/              后端
  auth/pkce.rs              PKCE（S256）生成与校验
  auth/loopback.rs          127.0.0.1 回环回调 HTTP 服务器
  auth/keychain.rs          系统钥匙串封装（refresh token、SSH 密码）
  auth/mod.rs               浏览器登录编排、静默续期、心跳、退出登录
  control.rs                控制面客户端（真实 HTTP + mock 两种实现）
  askpass.rs                SSH 密码通过私有 unix socket 交给 OpenSSH
  ssh.rs                    ~/.ssh/config 解析、粘贴识别、连接探测
  project.rs                项目配置持久化与目录推导
  deploy.rs                 四阶段部署（连接 / 装组件 / 建目录 / 首次同步）
  files.rs                  本地同步目录资源管理器（含撤销暂存）
  conflicts.rs              冲突投影与三种解决方式（含撤销）
  sync/                     进程内 sync-core + watcher + transport 会话监督器
  terminal.rs               PTY + ssh + tmux 持久会话
```

前端只通过 `Api` 接口与后端对话，`ApiProvider` 注入实现。测试注入 `MockApi`，
`npm run dev` 在非 Tauri 环境下自动注入 `MockApi`，Tauri 里注入 `createTauriApi`。

### 同步引擎（进程内）

打开/保存项目时，`SyncManager` 会为该项目拉起：

1. `fns-sync-core`（状态目录 `~/Library/Application Support/cchaven/sync-state/{projectId}/`）
2. 本地 `fns-fs` watcher（`protect_secrets` 强制开启）
3. 会话监督器：按 6.3 的 2s→5s→10s→30s 连接 loopback `workspace-sync-v2`

没有 agent 端点或令牌时状态条如实显示离线与退避秒数，**不会伪造同步进度**。
开发联调可设 `CCHAVEN_SYNC_ENDPOINT` / `CCHAVEN_SYNC_TOKEN`，或写入
`sync-state/{id}/endpoint` + 钥匙串 `sync-agent-token:{id}`。Linux agent 的打包与
部署仍见 `docs/spec-gaps.md` B2。

## 与控制面的对接约定

控制面为 `services/cchaven-control`，响应体统一为 `{"data": …}`，错误为
`{"error":{"code","message","details"}}`。APP 只用到下面这些端点：

| 用途 | 端点 | 说明 |
| --- | --- | --- |
| 浏览器授权 | 打开 `{web}/authorize?…` | `client_id=cchaven-desktop`、`scope=profile workspace offline_access`、PKCE `S256` |
| 换取令牌 | `POST /api/v1/oauth/token` | `grant_type=authorization_code`，响应含 `activation`（首月试用）与 `entitlement` |
| 静默续期 | `POST /api/v1/oauth/token` | `grant_type=refresh_token`，启动时调用 |
| 退出登录 | `POST /api/v1/oauth/revoke` | 撤销会话族，随后清钥匙串 |
| 账号信息 | `GET /api/v1/me` | 邮箱 + entitlement |
| 心跳 | `POST /api/v1/app/heartbeat` | 上报 `device_id`/`app_version`/`os_version`/`arch`，返回 `entitlement` 与 `notices` |

回调地址：主用回环 `http://127.0.0.1:{ephemeral}/callback`（已在
`migrations/0002_seed.sql` 注册为白名单模式），自定义 scheme `cchaven://auth/callback`
作兜底。APP 先绑定端口再拼 `authorize` URL，令牌兑换时原样回传同一个 `redirect_uri`。

**令牌处置**：refresh token 只进 macOS 钥匙串（service `cn.cchaven.desktop`），
access token 只存内存并在过期前 60 秒自动续期；两者都不落磁盘、不打日志。

## mock 说明（本机没有控制面时如何开发与验证）

本机无法启动 `services/cchaven-control`（需要 PostgreSQL），因此两侧都内置 mock：

- **Rust 侧**：`ControlClient::Mock`，debug 构建默认启用。响应结构严格对齐
  `internal/api/handler_oauth.go` 与 `handler_me.go`。
- **前端**：`MockApi` 实现完整 `Api` 接口，`npm run dev`（浏览器）与全部测试使用。

环境变量：

| 变量 | 作用 |
| --- | --- |
| `CCHAVEN_CONTROL_MOCK=0` | 关闭 mock，改打真实控制面（release 构建默认即为真实） |
| `CCHAVEN_API_BASE` / `CCHAVEN_WEB_BASE` | 指向本地控制面，如 `http://127.0.0.1:8080` |
| `CCHAVEN_MOCK_DAYS_LEFT=2` | 造出「剩余 ≤3 天」以验证到期横幅 |
| `CCHAVEN_MOCK_SUBSCRIBED=1` | 由「试用中」切换为「已订阅」 |
| `CCHAVEN_MOCK_INVITED=0` | 关闭邀请归因，首次登录不发试用 |
| `CCHAVEN_MOCK_OFFLINE=1` | 模拟网络不可达，验证离线只读模式 |
| `CCHAVEN_SECRET_BACKEND=memory` | 秘密存内存而不是钥匙串（无头环境/CI） |
| `CCHAVEN_SYNC_ENDPOINT` | 开发用 loopback `workspace-sync-v2` URL（覆盖项目 endpoint 文件） |
| `CCHAVEN_SYNC_TOKEN` | 开发用 agent bearer token（覆盖钥匙串） |

mock 里的固定行为：授权码 `invalid` 触发 `invalid_grant`；首次 `exchange_code`
发放试用，第二次返回 `trial_denied_reuse`；`revoke` 之后的续期一律失败。

## 安全约定

- **机密文件永不同步**：`protectSecrets` 固定为 `true`，后端 `normalise_sync()`
  会强制回写，UI 只显示固定提示、不提供开关；实际拦截由 `fns-fs` 的
  `HARD_SECRET_EXCLUDES` 完成。
- **秘密不落明文**：SSH 密码与 refresh token 只进系统钥匙串。执行 `ssh` 时密码通过
  `SSH_ASKPASS` + 0700 私有 unix socket 传递，不出现在 argv、环境变量或磁盘上。
- 远端命令一律 argv 传参并对路径做 POSIX 引号转义（`deploy::shell_quote`）；
  诊断信息在展示前过滤掉包含 password/passphrase 的行。
- 首次连接使用 `StrictHostKeyChecking=accept-new`；主机密钥变更会中止并提示。

## 已知缺口

规范与实现之间的矛盾、以及尚未接通的能力，记录在
[`docs/spec-gaps.md`](docs/spec-gaps.md)。
