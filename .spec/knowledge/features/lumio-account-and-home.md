---
name: lumio-account-and-home
description: BestCodex 桌面账户与首页：注册/登录/2FA、provisioning、问候+一行余额+启动卡、修复与设置、本机缺失时首次安装官方应用——改账户壳或启动编排时查
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
- 不改：bundle id `games.lumio.codex`、LaunchAgent 名 `games.lumio.codex.plist`、二进制 `lumio-codex`。用户可见名是 **BestCodex**；本机状态目录跟 `PRODUCT_NAME`（`project_dirs("games", "Lumio", "BestCodex")`）；桌面 Key `BestCodex Desktop`。安装包**显示名** BestCodex；`.app` 目录 / Windows `InstallDir` / 注册表键仍为 `Lumio Codex`；制品文件名 `LumioCodex-*`。

## 设计

- **设计面**：状态机阶段见架构规格；错误码八域（`AUTH_` / `ACCOUNT_` / `KEY_` / `SERVICE_` / `CODEX_` / `PAYMENT_HANDOFF_` / `PREFERENCE_` / `UPDATE_`）+ `UNKNOWN`（`PREFERENCE_` 随开机启动交付，D-16 时为七域）；凭据本期落本机 owner-only 文件（ADR-0001），非系统钥匙串。
  - 网关面（`/v1/*`，无管理 API 信封）错误体是 `{"code","message"}`：`api.rs` 的 `gateway_error_reason` 按 `code` → `reason` → `error.code` 解析后交给 `normalize_reason`；余额不足（`INSUFFICIENT_BALANCE`）归 `ACCOUNT_INSUFFICIENT_BALANCE`（用户可操作，禁止伪装成宕机），空模型目录归 `SERVICE_MODEL_CATALOG_EMPTY`（D-16）。
  - **交互面**：文案与账户流程以 UX 交互规格为准，Codex 首页壳以 BestCodex 规格为准。顶栏分段控件 `[ Codex | Claude ]`，冷启动落在 Codex；`?` 打开 `https://bestcodex.app/help`。恢复动作按整文件回滚诚实描述；支付 / 遥测本期禁用并附说明（开机启动已交付，见下；更新为「提示 + 用户点击 + 应用内下载 + 安装向导」的手动流程，见「官网与更新提醒」，后台自动安装未开放）。
    **Codex 首页**主区永远是：问候 + **一行**「邮箱 · 余额 · 充值」+ 一张玻璃启动卡。不要余额 / 套餐 / 模型三张指标墙。启动卡三态：已就绪（Codex 已就绪 · 可启动，主按钮「启动 Codex」）；未安装（尚未安装官方 Codex，进度 / 失败都留在这张卡里，主按钮「安装并启动官方 Codex」）；离线（离线可用 · 缓存，充值需要网络、启动不需要）。安装中按钮禁用成「正在安装…」加阶段（下载 / 校验 / 安装），进度条在卡内（下载带百分比与已下载 / 总量，未知总量 / 校验 / 安装为往复动画，D-19）；失败 / 取消原因常驻卡内（警示色 + 错误码，主按钮即重试，D-20），后台提前失败同样收敛为 failed 不许停在「正在安装」；切到 Claude 时安装继续。无应用且离线禁用并注明「安装官方应用需要网络」。配置冲突仍走无 Tab 的修复页，不在这三态里。设置里的重新检测 / 手动选择只留给装过但路径异常的人。
    **工作区 Tab**：`workspace` 是 UI 选择（默认 `codex`），不是 `LumioPhase`。仅 `ready-online` / `ready-offline` 露 `[ Codex | Claude ]`；`signed-out` / `authenticating` / `provisioning` / `needs-repair` 无 Tab。`HomeView` 与 `ClaudeWorkspace` 用 `hidden` 保活，切 Tab 不卸载。Claude 订阅问 `https://api.cc.bestcodex.app/api/v1/me/entitlement`（Sub2API Bearer，成功体 `{data}`）；`none` = 开通卡，开通打开 `https://bestcodex.app/account`，禁止把 `/purchase` 当 Claude 支付。控制面不可达且本地有项目 → 工作台只读/已缓存。四步 sheet 状态在 `lumio/claude/store.ts`；密码只留内存 Map，不进 persistable JSON。PTY / 双向同步未接：终端 Tab 可见，降级为远程命令 + 系统终端。
