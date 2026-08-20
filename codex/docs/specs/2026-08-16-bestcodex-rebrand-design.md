# BestCodex 需求梳理

> 状态：设计进行中（2026-08-16）· 第 1–3 步已画  
> 原型：[`prototypes/bestcodex/index.html`](../../prototypes/bestcodex/index.html)  
> 图标：[`codex/assets/brand/bestcodex-icon.jpg`](../../../assets/brand/bestcodex-icon.jpg)  
> 旧 UX「品牌弱化」作废。账户状态机、错误码、零配置、秘密不上屏仍以 [2026-08-12 UX](2026-08-12-lumio-ux-interaction-design.md) 为准。

---

## 1. 产品是什么

**BestCodex 是一个启动器。** 下载一次、登录一次，窗口里两个 Tab：

| Tab | 默认 | 用户得到什么 |
| --- | --- | --- |
| **Codex** | 是 | 配好并启动官方 Codex |
| **Claude** | 否 | 把 Claude 跑在自己的服务器上：项目、终端、双向同步、冲突 |

不是两个安装包，也不是登录后再选一次产品。Tab 一直在，点一下就换。

一句话：一个应用，两种工作方式。

站点：`https://bestcodex.app/`（域名你这边迁，本仓不管 DNS）。

---

## 2. 已经锁定的决定

| 项 | 决定 |
| --- | --- |
| 品牌名 | `BestCodex`（一个词），不另起中文名 |
| 第二个 Tab 名 | **Claude**（用户可见）；仓库里仍可叫 cchaven / CC |
| 信息架构 | 顶栏分段控件 `[ Codex \| Claude ]`，冷启动落在 Codex |
| 图标 | 一对折纸小人：左青绿 = Codex，右珊瑚 = Claude，冰玻璃底 |
| 视觉方向 | 浅色官网 + App 跟系统；苹果系统风，材质要厚（毛玻璃、浅光），不要赛博极光 |
| 图标气质 | 软拟物 / 折纸手感，拟人但不做夸张表情 |
| 账号 | 第一期仍走现有门户 + Sub2API；官网不做第二套登录 |
| API | `api.lumio.games` 不动 |
| 已装用户 | bundle id、本机目录、桌面 Key 名第一期不改 |
| 域名 / 证书 | 你这边做 |

---

## 3. 用户旅程

```
打开 BestCodex
  未登录 → 欢迎页（无 Tab）→ 登录 / 注册 → 准备连接（只配 Codex 本机）
  已登录且配置健康 → 窗口，默认 Codex Tab
  Codex 配置冲突 → 修复页（无 Tab），修好后再露 Tab
  服务不可达但本机已就绪 → 仍进窗口，Codex 离线可启动

Codex Tab：启动 / 安装并启动官方应用
Claude Tab：
  未订阅 → 开通卡（可立刻回 Codex）
  已订阅、没项目 → 说明页 → 连接服务器
  已有项目 → 左项目轨 + 右工作台（终端 / 文件 / 冲突）
```

登录是启动器级的。两个 Tab 共用这一次登录。切 Tab 时另一边不断（安装进度、SSH、同步都还在）。

---

## 4. 各面要交付什么

### 4.1 启动器（主交付）

- 浅色优先、可跟系统深色
- macOS overlay 标题栏；Windows 原生标题栏 + 下面一条 Tab
- Codex Tab：余额一行 + 启动卡，不要仪表盘墙
- Claude Tab：环境，不是再跳进另一个 App
- 设置共用，不是第三个 Tab：账户 / Codex / Claude / 通用 / 支持
- `?` 打开 `https://bestcodex.app/help`

#### 正式图标怎么用

权威图是 `codex/assets/brand/bestcodex-icon.jpg`（一对折纸小人，左青绿 Codex，右珊瑚 Claude，冰玻璃底）。原型副本在 `prototypes/bestcodex/assets/icons/app-icon.jpg`。

- 欢迎页、登录 / 注册、准备连接、官网 Hero、设置账户行、帮助顶栏：都用这一张
- 源图是竖构图，小人偏下、上边工作室留白多。界面用方裁副本（去掉留白、把一对小人抬到视觉中心），再套苹果圆角；不要再垫一层深蓝底。权威原图不改。
- 浅色 / 深色都保留图标自己的冰玻璃盘，只改外阴影；不要另出深色版
- 16–28px 认的是青绿 / 珊瑚两块色，不指望看清脸
- 不再使用旧六边形线标当品牌

#### Codex Tab 三态

主区永远是：问候 + **一行**「邮箱 · 余额 · 充值」+ 一张玻璃启动卡。不要余额 / 套餐 / 模型三张指标墙。

