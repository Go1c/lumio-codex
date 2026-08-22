---
name: lumio-account-and-home
description: BestCodex 桌面账户与首页：注册/登录/2FA、provisioning、问候+一行余额+启动卡、Claude 余额开通、修复与设置、本机缺失时首次安装官方应用——改账户壳或启动编排时查
metadata:
  type: doc
  status: 已交付
---

# BestCodex 桌面账户与首页

在 BestCodex 启动器上落地注册、登录（含 2FA）、自动配置、Codex 首页（问候 + 一行余额 + 一张启动卡）、needs-repair 与设置，对接 Sub2API 认证，完成官方 Codex 配置接管与无注入启动；本机没有官方桌面应用时，启动卡内可首次安装原样官方包后再启动。用户可见品牌是 **BestCodex**；站点 `https://bestcodex.app`，帮助 `https://bestcodex.app/help`。

账户状态机、错误码八域、零配置（用户不填 Base URL / API Key）、秘密不上屏仍有效。旧 UX「品牌弱化」作废，只换壳不改逻辑。

## 背景 / 目标

- 用户可见表面只讲账户、余额、启动与本机偏好；不暴露 provider / base url / API Key 等内部概念。
- Codex 首页不是仪表盘墙：问候 + **一行**「邮箱 · 余额 · 充值」+ 一张玻璃启动卡。
- 窗口两个 Tab：默认 **Codex**，第二个 **Claude**（仓库仍可叫 cchaven / CC）。设置共用，不是第三个 Tab。
- 服务暂不可用时，已登录且本机配置健康的用户仍可启动官方 Codex（离线降级态）。
- 绝不静默覆盖用户本机 Codex 配置；冲突必须进无 Tab 的修复页。
- 不改：bundle id `games.lumio.codex`、LaunchAgent 名 `games.lumio.codex.plist`、二进制 `lumio-codex`。用户可见名是 **BestCodex**；本机状态目录跟 `PRODUCT_NAME`（`project_dirs("games", "Lumio", "BestCodex")`）；桌面 Key `BestCodex Desktop`。安装包**显示名**与 macOS `.app` 目录均为 BestCodex；Windows `InstallDir` / 注册表键仍为 `Lumio Codex`；制品文件名 `LumioCodex-*`。

## 设计

