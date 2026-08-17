# BestCodex 补全开工提示词（第一期收尾）

> 背景：`ec5a2df` 已落地 BestCodex 启动器壳、官网与 Claude Tab 交互壳。本文交给执行 Agent，补全 Review 发现的剩余缺口。
> 需求权威：[`../specs/2026-08-16-bestcodex-rebrand-design.md`](../specs/2026-08-16-bestcodex-rebrand-design.md)
> 可点原型：[`../../prototypes/bestcodex/index.html`](../../prototypes/bestcodex/index.html)
> 上一轮实现提示词：[`2026-08-16-bestcodex-implement.md`](2026-08-16-bestcodex-implement.md)
> 已拍板：官网合并成真单站（顶栏站内换整页）；Claude Tab 全量搬 cchaven 能力（同步 + 冲突 + PTY + 文件树）。

把下面「提示词」整段交给开工的主 Agent。

---

## 提示词

```
你在仓库 lumio-codex 补全 BestCodex 第一期的剩余缺口。设计已锁定（不重开产品讨论、不另出图标、不管 DNS）。上一轮已落地壳与官网骨架，这一轮补真实能力与稿面收口。

动手前读（按顺序）：
1. .spec/AGENTS.md（调度与收口门槛）
2. .spec/knowledge/README.md
3. .spec/rules/system.md
4. .spec/knowledge/features/lumio-account-and-home.md（含「待解决」清单，与本任务对应）
5. 需求权威：codex/docs/specs/2026-08-16-bestcodex-rebrand-design.md
6. 可点原型：codex/prototypes/bestcodex/index.html（app/*.html 与 site/*.html）
7. 现有 CC 桌面端（能力搬运的源头）：cchaven/apps/desktop/、cchaven/bins/fns-agent/
8. 用 before-you-code 校准深度后再动手。

————————————————
调度（重要：并行）
————————————————

三条轨道文件集互不重叠，按 .spec/AGENTS.md 的 wave 机制并行扇出三个 subagent（Wave 1）；
每个 subagent 交付后按两级审查（spec 合规 + 代码质量）过 reviewer 再合入。
Wave 2 做集成联调与真机验收，串行。

文件所有权（并行期间不得越界改；确需跨界改动，上报主 loop 协调，不许自行改）：
- 轨道 A 独占：codex/apps/codex-plus-manager/src/lumio/claude/**、src/lumio/views/claude/**、
  src-tauri/src/claude_commands.rs、codex/crates/codex-plus-core/src/lumio/claude_control.rs
- 轨道 B 独占：src/lumio/views/HomeView.tsx / RepairView.tsx / SettingsView.tsx、
  src/LumioApp.tsx、src/lumio/state.ts、src/lumio/install-progress.ts、src/lumio/invoke.ts、lumio-shell.css
- 轨道 C 独占：web/**
- 共享文件（如 src-tauri/src/lib.rs 注册新命令）：轨道 A 需要时列出改动点交主 loop 合入，
  或排到该文件当轮无人占用时再改。

————————————————
轨道 A：Claude Tab 真实能力（全量，最重）
————————————————

目标：把 cchaven 桌面端已有能力搬进 Claude Tab，消灭 stub。能力清单与源头对照：

A1. 装组件真部署
   - 现状：lumio_claude_prepare_remote 只本地 create_dir_all + 远端 mkdir -p，
     UI 却显示「安装同步组件 · 自动完成」。
   - 要求：参照 cchaven/apps/desktop 的 deploy 逻辑，真把同步组件（fns-agent/server）
     部署到远端；UI 人话进度不变（不出现 agent / tmux 字样）。
   - 这一步失败必须有失败面（人话原因 + 错误码 + 返回修改 / 重试），
     样式对齐探测步；失败后不得继续「开始首次同步」。

A2. 首次同步真拉文件
   - 现状：lumio_claude_first_sync 数完文件数固定返回 SYNC_ENGINE_UNAVAILABLE，
     files_done 恒 0，不拷贝任何文件。
   - 要求：接入 fns 双向同步引擎完成首次同步；进度事件流式上报
     （files_done / files_total 随传输递增），前端进度条真实前进。
   - 可切去 Codex，同步在后台继续（不只是「状态不丢」，是任务真在跑）；
     回来还在同一进度上。

A3. 嵌入式 PTY 终端
   - 现状：TerminalPane 是单行远程命令 + 「打开系统终端」兜底。
   - 要求：应用内交互式终端（参照 cchaven 的 terminal.rs + TerminalPane.tsx：
     portable-pty / ssh 通道 + tmux 会话，前端 xterm）。
     右工作台默认终端；切 Tab 会话不断；点项目只换右侧。
   - 允许为此引入必需依赖（如 portable-pty、@xterm/xterm），记入交付清单。

A4. 文件 / 冲突 Tab 从占位到可用
   - 文件：完整文件树（远端 + 本机），交互沿用 cchaven files 能力（浏览 / 预览）。
   - 冲突：接 cchaven conflicts 引擎，keepLocal / keepRemote / keepBoth + diff 展示；
     冲突不静默覆盖（红线）。

A5. 补漏
   - 高级选项补「本机 SSH config / Host 别名」（设计要求，现只有端口 + 密钥路径）。
   - 密码仍只留内存，不进持久化 JSON（现有红线不动）。

验收（轨道 A）：真实服务器走完 未订阅→说明→四步 sheet→工作台 全程；
首次同步文件真的到了本机目录；终端能跑交互命令；制造一次冲突并用三种方式各解一次；
切去 Codex 再回来，同步与终端会话都还活着。

————————————————
轨道 B：Codex 侧稿面修正（小改动，快）
————————————————

逐条对照原型修（都是 Review 已定位的点）：

B1. 安装失败后主按钮文案改「重试」（行为已对，只改文案；对照 app/home-install.html fail 态）。
B2. 下载 / 安装过程接「取消」入口：后端 lumio_cancel_official_app 与 invoke.ts 的
    cancelOfficialApp 已存在，HomeView 渲染取消按钮并把取消原因常驻卡内。
B3. 安装阶段进度文案带写入路径（「正在写入 /Applications/Codex.app」，
    用用户所选 destination；install-progress.ts + HomeView）。
B4. 离线且未安装时，顶栏横幅不得再说「你仍可以启动官方 Codex」，
    改为与卡片一致的「安装官方应用需要网络」（LumioApp/state 的 offline 分支按 codexApp 有无分流）。
B5. 清残留「Lumio」用户可见文案：RepairView.tsx（三处）与 SettingsView.tsx 恢复确认框，
    改 BestCodex。全局 grep 复核用户可见字符串。
B6. 失败卡补「打开帮助」次要按钮；修复页补「打开帮助」入口（可与导出诊断日志并存），
    都指 https://bestcodex.app/help。
B7. 小项（顺手对齐，不扩scope）：离线未装卡 meta 补「连上之后再回来装。」；
    准备连接页标题改「正在准备」。

验收（轨道 B）：对照 home.html / home-install.html / home-offline.html / repair.html 逐屏；
浅色 / 深色各过一遍；触发一次真下载并中途取消。

————————————————
轨道 C：官网真单站合并
————————————————

C1. 把 web/apps/codex 与 web/apps/cc 合并成一个产品站 app（建议新建 web/apps/bestcodex，
    复用 web/packages/ui 的 SiteShell / ProductDownloads / HelpCenter）。
    顶栏 [ Codex | Claude ] 是站内换整页（SPA 路由，如 /codex 与 /claude），
    不再跨子域 <a> 跳转；默认落 Codex 页。对照原型 site/index.html 的切换行为。
C2. Claude 页补 FAQ 区块（content.ts 里 FAQS 已有，挂到首页下载区之后）；
    删除旧独立路由 /pricing、/download（站内锚点替代；旧 URL 做站内 redirect）。
C3. 域名落点：产品站目标域 bestcodex.app；codex.bestcodex.app 与 cc.bestcodex.app
    做 301 的事在运维，不在本仓；仓内把 config.ts 的 siteUrl 逻辑改为单站 + 路由，
    保持环境变量可配。门户与产品站在 apex 的共存 / 迁移方案记一条 ADR（.spec/decisions/），
    不擅自改门户部署。
C4. 门户交叉文案收口：web/apps/portal 品牌保持 Lumio 不动，但产品卡 / 登录注册页
    不再写「Lumio Codex 与 CC避风港」两个旧产品，改为指向 BestCodex（一个启动器、
    一个下载）；OpenAI 花瓣 / Claude 星芒装饰按设计去掉。
C5. 帮助中心维持现有五篇 IA（安装 / 未签名 / 登录 / 修复 / Claude 连服务器）；
    原型 help.html 卡面差异已接受，不跟。
C6. 官网保持浅色（设计 §2「浅色官网」，不做站点深色主题）。

验收（轨道 C）：浏览器点通：/ → Codex 页 → 顶栏切 Claude（无整页刷新）→ 定价 → FAQ →
下载三平台 → 帮助五篇 → 账户跳门户；旧 /pricing /download URL 能到达对应锚点；
顶栏「下载」按钮字可见。

————————————————
Wave 2：集成与真机验收（串行，三轨合入后）
————————————————

- codex/apps/codex-plus-manager：npm run check / test / 构建；codex/ cargo fmt --all -- --check + cargo test
- web/：npm run check / test / 构建
- 真机（至少 macOS，Windows 尽力）：干净机走一遍 注册→登录→安装官方 Codex（含取消一次）→
  启动；Claude 连真实服务器全程；官网主路径浏览器点完再声称完成
- 知识沉淀：用 spec-steward 更新 .spec/knowledge/features/lumio-account-and-home.md 的
  「待解决」（同步 / PTY / 装组件条目改状态），架构级决策记 .spec/decisions/

————————————————
明确不要做
————————————————

- 不管 DNS / 证书；不改 api.lumio.games 与充值页
- 不改 bundle id、本机状态目录、LaunchAgent 名、桌面 Key 名
- 不焊两个 Rust workspace；不为 Claude 打独立安装包
- 不做 bestcodex.app/login（第一期口径不变）
- 不把门户整站改成 BestCodex（只按 C4 收交叉文案）
- 不另出图标、不重写历史 ADR
- 除轨道 A 明示的终端 / 同步依赖外，不引入任务外新依赖

————————————————
交付
————————————————

按 .spec/AGENTS.md 交回物格式：改动清单、验证证据（命令与关键输出）、known gaps、
知识沉淀落点。三轨各自交付各自过审，Wave 2 整体收口后再交总账。
```