- **实现面**：
  - Rust：`codex/crates/codex-plus-core/src/lumio/`（api / credentials / secret_file / session / config_takeover / account / launch / official_app_install / autostart / claude_control）
  - 开机启动（壳自身，非官方 Codex）：`autostart.rs` 默认开启（opt-out）——bootstrap 时对从未表达偏好的用户注册一次，用户关闭后偏好落 `state_dir()/launch-at-login.json` 永不自动重开；偏好开着但注册指向旧路径（应用被移动/重装，D-26）时 bootstrap 重对齐到当前 exe；macOS 写 `~/Library/LaunchAgents/games.lumio.codex.plist`（launchd 拉起，第一期不改此名），Windows 经 `reg.exe` 写 HKCU Run（参数走 `Command::args`，无 shell 解析）；cargo 直跑（非 .app bundle / target 目录）不支持并报 `PREFERENCE_LAUNCH_AT_LOGIN_UNSUPPORTED`；系统现状为权威上报，注册被用户从系统设置移除不自动恢复；零新依赖
  - 首次安装：`official_app_install/` 按计划 → 下载 → 校验 → Windows / macOS 适配切开；默认镜像，官方直链 / FE3 备用；进度可轮询，不堵 UI；缓存文件按平台带 `.msix`/`.dmg` 扩展名（裸名会让 Windows 签名/部署工具认不出包，D-21）；镜像 v5 起 sha 缺位时用 per-arch `contentLength` 做尺寸防线；Windows Authenticode 预检三态——钉选通过 / 确凿不匹配拒绝 / 预检不可用仅在侧载路线放行（`Add-AppxPackage` 系统级签名验证 + 装后包族钉选双兜底），便携路线无系统兜底必须硬失败（D-21）；**安装位置可选**（ADR-0006 / D-23）：主按钮先弹「选择安装位置」——Windows「标准安装（MSIX，默认）」与「选择安装目录（便携解压，钉选校验不降级）」并列，macOS 默认 /Applications 可任选；自选目录在下载前先 `create_dir_all` 并探测可写（坏目录不等到 745MB 下载完才暴露，D-28）；安装成功即删安装包（失败保留重试）；自选目录的最终安装路径持久化到 `state_dir()/official-app-path.json` 并优先于自动探测（失效回落，防重启后误判未安装重复装）
  - Tauri：仅 `lumio_` 命令白名单（含 `lumio_install_official_app` / `lumio_official_app_status` / `lumio_cancel_official_app`）；秘密不跨 IPC
  - 前端：`codex/apps/codex-plus-manager/src/LumioApp.tsx` 的 `planStartup` 负责探活 + 接管健康检查后再决定 provisioning / offline-ready / needs-repair（状态机仍有效）；安装进度挂在 Codex 启动卡内，不新增全屏阶段；余额只出现在首页那一行（可刷新），不要独立指标卡。余额刷新三通道（D-29）——ready-online 时 60s 定时轮询、托盘唤起（Rust `show_main_window` 发 `lumio://window-shown`，前端按距上次同步 ≥10s 节流）补刷、余额行内手动刷新，都走 `account-refreshed` 事件（节流判定在 `lumio/account-refresh.ts`）
  - 配置接管以**快照存在性**判定首次，不以 manifest；敏感文件经 `secret_file::write_secret` 创建即 0600；接管产物里 provider 是**内联表**（`model_providers = { lumio = {...} }`），对它做字段增删必须同时覆盖 inline / 标准表两种形态——只认 `as_table_mut` 的移除是死代码（D-22）；bootstrap 对 Healthy 接管做 legacy `env_key` 愈合并同步 manifest 哈希（D-22：旧接管残留该字段时官方 Codex 聊天必报 Missing environment variable，Healthy 状态永不重接管，必须在启动编排前单独清）
  - 启动仍走 `launch::launch_official_codex`（macOS `open -a`，Windows 直接拉官方可执行文件），无注入 / CDP

## 待解决

- Claude 终端无应用内 PTY（无 `portable-pty` 依赖）；双向同步 / 冲突引擎未焊 `fns-*`；装组件只建目录。懂 SSH 的本机 SSH config alias 未接
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
- 旧静态站（过渡）：[`codex/site/`](../../../codex/site/)（历史 `lumio.games`，待 301；DNS 仍待运维）
- 产品站常量 `SITE_BASE_URL` 现为 `https://bestcodex.app`；充值仍 `https://api.lumio.games/purchase`。Sub2API CORS 放行 `https://bestcodex.app` 仍待运维
- 更新提醒：`lumio_check_update` 先对照 GitHub Releases latest，404（内测渠道只发 prerelease，latest API 对其恒 404）时回退读 `/releases` 列表（含 prerelease，跳过 draft，D-27）
- 手动更新（D-30）：不做后台自动安装——检测到新版本时**右下角弹出常驻通知卡**，**绿色标记常驻**于导航「设置」角标、footer 版本旁入口与设置页「发现新版本」条目；任一入口触发 `lumio_download_update`，应用内下载平台安装包（资产按既有 CI 命名选：`-setup-*.exe` / `macos-<arch>-*.dmg`，文件名 `LumioCodex-*` 第一期可不动）到缓存 `updates/` 并打开安装向导，安装由用户在向导里完成。弹窗有频率闸门（`lumio/update_notice.rs`，偏好落 `state_dir()/update-notice.json`）：点「稍后」= 持久忽略该版本（下个版本才恢复弹窗）、同一天最多弹一次（UTC epoch 天）；闸门只静默弹窗，绿色标记常驻不受影响

## 相关

- [BestCodex 需求](../../../codex/docs/specs/2026-08-16-bestcodex-rebrand-design.md)
- [架构设计](../../../codex/docs/specs/2026-08-11-lumio-codex-branded-client-design.md)
- [交互设计](../../../codex/docs/specs/2026-08-12-lumio-ux-interaction-design.md)
- [实现计划](../../../codex/docs/plans/2026-08-12-lumio-account-and-home.md)
- [ADR-0001 凭据本地文件](../../decisions/0001-lumio-credentials-local-file.md)
- [ADR-0005 首次安装官方桌面应用](../../decisions/0005-lumio-first-official-app-install.md)
- 可点击原型：`prototypes/lumio-ux/`