- **设计面**：状态机阶段见架构规格；错误码八域（`AUTH_` / `ACCOUNT_` / `KEY_` / `SERVICE_` / `CODEX_` / `PAYMENT_HANDOFF_` / `PREFERENCE_` / `UPDATE_`）+ `UNKNOWN`（`PREFERENCE_` 随开机启动交付，D-16 时为七域）；凭据本期落本机 owner-only 文件（ADR-0001），非系统钥匙串。
  - 网关面（`/v1/*`，无管理 API 信封）错误体是 `{"code","message"}`：`api.rs` 的 `gateway_error_reason` 按 `code` → `reason` → `error.code` 解析后交给 `normalize_reason`；余额不足（`INSUFFICIENT_BALANCE`）归 `ACCOUNT_INSUFFICIENT_BALANCE`（用户可操作，禁止伪装成宕机），空模型目录归 `SERVICE_MODEL_CATALOG_EMPTY`（D-16）。
  - **交互面**：文案与账户流程以 UX 交互规格为准，Codex 首页壳以 BestCodex 规格为准。顶栏分段控件 `[ Codex | Claude ]`，冷启动落在 Codex；空白标题栏可拖窗口（macOS Overlay 用 `data-tauri-drag-region`，分段控件与帮助/设置除外）；`?` 打开 `https://bestcodex.app/help`。恢复动作按整文件回滚诚实描述；支付 / 遥测本期禁用并附说明（开机启动已交付，见下；更新为「提示 + 用户点击 + 应用内下载 + 安装向导」的手动流程，见「官网与更新提醒」，后台自动安装未开放）。
    **Codex 首页**主区永远是：问候 + **一行**「邮箱 · 余额 · 充值」+ 一张玻璃启动卡。不要余额 / 套餐 / 模型三张指标墙。启动卡三态：已就绪（Codex 已就绪 · 可启动，主按钮「启动 Codex」）；未安装（尚未安装官方 Codex，进度 / 失败都留在这张卡里，主按钮「安装并启动官方 Codex」）；离线（离线可用 · 缓存，充值需要网络、启动不需要）。安装中按钮禁用成「正在安装…」加阶段（下载 / 校验 / 安装），进度条在卡内（下载带百分比与已下载 / 总量，未知总量 / 校验 / 安装为往复动画，D-19）；失败 / 取消原因常驻卡内（警示色 + 错误码，主按钮即重试，D-20），后台提前失败同样收敛为 failed 不许停在「正在安装」；切到 Claude 时安装继续。无应用且离线禁用并注明「安装官方应用需要网络」。配置冲突仍走无 Tab 的修复页，不在这三态里。设置里的重新检测 / 手动选择只留给装过但路径异常的人。
    **工作区 Tab**：`workspace` 是 UI 选择（默认 `codex`），不是 `LumioPhase`。仅 `ready-online` / `ready-offline` 露 `[ Codex | Claude ]`；`signed-out` / `authenticating` / `provisioning` / `needs-repair` 无 Tab。`HomeView` 与 `ClaudeWorkspace` 用 `hidden` 保活，切 Tab 不卸载。Claude 订阅问 `https://api.cc.bestcodex.app/api/v1/me/entitlement`（Sub2API Bearer，成功体 `{data}`），解析并保留 `status` / `expires_at` / `days_left` / `expiring_soon`；`none` / `expired` = 开通卡，开通卡用余额支付 19.9（控制面 `POST /api/v1/billing/pay-with-balance`，价 1990 分、+30 天、不自动续费）；不足才打开 `/purchase`（`paymentUrl` → `https://api.lumio.games/purchase`）；禁止把账户中心当开通页。开通后 Empty / Home 固定展示「已订阅 · 有效期至 {本地日历日}（剩余 N 天）」；≤3 天另有「即将到期」文字，不只靠颜色。支付成功用回执里的 entitlement 立刻写入 store。`503 debit_unavailable` 映射为 `SERVICE_DEBIT_UNAVAILABLE`（「扣费服务暂时不可用，请稍后重试。请勿换新单。」），不得折叠成泛化宕机；失败路径禁止 `clear_pay_key`。工作台与开通卡的「开通记录」打开门户账户中心 `https://bestcodex.app/account#orders`（页签分栏里的开通记录）；账单仍来自控制面 `GET /billing/orders`，pending 写「处理中，请勿重复支付」，并可点「刷新开通状态」走原单 `POST /billing/orders/{orderNo}/resume`。控制面不可达且本地有项目 → 工作台只读/已缓存。四步 sheet 状态在 `lumio/claude/store.ts`；密码只留内存 Map，不进 persistable JSON。装组件把远端同步组件拷到服务器并写成常驻服务（开机自启、挂了就拉起来，失败停在该步，不得进首次同步）。远端 linux-x86_64 `fns-agent` 是静态 musl ELF，不依赖目标机 glibc 版本；`fns-server bootstrap-workspace` 以现有服务端数据库签发并幂等复用严格限定为 `ws / fns-agent / workspace_rw` 的 JWT，同时注册 workspace root，禁止客户端内置固定 token。服务端仅监听 loopback 并用显式 YAML 启动；远端组件读取 owner-only token 文件，本机 sidecar 在远端启动成功后经 SSH 读取同一 token，并只通过匿名 stdin 管道注入进程、不得落盘，凭据不得含空白或控制字符。systemd 不可用或启动失败时由 watchdog 持续拉起两份组件，不再使用一次性 nohup。拷文件走 scp，端口用 `-P`，不得复用 ssh 的 `-p`。服务器上已经有组件时不得报失败，弹出确认面让用户点「重装」；重装先停掉 watchdog 与正在跑的文件、确认退出后再覆盖。打开已保存项目时若进程不在也会再拉一次。首次同步经本机 sidecar + SSH 隧道拉文件，未确认拷贝不得关 sheet 进工作台。打开已保存的已连接项目必须 resume 官方双向同步（sidecar + 隧道到远端 9000），SSH 列文件不算已同步；状态栏只在远端同步组件确实在跑时报运行中，未运行或引擎错误用红色警告，冲突单独标，不得把「本机目录已就绪」或「本机进程还在」当成成功。底栏失败只写短标签（同步未运行 / 同步不可用 / 同步出错）；远端没拉起来不得写成「这个版本没有把同步组件打进来」。打开已保存项目时若进程不在也会再拉一次。自动失败后，左栏红字、底栏和抽屉服务表都要能点「重新拉起」，拉不起来再走确认后的「重装」；「刷新」只读状态、不是修复。Claude 工作台是三层模型：服务器（一个 SSH 连接）→ 项目（这台机器上的一个文件夹）→ 会话（这个项目里的一个对话）。连接状态、Claude 版本、Anthropic 登录是服务器级事实，只出现在左栏服务器行，项目行不许再标「已登录」或版本号；会话只出现在中栏标签条，左栏不列会话。点「新对话」在项目目录启动官方 Claude（`~/.local/bin/claude`），不是空 shell；终端设备回包不得当成对话标题。布局是三栏 + 底栏（方案 D）：左 236px 服务器分组→项目，中栏顶部会话标签 + 下面整格终端（期一是初始化清单），右 282px 本机/服务器合并的文件浏览器，底 26px 一行小字点开抽屉（服务器状态 / 对话状态 / 冲突）；再点同一段、点当前页签、点页签条空白或「收起」都会关上。五个旧 stage tab（终端 / 文件 / 冲突 / 服务器状态 / 对话状态）已退场。期一中栏清单：连接 → 装同步组件 → 首次同步 → 装官方 Claude CLI → 登录 Anthropic；期二日常对话，冲突/有新版只在底栏出角标；期三点左栏项目自动重连（三行进度），登录过期用浮层盖住终端，连不上给离线卡但本机文件仍可看；点右上「离线」或「看本机文件」关掉卡片回到工作台，底栏继续标离线。切项目不停会话。文件浏览器对齐 Orca 那一套（名称/内容检索、Aa/ab/.*、glob、目录优先、类型图标、右键菜单）；角标仍是两侧内容差（已改 / 新增 / 冲突），指纹来自两侧内容而不是远端缺失的 size。冲突仍是单文件颜色 diff（keepLocal / keepRemote / keepBoth，不静默覆盖）。远端官方 CLI 走已授权的官方安装器（见 ADR-0015），装到 `~/.local/bin/claude`，装完回读 `claude --version`；Anthropic 登录是界面一等步骤（系统浏览器 + 授权码输入框），不许在黑底终端里盲贴。连接方式用 Tab 切换「IP 用户密码」与「本机 SSH 方式」（读 ~/.ssh/config Host 别名）；密码表单字段是「主机IP」。用户可见文案不出现 agent / tmux。
