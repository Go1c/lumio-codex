# 规范缺口与矛盾（M3 桌面 APP）

实现依据 `docs/design/interaction-design.md`（v3）与 `design/prototype/`。下面记录
实现过程中发现的**规范矛盾、未定义之处、以及尚未接通的能力**。规范本身未作修改。

编号仅供引用，不代表优先级。

---

## A. 规范内部矛盾 / 未定义

### A1. 「离线 — {n} 秒后重试」的 n 没有定义来源

6.3 状态表给出「离线 — {n} 秒后重试」，同节的重连策略是 2s→5s→10s→30s 指数退避。
两处未说明状态条的 n 是否就是当前退避值；原型固定显示 5 秒。

**当前实现**：侧栏全局状态条已改为读后端 `retryInSeconds`（会话监督器真实退避截止
时间）；无值时仍回落 5 秒。终端断线横幅继续用自己的退避。与原型「固定 5」不一致，
属有意对齐 6.3 重连策略。

### A2. 「正在同步 {n} 个文件」的 n 目前没有真实来源

6.3 与 5.4 都要求状态条显示在传文件数，附录 A 把它列为「待后端确认：sync-core 的
聚合状态事件（需暴露给前端）」。

**当前实现**：`sync_status` 从引擎 outbox + 未落地 stream item 聚合 `pending`，有会话
时不再伪造进度。无 agent 端点时保持 `offline`（可带真实退避 n），绝不假装 syncing。
资源管理器里逐文件的 ↑↓ 标记仍按「最近 60 秒内修改」近似推导。

### A3. 排查清单第 3 条引用了高级选项里的端口

5.3 失败态第 3 条文案是「云平台安全组/防火墙是否放行 {端口} 端口」，但端口字段收在
高级选项中，默认路径的用户看不到它。

**当前实现**：文案填入实际使用的端口（默认 22），不额外解释端口从哪来。

### A4. 编辑项目时密码字段的行为未定义

5.4 的「编辑（打开向导预填）」会重新展示第 1 步，但密码已经在钥匙串里，规范没说要不
要重新输入。

**当前实现**：编辑态密码框留空即沿用钥匙串中的既有密码；填了才覆盖。第 1 步在编辑态
默认视为「已验证连接」，不强制重连。

### A5. 「两者都保留（另存副本）」的副本命名未定义

**当前实现**：`src/engine.rs` → `src/engine.服务器版本.rs`（无扩展名时追加
`.服务器版本`），保证副本在 Finder 中紧邻原文件且含义自明。

### A6. 撤销窗口内被覆盖内容的存放位置未定义

5.5 要求「被覆盖版本先存入本地回收暂存」，未指定位置与清理时机。

**当前实现**：`~/Library/Application Support/cchaven/trash/{token}/`，10 秒 toast
过期后由前端调用 `purge_delete` 清除；冲突解决的旧内容随冲突记录文件一起保存，
`forget_conflict_undo` 时丢弃。

### A7. 有 refresh token、网络不可达、但本地没有任何缓存项目

3.4 要求这种情况进「离线模式」，5.1 又要求「离线使用」链接**仅在有本地缓存项目时**
显示。二者在「无项目」时冲突。

**当前实现**：按 5.1 处理——停在登录页，显示错误条 + 重试，不提供离线入口（离线进去
也没有任何可看的内容）。

### A8. 2.3 写的是「引入轻量路由」，但 APP 没有可见的地址栏

**当前实现**：主区形态（登录 / 空状态 / 工作区）+ 模态用状态机管理，不引入 URL 路由。
导航层级与 2.3 完全一致，只是没有 URL 表达。若后续需要深链（如 `cchaven://` 打开某
项目），再引入路由更合适。

---

## B. 尚未接通的能力（需要其他里程碑配合）

### B1. 进程内 sync 已接通；跨主机 agent 尚未分发

桌面进程内已托管 `fns-sync-core` + 本地 watcher + `fns-transport` 会话监督器
（`src-tauri/src/sync/`）：