| 态 | 卡上写什么 | 主按钮 | 稿 |
| --- | --- | --- | --- |
| 已就绪 | Codex 已就绪 · 可启动。本机配置已写好 | 启动 Codex | `app/home.html` |
| 未安装 | 尚未安装官方 Codex。进度、失败都留在这张卡里 | 安装并启动官方 Codex | `app/home-install.html` |
| 离线 | 离线可用 · 缓存。充值需要网络，启动不需要 | 启动 Codex（本机已装时） | `app/home-offline.html` |

未安装细稿（交互沿用 2026-08-12 UX，壳换成这张卡）：

1. 点「安装并启动」先出 sheet：**默认位置（/Applications）** / **选择文件夹…**。取消不开始下载。
2. 选定后 sheet 收起，卡内切到进度：下载（百分比 + 已下载 / 总量）→ 校验（往复条）→ 安装（往复条 + 写入路径）。按钮禁用成「正在安装…」。
3. 失败 / 取消：原因常驻卡内（警示色 + 错误码，如 `CODEX_APP_DOWNLOAD_FAILED`），主按钮变「重试」，重试仍先问位置。
4. 切到 Claude，安装继续；回来还在同一张卡。
5. 离线且未安装：按钮禁用，注明「安装官方应用需要网络」。`home-offline.html?missing=1`。

配置冲突仍走无 Tab 的修复页，不在这三态里。

### 4.2 官网 `bestcodex.app`

- 一个站、一个下载。不是两个安装包。
- 官网是完整落地页，不要套在 App 假窗口里。顶栏中间 `[ Codex | Claude ]` 换整页。
- 点 Codex 只显示 Codex 站（现网口径）；点 Claude 只显示 Claude 站。排版跟苹果产品页：大留白、大标题、一块主视觉。
- 官网不放启动器假窗口（你好 Mary / 已就绪卡）。Codex 页按现网站：Hero → 三步 → 下载 → FAQ。Claude 页按现网站：左文右终端 → 防封/同步 → 定价 → 下载 → FAQ。
- **Codex 页**：更快开始使用官方 Codex；三步；下载；FAQ。去掉 OpenAI 花瓣。
- **Claude 页**：不再担心封号；终端；防封 + 同步；¥19.9 / 月；邀请；下载；FAQ。
- 一个安装包。账号 / 帮助共用。
- 帮助中心：安装 / 未签名 / 登录 / 修复 / Claude 连服务器
- 账户按钮跳现有门户
- 页脚：与 OpenAI、Anthropic 无从属

### 4.3 文档 / 帮助 / 可见文案

- README、ops、产品知识里的用户可见名改成 BestCodex
- 站点 URL 改成 `https://bestcodex.app`
- 安装包**显示名**改成 BestCodex
- 历史 ADR / 旧计划标题不改

### 4.4 图标落地

- 权威图：`codex/assets/brand/bestcodex-icon.jpg`
- 之后出 icns / ico / favicon / 托盘，都从这一张出

---

## 5. Claude 怎么接进来（功能需求）

用户看见的是 Claude，能力来自现有 CC 桌面端：授权、SSH、同步、PTY。

- 第一次：先讲清再连服务器，不要一进来就甩 SSH 表单
- 接入：sheet 盖在 Claude Tab 上（主机 → 探测 → 装组件 → 首次同步），不藏外层 Tab
- 接入后：左轨选项目，右面就是终端；点项目只换右侧
- 点回 Codex：Claude 会话和同步继续
- 未订阅：Tab 还在，主区是开通，不挡 Codex

### 5.1 未订阅

`app/cc-subscribe.html`。外层 `[ Codex | Claude ]` 仍在。主区一张开通卡：一句话、价格、主按钮「开通 Claude」、次要链回 Codex Tab。开通走现有账户中心（第一期不在启动器里做支付页）。

### 5.2 第一次说明

`app/cc-empty.html`。已订阅、还没项目。三件事：独立环境、本机仍能改文件（冲突不静默覆盖）、一次登录。主按钮「连接一台服务器」。取消 / 先留在 Codex 都回得去。

### 5.3 连接向导（四步 sheet）

`app/cc-connect.html`。盖在说明页上面，标题栏 Tab 不撤。取消回到说明页。步骤条：主机 / 探测 / 装组件 / 首次同步。

**1. 主机**  
默认只问：主机（公网 IP）、用户（预填 `root`）、密码。提示可粘贴整条 `ssh root@…`。端口、密钥、本机 SSH config 收进「懂 SSH 再用」。密码只留本机。主按钮「探测连接」。