- **远端组件隔离**：一台服务器共享一个仅监听 loopback 的 `fns-server`；每个远端项目在 `state/workspaces/<workspace-id>/` 独立保存凭据、配置、SQLite 状态、日志、PID 与 watchdog，并使用独立的 `bestcodex-sync-<workspace-id>.service`。重装共享二进制前停止全部工作区进程，两份制品先上传到临时文件、确认齐全后再换入；上传、换入或启动失败会恢复已有工作区，升级成功后逐一恢复。旧版全局 `bestcodex-sync.service` 与全局 watchdog 在首次启动新布局时停用，但旧状态库不删除。
- **实现面**：
  - Rust：`codex/crates/codex-plus-core/src/lumio/`（api / credentials / secret_file / session / config_takeover / account / launch / official_app_install / autostart / claude_control）
  - 开机启动（壳自身，非官方 Codex）：`autostart.rs` 默认开启（opt-out）——bootstrap 时对从未表达偏好的用户注册一次，用户关闭后偏好落 `state_dir()/launch-at-login.json` 永不自动重开；偏好开着但注册指向旧路径（应用被移动/重装，D-26）时 bootstrap 重对齐到当前 exe；macOS 写 `~/Library/LaunchAgents/games.lumio.codex.plist`（launchd 拉起，第一期不改此名），Windows 经 `reg.exe` 写 HKCU Run（参数走 `Command::args`，无 shell 解析）；cargo 直跑（非 .app bundle / target 目录）不支持并报 `PREFERENCE_LAUNCH_AT_LOGIN_UNSUPPORTED`；系统现状为权威上报，注册被用户从系统设置移除不自动恢复；零新依赖
  - 首次安装：`official_app_install/` 按计划 → 下载 → 校验 → Windows / macOS 适配切开；默认镜像，官方直链 / FE3 备用；进度可轮询，不堵 UI；缓存文件按平台带 `.msix`/`.dmg` 扩展名（裸名会让 Windows 签名/部署工具认不出包，D-21）；镜像 v5 起 sha 缺位时用 per-arch `contentLength` 做尺寸防线；Windows Authenticode 预检三态——钉选通过 / 确凿不匹配拒绝 / 预检不可用仅在侧载路线放行（`Add-AppxPackage` 系统级签名验证 + 装后包族钉选双兜底），便携路线无系统兜底必须硬失败（D-21）；**安装位置可选**（ADR-0006 / D-23）：主按钮先弹「选择安装位置」——Windows「标准安装（MSIX，默认）」与「选择安装目录（便携解压，钉选校验不降级）」并列，macOS 默认 /Applications 可任选；自选目录在下载前先 `create_dir_all` 并探测可写（坏目录不等到 745MB 下载完才暴露，D-28）；安装成功即删安装包（失败保留重试）；自选目录的最终安装路径持久化到 `state_dir()/official-app-path.json` 并优先于自动探测（失效回落，防重启后误判未安装重复装）
  - Tauri：仅 `lumio_` 命令白名单（含 `lumio_install_official_app` / `lumio_official_app_status` / `lumio_cancel_official_app` 与 `lumio_claude_*`）；秘密不跨 IPC。Claude 引擎胶水在 `src-tauri/src/claude_*.rs`；fns 以 sidecar / 远端制品接入，不把 `fns-*` 焊进 Codex workspace；fns-server 源在仓内 `cchaven/services/fns-server` 独立维护，构建期由 `scripts/sync-components/stage.mjs` 暂存，真产物不入库
  - 前端 Claude：`src/lumio/claude/**` 与 `views/claude/**`；终端用 `@xterm/xterm`（及 fit / web-links）
  - 前端：`codex/apps/codex-plus-manager/src/LumioApp.tsx` 的 `planStartup` 负责探活 + 接管健康检查后再决定 provisioning / offline-ready / needs-repair（状态机仍有效）；安装进度挂在 Codex 启动卡内，不新增全屏阶段；余额只出现在首页那一行（可刷新），不要独立指标卡。余额刷新三通道（D-29）——ready-online 时 60s 定时轮询、托盘唤起（Rust `show_main_window` 发 `lumio://window-shown`，前端按距上次同步 ≥10s 节流）补刷、余额行内手动刷新，都走 `account-refreshed` 事件（节流判定在 `lumio/account-refresh.ts`）
  - 配置接管以**快照存在性**判定首次，不以 manifest；敏感文件经 `secret_file::write_secret` 创建即 0600；接管产物里 provider 是**内联表**（`model_providers = { lumio = {...} }`），对它做字段增删必须同时覆盖 inline / 标准表两种形态——只认 `as_table_mut` 的移除是死代码（D-22）；bootstrap 对 Healthy 接管做 legacy `env_key` 愈合并同步 manifest 哈希（D-22：旧接管残留该字段时官方 Codex 聊天必报 Missing environment variable，Healthy 状态永不重接管，必须在启动编排前单独清）
  - 启动仍走 `launch::launch_official_codex`（macOS `open -a`，Windows 直接拉官方可执行文件），无注入 / CDP