| 已接通 | 仍缺 |
| --- | --- |
| 打开项目时拉起引擎 / watcher / 重连循环 | Linux `fns-agent` 交叉编译、打进 DMG、部署上传（见 B2） |
| 6.3 四态 + 真实退避 `retryInSeconds` | 没有 loopback 端点/令牌时只能诚实离线（`detail=no_endpoint`） |
| 资源管理器 create/rename/delete 写入引擎 outbox | — |
| `protect_secrets` 在引擎层强制开启，项目配置无法关闭 | — |
| 引擎冲突非空时投影进 `ConflictStore`；解决会调用 `resolve_conflict` | 无会话时仍用投影文件；mock 仍可种样例冲突 |
| 端点：`CCHAVEN_SYNC_ENDPOINT` 或 `{sync-state}/{id}/endpoint`；令牌：钥匙串 / `CCHAVEN_SYNC_TOKEN` | 向导不会自动写入这些凭据（依赖 B2 部署） |

### B2. 「安装CC避风港同步组件」阶段没有可上传的 agent 产物

附录 A 要求「agent 部署过程的阶段事件上报」，但仓库里没有为 Linux 目标构建的
`fns-agent` 产物，也没有既有的部署脚本可复用。

**当前实现**：该阶段做真实的远端环境体检——检查可用磁盘（<200MB 直接给出人话错误）、
检查持久会话组件是否可用、创建组件目录，并按 5.3 的措辞报告。上传与拉起 agent、
回写 loopback `endpoint` + agent token 留给后续里程碑，接入点在
`deploy.rs::stage_install`。没有这些凭据时，B1 的会话监督器会按 2/5/10/30s 重试并
保持离线，而不会假装已同步。

### B3. 控制面无法本机联调

`services/cchaven-control` 需要 PostgreSQL，本机起不来。Rust 侧
`ControlClient::Mock` 与前端 `MockApi` 都严格对齐 `internal/api` 里的真实响应结构
（含 `{"data": …}` 信封与错误信封）。真实 HTTP 客户端已实现但**未经过真实服务端验证**，
需要在 CI 或有数据库的环境里做一次端到端联调。

### B4. 自定义 scheme `cchaven://auth/callback` 尚未注册

回环回调（`http://127.0.0.1:{port}/callback`）已完整实现并被控制面白名单接受。
scheme 兜底需要在 `Info.plist` 里注册 URL scheme 并接入 Tauri 的 deep-link 插件，
属于打包配置改动，M3 未做。回环失败时的兜底目前是「手动粘贴授权码」（5.1 已要求）。

---

## C. 需人决策

### C1. 应用标识符与配置目录已改名，旧版本数据不会自动迁移

| 项 | 旧值 | 新值 |
| --- | --- | --- |
| bundle identifier | `com.go1c.fns-workspace` | `cn.cchaven.desktop` |
| 配置目录 | `~/Library/Application Support/fns-workspace` | `…/cchaven` |
| 钥匙串 service | 无 | `cn.cchaven.desktop` |

产品尚未发布，按「不写迁移」处理。若已有内测用户，需要补一段一次性迁移。

### C2. 非 macOS 平台的密码登录

SSH 密码通过 `SSH_ASKPASS` + 私有 unix socket 传递（见 `askpass.rs`）。Windows 上没有
对应机制，该路径返回「该平台不支持密码登录，请使用 SSH 密钥」。产品只面向 macOS，
但 workspace CI 会在 Windows 上跑 `cargo check`，故保留了可编译的兜底实现。

### C3. 控制面 HTTP 客户端的 TLS 后端

`reqwest` 使用 `native-tls`：macOS 走 Security.framework、Windows 走 schannel，
两者都不需要额外依赖；**Linux 需要 OpenSSL 开发包**。workspace 的 CI 会在
ubuntu 上跑 `cargo check --workspace --all-targets`，GitHub 托管镜像自带 libssl-dev，
但自建 runner 需要注意。若希望彻底摆脱系统依赖，可改用 `rustls-tls`（代价是编译时间
明显变长）。

### C4. 心跳频率

5.6 没有规定 `POST /api/v1/app/heartbeat` 的频率。当前取 5 分钟一次 + 启动时一次。
若后台的 DAU/留存口径对频率敏感，需要后端给一个明确值。