**2. 探测**  
清单：网络可达 → 可以登录 → 系统可用（发行版 / CPU / 内存）。成功后「继续装组件」。失败停在这一步：人话原因 + 错误码 + 三条排查（公网 IP、密码、安全组放行端口），「返回修改」或「重试」。

**3. 装组件**  
人话进度，不出现 agent / tmux：已连上服务器 → 安装同步组件 → 创建项目目录。目录自动预设（远端 `/root/bestcodex/{name}`，本机 `~/BestCodex/{name}`），这一步只展示不填表。

**4. 首次同步**  
文件计数 + 进度条。可以切去 Codex，同步继续。完成后 sheet 关掉，进入左轨 + 右工作台。

能力与排错口径沿用现有 CC 桌面端；BestCodex 只是换成四步可见 sheet，不再把探测藏进「连接并继续」。

### 5.4 接入之后

~~`app/cc-home.html`。左轨：项目列表 + 新建 / 连接新服务器。右工作台默认终端（PTY）。点项目只换右侧。文件 / 冲突 Tab 可见。~~

已由方案 D 取代（项目所有者拍板）：`app/cc-home-d.html`（日常）、`app/cc-init.html`（初始化）、`app/cc-resume.html`（重新进来）。三层模型（服务器 → 项目 → 会话）+ 三栏工作台（左服务器/项目、中会话标签+终端或清单、右文件浏览器）+ 底栏抽屉。五个 stage tab 退场。设计与实现见 [ADR-0015](../../../.spec/decisions/0015-claude-workspace-scheme-d.md) 与 [账户与首页知识](../../../.spec/knowledge/features/lumio-account-and-home.md)。

---

## 6. 明确不做（第一期）

- 不管 DNS / 证书 / 域名解析
- 不改 `api.lumio.games`、不改充值页本身
- 不把门户、CC 独立站整站改成 BestCodex（只改交叉文案和产品卡）
- 不改 bundle id `games.lumio.codex`
- 不改本机状态目录、LaunchAgent 名、桌面 API Key 名
- 不把两个 Rust workspace 焊成一个 crate
- 不为 Claude 再打一份独立安装包
- 不重写历史 ADR
- 官网第一期不做 `bestcodex.app/login`（第二期再说）

---

## 7. 还没画细 / 还没拍板

已画完（原型可点，见第 8 节对照）：正式图标回刷、Codex 三态、Claude 未订阅 / 说明 / 四步向导 / 方案 D 三栏工作台（`cc-home-d.html` / `cc-init.html` / `cc-resume.html`）。旧「右工作台默认终端 + 五个 stage tab」已作废，见 §5.4 与 ADR-0015。

还没画：

- 工作区「文件 / 冲突」在方案 D 右栏 / 底栏抽屉已落地；更细的预览/新建/删除后端仍可再补
- 设置每一组、修复页细稿、帮助中心各篇
- 官网细稿仍可再收：现已是页面级 Codex / Claude 两页 + 共用下载（整合现网两站），帮助各篇还没写完
- 支付、邀请、设备仍在网页账户中心
- 安装包**文件名** `LumioCodex-*.dmg` 第一期可先不动，只改显示名
- Sub2API CORS 要加 `https://bestcodex.app`（服务端，另开运维单）

---

## 8. 设计落地顺序（原型）与产品落地顺序

设计（本文件 + `prototypes/bestcodex/`）：

1. ~~用正式图标回刷欢迎页、官网 Hero、设置、帮助，确认浅 / 深都站得住~~  
2. ~~Codex Tab 三态：已就绪 / 未安装（进度在卡内）/ 离线~~  
3. ~~Claude：未订阅 → 第一次说明 → 向导四步 → 左轨+右工作台~~  
4. ~~工作区内页：终端 / 文件 / 冲突~~ → 方案 D 三栏 + 底栏（见 §5.4）  
5. 设置每一组、修复页、帮助中心各篇  
6. 官网单页收口  
7. 每块回写本规格，保持可点  

产品代码（还没开始）：

1. 图标进安装包 / 托盘 / favicon  
2. 启动器壳：品牌、Tab、浅色视觉、Codex Tab 换肤（逻辑不动）  
3. 官网 + 帮助改成 BestCodex，一个下载  
4. 把 Claude 环境嵌进第二个 Tab（现有 CC 能力搬进来）  
5. 文案、README、ops、显示名收口  

当前停在设计第 3 步之后。产品代码未按这套改。打开 [`prototypes/bestcodex/index.html`](../../prototypes/bestcodex/index.html) 评第 1–3 步。