## 待解决

- Claude 真机：组件随包机制已落地（`codex/scripts/sync-components/`，CI ubuntu job 产 Linux 制品，
  安装包内建校验，缺组件即构建失败）；干净机四步 sheet 端到端验收（装组件 + 首次同步）待跑
- Claude 真机：确认拷贝默认等 60s，超时 fail-closed（`SYNC_COPY_UNCONFIRMED`），重试可能因本机已有文件再次不确认；keepLocal 后按内容重检可能把同一冲突刷回来
- 干净机真机验收未跑：注册→登录→安装官方 Codex（含取消）→启动；官网主路径浏览器点通。测试服 `vps-108-80-81-15` 可 SSH，同步组件已随包（构建期 `scripts/sync-components/stage.mjs` 暂存），首次同步未在桌面端走完
- 字段级配置恢复（相对整文件回滚）
- 系统凭据库替换本地文件（需新依赖，另开 ADR）
- 安全支付交接（一次性 handoff token）；当前为打开 `https://api.lumio.games/purchase`
- 已知坑：`provision` 步骤 payload 若漏 `account`，前端 `undefined !== null` 会推进假账户并在首页读 `email` 黑屏；IPC 侧用 `normalizeOptionalAccount`，UI 用 truthy 守卫
- 真实遥测上报、签名后的自动安装更新
- 登录后 provisioning 路径可再补一次接管冲突检查（启动有凭据路径已拦）
- 官方应用镜像清单尚未经 Sub2API `GET /api/v1/desktop/config` 转发，源常量仍在客户端 `sources.rs`（ADR-0005）；镜像已升 schema v5（`manager.payloads` 与 `SHA256SUMS` 均撤除），客户端以 per-arch `contentLength` 兜完整性，镜像端是否恢复 sha 下发待定
- 首次安装的下载链路缺陷已修（D-17：镜像 302 逐跳 https 跟随、FE3 微软投递域 http 放行、总超时放宽到 3600s）；Windows 装后不自动打开已修（D-18：去掉已注册包列表的进程级缓存 + Windows 启动改走 `ApplicationActivationManager` 包激活，激活失败退回直拉 exe）、下载无进度反馈已修（D-19：前端透传 `bytesDownloaded/bytesTotal` 并渲染进度条）、失败原因常驻面板已修（D-20）、校验未通过已修（D-21：缓存文件补 `.msix` 扩展名 + 镜像 v5 contentLength 尺寸防线 + Authenticode 预检三态化——侧载路线预检不可用放行给系统部署验证）；Windows / macOS 真机验收（干净机下载、校验、安装并启动）仍未跑通，D-18/D-19/D-20/D-21 待真机复验，扩展名假设待 CI 诊断工作流（`.github/workflows/mirror-verify-probe.yml`，手动触发）闭合

