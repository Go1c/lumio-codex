# 0004 · CC避风港桌面端以引擎兼容的 P2 为骨架，把 P1 的产品外壳整段移植上去

- 日期：2026-08-13
- 状态：生效

## 背景

`cchaven/` 以 subtree 合入时带进了上游 `dev` 分支的合并提交 **`d08f164`**
（`merge(origin/dev): integrate remote desktop work, prefer CCHaven stack`）。
这条提交把两条长期分叉的桌面端分支做了 **union 合并**——不是二选一，而是把两边
代码硬拼在一起，结果桌面端 crate 完全编译不过（`cargo check -p fns-workspace-desktop`
在模块解析阶段就停了，报 29 个错误，且这只是下限）。

两条分支各自代表一种产品形态：

- **P1（`38d74e0`「ship CCHaven M1–M4 stack」）**——面向普通用户的产品外壳：
  浏览器 OAuth 登录、试用 / 订阅 / 邀请、全中文 i18n、粘贴 ssh 命令即可识别的三步
  向导、离线只读、文件写操作与 10 秒撤销、冲突的「两者都保留」、浏览器 mock 开发模式。
  同步引擎跑在 App 进程内。
- **P2（`abf6d0a`，origin/dev）**——面向工程师的运维工具：Claude 会话面板、服务器
  状态监控、诊断中心（时间线 / 健康快照 / 自检 / 脱敏支持包）、OSC 52 剪贴板桥接、
  SSH 隧道托管与退出清理、十步可预览可取消的部署、macOS 签名公证脚本、更细的冲突
  状态机。同步引擎改为跑在**本机 sidecar 进程 `fns-agent`** 上，经 SSH 隧道连服务器。

共同祖先 `730c498`。union 合并的实际破坏面：10 个文件被逐行拼接
（`lib.rs`、`files.rs`、`project.rs`、`terminal.rs`、`ssh.rs`、`deploy.rs`、`main.rs`、
前端 `App.tsx`、`styles.css`、`Cargo.toml`），`src/sync.rs` 与 `src/sync/mod.rs` 同名并存，
`credentials` / `ssh_tunnel` 缺 `mod` 声明。此外——上一轮调查未发现的一点——合并**丢掉了
P2 的整个前端外壳**（`WorkspaceView` / `OnboardingWizard` / `FileTree` / `Terminal` /
`ProjectList` 从未进入合并结果），只留下 P2 的功能面板，因此 P2 的多个源码断言测试指向
不存在的文件。

一个先决约束：`fns-sync-core` 与 `fns-transport` 已按 P2 版本恢复并提交，跑通 602 个测试
（提交 `80f772a`）。P1 的进程内同步层调用的接口（`with_engine` / `pending_commands` /
`record_local_changes` / `shutdown` / `snapshot_begin`）在恢复后的引擎上已不存在。

## 决策

**交付同时包含两组能力的完整桌面端**，实现路径是「以引擎兼容的 P2 为骨架，把 P1 的产品
外壳整段移植上去」，而不是二选一：

1. **后端重叠模块以 P2 为底，补进 P1 多出来的能力。**
   `deploy.rs`（3190 行十步部署）、`sync.rs`（4800 行 agent 生命周期）、`credentials.rs`、
   `ssh_tunnel.rs`、`remote_monitor.rs`、`diagnostics.rs` 取 P2 原样；`files.rs`、
   `terminal.rs`、`project.rs` 逐项合并；`ssh.rs`、`main.rs` 取 P1（其为功能超集）。
2. **`ProjectConfig` 统一为 P1 的 `server: ServerConfig`**，P2 遍布各处的
   `ssh_host_alias` 字段改为 `ssh_host_alias()` 方法（有 `~/.ssh/config` 别名时返回别名，
   否则返回 `user@host`）。这样密码登录的小白路径与 ssh-config 的工程师路径共用一套寻址。
3. **前端外壳取 P1，把 P2 的面板移植进来。** P1 的 `ApiProvider` / `lib/api.ts` 抽象是
   浏览器 mock 模式的前提，也是把 P2 面板接进来的现成接缝——P2 的
   `diagnosticsApi` / `remoteMonitorApi` 本来就是「注入 invoke 函数」的工厂，直接挂到
   同一个 `Api` 接口上即可，真实与 mock 两条路驱动同一批组件。
   工作区从三个 Tab 扩到六个（终端 / 文件 / 冲突 / Claude 会话 / 服务器状态 / 诊断）。
4. **绝不回退 `fns-sync-core` 与 `fns-transport`。** P1 的进程内同步层（`src/sync/` 四个
   文件、约 1330 行）整体删除，其职责由 P2 的 agent sidecar 承担。
5. **P1 的两项能力改为读 agent 的本地产物来实现**，不改 `crates/` 与 `bins/`：
   - 6.3 聚合状态的「待同步文件数」读 agent 原子写出的
     `<state_dir>/runtime-status.json`（`pending_commands` + `queued_watcher_batches`
     + `active_transfers`）；
   - 冲突的「两者都保留」需要服务器那一侧的字节。agent 是**本机** sidecar，其内容缓存
     就在本机 `<state_dir>/blobs/<hash>`，因此桌面端直接读文件即可，无需扩展 agent 协议。
   新增 `conflict_bridge.rs` 承担引擎 `ConflictView` ↔ 产品级三选一的双向翻译。
6. **重叠 UI 整合成一套，不并排保留。** 冲突页只有一个：P1 的三个人话选项 + 10 秒撤销，
   叠加 P2 的状态机反馈（等待确认 / 引擎占用中不可操作 / 取消本次提交 / 决策历史）。
   P2 的 `ConflictsPane` / `ConflictPaneContent` / `ConflictResolutionAction` 删除。
7. **界面语言统一为中文**，P2 面板的英文文案全部纳入 `i18n/zh-CN.ts`。

## 后果

- **P2 遗留的源码断言测试必须改写而非照搬。** 它们断言的是 P2 的具体写法
  （`invoke<SyncStatus>("sync_status")`、英文按钮文案、`WorkspaceView.tsx` 路径）。
  改写后断言的是**同一条保证**在合并实现上的形态；这类「源码形状测试」对重构脆弱，
  是这次代价最大的一处。
- **`retryInSeconds` 倒计时降级。** P1 的「离线 — {n} 秒后重试」依赖进程内 supervisor 的
  重连截止时间；agent 只发布 `reconnect_attempt` 计数，不发布截止时间。字段保留为可选，
  当前不填充，界面退化为不带倒计时的「离线」+ 原因码。不猜一个假倒计时。
- **两个配置目录并存。** `projects.json` / 回收暂存 / 冲突记录在 `~/…/cchaven/`，
  而 agent 状态与凭据仍在 P2 的 `~/…/fns-workspace/`。统一目录会迁移已有同步状态，
  风险大于收益，暂不动，属已知不一致。
- **P2 的前端 token 存储被 P1 的模型取代**（`src/auth.ts`、`features/account/`）。
  refresh token 只进 macOS 钥匙串、access token 只在 Rust 侧内存，前端不再持有令牌——
  这是安全面的改善，代价是 P2 那套 `secure-storage` 测试一并移除。
- **部署失败语义从「从失败步重试」变为「回滚后重来」。** P2 的十步部署在任一步失败时
  回滚凭据与项目记录；半配置好的服务器比一次失败更糟，因此采纳 P2 的语义。
- **依赖面变大**：桌面端 crate 现在依赖 `fns-agent`、`test-sync`、`fns-observability`、
  `serde_yaml`、`httparse`，构建时间相应变长。
