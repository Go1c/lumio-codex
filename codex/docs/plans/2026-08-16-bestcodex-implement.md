# BestCodex 实现开工提示词

> 给执行 Agent 的完整任务。设计权威与可点原型已齐，产品代码还没按这套改。  
> 规格：[`../specs/2026-08-16-bestcodex-rebrand-design.md`](../specs/2026-08-16-bestcodex-rebrand-design.md)  
> 原型：[`../../prototypes/bestcodex/index.html`](../../prototypes/bestcodex/index.html)

把下面「提示词」整段交给开工的 Agent。

---

## 提示词

```
你在仓库 lumio-codex 实现 BestCodex 第一期产品落地。设计已锁定，不要重开产品讨论，不要另出图标，不要改 DNS。

动手前读（按顺序）：
1. .spec/AGENTS.md
2. .spec/knowledge/README.md
3. .spec/rules/system.md
4. .spec/knowledge/features/lumio-account-and-home.md
5. 需求权威：codex/docs/specs/2026-08-16-bestcodex-rebrand-design.md
6. 可点原型：codex/prototypes/bestcodex/index.html
7. 官网稿：codex/prototypes/bestcodex/site/index.html
8. 正式图标：codex/assets/brand/bestcodex-icon.jpg
9. 旧 UX（账户状态机 / 错误码 / 零配置 / 秘密不上屏仍有效）：codex/docs/specs/2026-08-12-lumio-ux-interaction-design.md
10. 用 before-you-code 校准深度后再改文件。

————————————————
产品（已定）
————————————————

BestCodex 是一个启动器。下载一次、登录一次。窗口两个 Tab：

- Codex（默认）：配好并启动官方 Codex
- Claude（用户可见名；仓库里仍可叫 cchaven / CC）：把现有 CC避风港能力搬进这个 Tab（项目、终端、同步、冲突）

站点 https://bestcodex.app/ 。域名 / DNS / 证书不管。
账号第一期仍走现有门户 + Sub2API（api.lumio.games 不动）。
bundle id `games.lumio.codex`、本机状态目录、LaunchAgent 名、桌面 API Key 名第一期不改。
不为 Claude 再打一份独立安装包。不把两个 Rust workspace 焊成一个 crate。
官网第一期不做 bestcodex.app/login。

图标已锁定：一对折纸小人，左青绿 = Codex，右珊瑚 = Claude，冰玻璃底。
权威图：codex/assets/brand/bestcodex-icon.jpg
界面用原型里的方裁副本：codex/prototypes/bestcodex/assets/icons/app-icon.jpg
不要再出新图标方案。源图是竖构图，不要整张铺进圆角；用方裁。不要垫深蓝底。不要另出深色版。

视觉：浅色苹果风 + 软拟物（毛玻璃、浅光）。App 跟系统。不要赛博极光，不要过萌，不要 OpenAI 花瓣。

————————————————
按这个顺序做（做完一块再下一块）
————————————————

1) 图标进安装包 / 托盘 / favicon
   - icns / ico / favicon / 托盘都从权威图出
   - 安装包显示名改成 BestCodex；文件名 LumioCodex-*.dmg 第一期可先不动
   - 用户可见名、站点 URL 改成 BestCodex / https://bestcodex.app
   - 历史 ADR / 旧计划标题不改

2) 启动器壳（逻辑先不动）
   - 品牌、浅色优先、可跟系统深色
   - 顶栏分段控件 [ Codex | Claude ]，冷启动落在 Codex
   - macOS overlay 标题栏；Windows 原生标题栏 + 下面一条 Tab
   - 设置不是第三个 Tab：账户 / Codex / Claude / 通用 / 支持
   - ? 打开 https://bestcodex.app/help
   - Codex Tab 换肤成原型：问候 + 一行「邮箱 · 余额 · 充值」+ 一张玻璃启动卡
   - 不要仪表盘墙（不要余额 / 套餐 / 模型三张指标卡）
   - 三态按原型实现（交互沿用 2026-08-12 UX，只换壳）：
     · 已就绪：codex/prototypes/bestcodex/app/home.html
     · 未安装（选位置 sheet + 进度/失败都在卡内）：app/home-install.html
     · 离线（已装可启动；未装禁用并写明需要网络）：app/home-offline.html
   - 配置冲突仍走无 Tab 修复页
   - 切到 Claude 时 Codex 安装进度不断

3) 官网 + 帮助改成 BestCodex，一个下载
   - 视觉与信息架构以原型为准：codex/prototypes/bestcodex/site/index.html
   - 完整落地页，不要套在 App 假窗口里
   - 顶栏中间 [ Codex | Claude ] 换整页：点 Codex 只出 Codex 页，点 Claude 只出 Claude 页
   - 不要在官网上放启动器假窗口（你好 Mary / 已就绪卡）
   - Codex 页：更快开始使用官方 Codex → 三步开始 → 下载 → FAQ
     不要 Hero 下的「向下滚动」；不要「三步开始」下那句「官方 Codex 需单独安装…」；不要下载区长说明；FAQ 不要「在哪里注册和充值」
   - Claude 页：安心使用 Claude Code / 不再担心封号（左文右终端）→ 防封以及同步 → 简单定价 → 下载 → FAQ
     不要「主因是防封…」副标题；不要「一个价钱，功能全开…」副标题；不要「把 Claude Code 放到自己的服务器上」那条 CTA 带
   - Claude 包月 ¥19.9 / 月；邀请文案写在包月卡「立即订阅」下面，两行：
     🎁 经朋友邀请注册并登录 APP
     首月免费（每个账号限一次）
   - 一个安装包（Mac Apple / Mac Intel / Windows）
   - 账户按钮跳现有门户
   - 页脚：与 OpenAI、Anthropic 无从属
   - 帮助中心至少：安装 / 未签名 / 登录 / 修复 / Claude 连服务器
   - 现有 web/apps/codex 与 web/apps/cc 两站内容迁到统一站壳下，用户可见名改 BestCodex；不要把门户整站改成 BestCodex
   - Sub2API CORS 要加 https://bestcodex.app（服务端另开运维单，这里先记下，不要擅自改生产）

4) 把 Claude 环境嵌进第二个 Tab（现有 CC 能力搬进来）
   - 对照原型：
     · 未订阅：app/cc-subscribe.html（¥19.9，可回 Codex）
     · 第一次说明：app/cc-empty.html（先讲清再连，不甩 SSH 表单）
     · 四步 sheet：app/cc-connect.html（主机 → 探测 → 装组件 → 首次同步），盖在 Claude Tab 上，外层 Tab 不撤
     · 接入后：app/cc-home.html（左项目轨 + 右终端）
   - 能力来自现有 cchaven 桌面端：授权、SSH、同步、PTY
   - 点回 Codex：Claude 会话和同步继续
   - 文件 / 冲突 Tab 先可见；细交互沿用现有 CC 桌面端，只换壳。若来不及画细则先终端可用
   - 开通走现有账户中心，第一期不在启动器里做支付页

5) 文案、README、ops、显示名收口
   - 用户可见名与站点 URL 收口为 BestCodex / https://bestcodex.app
   - 历史 ADR / 旧计划标题不改

————————————————
明确不要做
————————————————

- 不管 DNS / 证书 / 域名解析
- 不改 api.lumio.games、不改充值页本身
- 不改 bundle id、本机目录、LaunchAgent 名、桌面 Key 名
- 不焊两个 Rust workspace
- 不为 Claude 再打独立安装包
- 不重写历史 ADR
- 不做 bestcodex.app/login
- 不另出图标
- 不重开「要不要两个产品」的讨论

————————————————
验收
————————————————

- 对照原型逐屏：欢迎 / 登录 / Codex 三态 / Claude 未订阅→说明→四步→工作台 / 官网两个页签
- 官网点 Codex / Claude 是换整页，不是同一页里两套内容互藏一半
- 顶栏「下载」按钮字必须看得见（不要黑底黑字）
- 浅色 / 深色都过一遍
- Web 改动用浏览器点完主路径再声称完成
- 收口门槛按 .spec/AGENTS.md：相关目录的 cargo fmt/test、npm run check / test / 构建；只跑与改动相关的组合
- 交付按交回物格式：改动清单、验证证据（命令与关键输出）、known gaps、知识沉淀落点或声明无需沉淀

先做第 1、2 步。做完给我看，再往下做官网和 Claude Tab。
```