## 官网与更新提醒

- 用户可见站点：[`https://bestcodex.app`](https://bestcodex.app)；帮助：[`https://bestcodex.app/help`](https://bestcodex.app/help)
- 产品站是单站 `web/apps/bestcodex`：`/` `/codex` 为 Codex 页，`/claude` 为 Claude 页，顶栏站内换页。旧 `web/apps/codex` / `web/apps/cc` 已退役。门户仍是独立账号中心（ADR-0007），用户可见品牌是 BestCodex；apex 共存窗口待运维切 DNS
- 旧静态站（过渡）：[`codex/site/`](../../../codex/site/)（历史 `lumio.games`，待 301；DNS 仍待运维）
- 充值仍 `https://api.lumio.games/purchase`。Sub2API CORS 放行 `https://bestcodex.app` 仍待运维
- 更新提醒：`lumio_check_update` 先对照 GitHub Releases latest，404（内测渠道只发 prerelease，latest API 对其恒 404）时回退读 `/releases` 列表（含 prerelease，跳过 draft，D-27）
- 手动更新（D-30）：不做后台自动安装——检测到新版本时**右下角弹出常驻通知卡**，**绿色标记常驻**于导航「设置」角标、footer 版本旁入口与设置页「发现新版本」条目；任一入口触发 `lumio_download_update`，应用内下载平台安装包（资产按既有 CI 命名选：`-setup-*.exe` / `macos-<arch>-*.dmg`，文件名 `LumioCodex-*` 第一期可不动）到缓存 `updates/` 并打开安装向导，安装由用户在向导里完成。弹窗有频率闸门（`lumio/update_notice.rs`，偏好落 `state_dir()/update-notice.json`）：点「稍后」= 持久忽略该版本（下个版本才恢复弹窗）、同一天最多弹一次（UTC epoch 天）；闸门只静默弹窗，绿色标记常驻不受影响

## 相关

- [BestCodex 需求](../../../codex/docs/specs/2026-08-16-bestcodex-rebrand-design.md)
- [架构设计](../../../codex/docs/specs/2026-08-11-lumio-codex-branded-client-design.md)
- [交互设计](../../../codex/docs/specs/2026-08-12-lumio-ux-interaction-design.md)
- [实现计划](../../../codex/docs/plans/2026-08-12-lumio-account-and-home.md)
- [ADR-0001 凭据本地文件](../../decisions/0001-lumio-credentials-local-file.md)
- [ADR-0005 首次安装官方桌面应用](../../decisions/0005-lumio-first-official-app-install.md)
- [ADR-0006 官方应用安装位置](../../decisions/0006-official-app-install-destination.md)
- [ADR-0007 产品站 apex 与门户共存](../../decisions/0007-bestcodex-apex-portal-coexistence.md)
- [ADR-0012 Claude 余额开通](../../decisions/0012-claude-balance-subscribe.md)
- [ADR-0015 Claude 工作台方案 D](../../decisions/0015-claude-workspace-scheme-d.md)
- 可点击原型：`codex/prototypes/bestcodex/`（日常 `app/cc-home-d.html`，初始化 `app/cc-init.html`，重连 `app/cc-resume.html`）
