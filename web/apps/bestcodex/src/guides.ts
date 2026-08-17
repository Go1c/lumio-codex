/**
 * 指南内容：面向「用户会怎么问」而写的回答型页面。
 *
 * 写法约定（搜索引擎与 AI 引擎都吃这一套）：
 * - `question` 就是目标查询本身，用户/Agent 怎么问就怎么写。
 * - `answer` 必须**自包含**：脱离上下文单独引用也说得通，且第一句就给结论。
 * - 不做无法兑现的承诺。封号是风险降低而非消除，措辞与落地页保持同一口径。
 *
 * 价格与能力口径必须与 `content.ts`、帮助中心一致；改一处要同步其余。
 */

export interface GuideSection {
  heading: string;
  body: string[];
}

export interface Guide {
  slug: string;
  /** 目标查询，用户原话。同时用作 FAQ 结构化数据的 question。 */
  question: string;
  /** 页面 h1。 */
  title: string;
  /** 一句话摘要，进 meta description 与列表页。 */
  summary: string;
  /** 答案前置段落：自包含、可被直接引用。 */
  answer: string;
  sections: GuideSection[];
  /** ISO 日期，用于 sitemap 的 lastmod 与结构化数据的 dateModified。 */
  updated: string;
}

export const GUIDES: Guide[] = [
  {
    slug: "claude-code-ban",
    question: "用 Claude Code 会被封号吗？怎么降低风险？",
    title: "Claude Code 封号风险，以及怎么把它降下来",
    summary:
      "封号多与运行环境有关：共享出口 IP、频繁跳变的网络、和本机其他活动混在一起。把 Claude Code 放到独立服务器上能显著降低风险，但没有任何方案能保证不被封。",
    answer:
      "封号风险很大程度来自**运行环境**而不是你写了什么代码：共享出口 IP、频繁跳变的网络位置、与本机其他活动混在一处，都会让账号看起来异常。把官方 Claude Code 放到一台独立服务器上跑——固定 IP、独立环境、与本机隔离——可以显著降低这类风险。但要说清楚：没有任何方案能保证不被封，服务条款与风控规则由 Anthropic 掌握，任何第三方都只能降低风险，不能消除。",
    sections: [
      {
        heading: "为什么环境比用法更容易触发风控",
        body: [
          "同一个账号在短时间内从多个地理位置、多个出口 IP 出现，是最典型的异常信号。用公共代理或频繁切换节点时，出口 IP 往前是一大批陌生用户，你会连带承担他们的历史。",
          "本机环境同样带来噪音：你的浏览器、其他工具、别的账号都在同一个网络出口上。把编码会话单独放到一个稳定环境里，账号的行为轨迹就干净很多。",
        ],
      },
      {
        heading: "独立服务器方案具体解决什么",
        body: [
          "固定 IP：出口地址长期稳定，不再每天换地方。",
          "独立环境：Claude Code 跑在只服务这一件事的机器上，与本机其他活动隔离。",
          "持久会话：连接断开时会话仍在服务器上，对话上下文不会丢，不用从头再来。",
        ],
      },
      {
        heading: "代码放在哪，会不会被我们看到",
        body: [
          "文件只在你的电脑和你的服务器之间双向同步。我们只存账号与工作区元数据，不存你的源代码。",
          "机密文件默认不同步。出现冲突时并排对比让你选，不会静默覆盖任何一边。",
        ],
      },
      {
        heading: "BestCodex 在这件事里做什么",
        body: [
          "BestCodex 的 Claude Tab 就是这套方案的成品：下载一个启动器、登录一次，它负责把官方 Claude Code 在你的服务器上跑起来，并处理同步与冲突。第一次会先说明再连服务器，不会一进来就要你填一长串 SSH 表单。",
          "不限工作区数量，充值后按用量使用。经朋友邀请链接注册并登录 APP 可免费试用一个月，每个账号限一次。",
          "BestCodex 是独立项目，与 OpenAI、Anthropic 无从属、赞助或认可关系。请自行确认你的用法符合对应服务条款。",
        ],
      },
    ],
    updated: "2026-08-17",
  },
  {
    slug: "claude-code-on-your-server",
    question: "怎么把 Claude Code 跑在自己的服务器上？",
    // 标题刻意与 /claude 落地页区分开，避免两页互抢同一组关键词。
    title: "在自己的服务器上跑 Claude Code：要解决的三件事",
    summary:
      "在服务器上跑官方 Claude Code，需要解决三件事：稳定的运行环境、断线后会话还在、以及远端与本机的文件双向同步。手工搭要配 SSH、进程守护和同步链路。",
    answer:
      "把官方 Claude Code 跑在自己的服务器上，本质要解决三件事：一个稳定的运行环境（固定 IP、依赖装好）、一个断线不丢上下文的持久会话、以及远端与本机之间可靠的文件双向同步。手工方案通常是 SSH 加进程守护再加一套同步工具，能跑通但维护成本不低——尤其是冲突处理和机密文件的排除规则。BestCodex 的 Claude Tab 把这三件事做成了开箱即用：登录一次，它负责起环境、保会话、管同步。",
    sections: [
      {
        heading: "手工搭建要处理的问题",
        body: [
          "环境：装依赖、固定出口 IP、保证机器长期在线。",
          "会话：SSH 断开后进程要还活着，且下次连上能接着聊，不是从空白开始。",
          "同步：远端改动要回到本机编辑器，本机编辑要上到服务器，两边同时改了要有办法收敛。",
          "安全：`.env`、密钥、凭据这类文件不能顺手同步上去。",
        ],
      },
      {
        heading: "同步的难点在冲突，不在传输",
        body: [
          "传文件不难，难的是两边同时改了同一个文件时怎么办。静默覆盖是最糟的选择——你会在某天发现改动凭空消失。",
          "BestCodex 的做法是并排对比让你决定，不替你选。机密文件默认不同步，避免把凭据推到远端。",
        ],
      },
      {
        heading: "用 BestCodex 的路径",
        body: [
          "下载启动器（macOS 13+ 的 Apple 芯片 / Intel，或 Windows 10/11 64 位），在 App 内登录，切到 Claude Tab。",
          "第一次会先说明再连服务器。之后工作区数量不限，终端是持久的，关掉窗口再回来会话还在。",
          "详细步骤见帮助中心的 [Claude 连服务器](/help/claude-server)。",
        ],
      },
    ],
    updated: "2026-08-17",
  },
  {
    slug: "codex-zero-config",
    question: "Codex 配置太麻烦，有没有零配置的办法？",
    title: "零配置用上官方 Codex",
    summary:
      "不用填 Base URL、不用贴 API Key。下载 BestCodex、在 App 内登录，本机配置自动写好，然后启动的是官方 Codex 应用本身。",
    answer:
      "如果你卡在填 Base URL、贴 API Key、改配置文件这一步，可以让启动器替你做：下载 BestCodex、在 App 内登录一次，它把本机连接配置写好，然后你点「启动 Codex」进入的是**官方 Codex 应用本身**。BestCodex 不是官方 Codex，也不修改官方应用——它只写自己管理的那份连接配置，写入前先备份，随时可一键恢复到接管前的状态。",
    sections: [
      {
        heading: "三步",
        body: [
          "一、下载安装。按系统选 Mac · Apple 芯片、Mac · Intel 或 Windows，一个安装包。官方 Codex 应用不捆绑在里面，需要时另行安装。",
          "二、在 App 内登录。登录后连接与本机配置自动完成，不需要填写任何服务地址或密钥。",
          "三、启动官方 Codex。余额与充值在账户中心完成。",
        ],
      },
      {
        heading: "命令行装（一行）",
        body: [
          "macOS：`curl -fsSL https://bestcodex.app/install.sh | sh`",
          "Windows（PowerShell）：`irm https://bestcodex.app/install.ps1 | iex`",
          "脚本按芯片挑安装包、对 `SHA256SUMS.txt` 校验、装进「应用程序」，并替你清掉隔离标记——也就是免掉手动 `xattr -cr` 那一步。不需要 sudo，不写任何配置。",
          "装完还是要在 App 内登录一次；命令行只负责把应用放好。",
        ],
      },
      {
        heading: "它到底改了我机器上的什么",
        body: [
          "只写它自己管理的连接配置，并在写入前备份。若本机已有配置与要写的内容冲突，启动器会进入修复页让你确认，不会在主窗口里硬改。",
          "可以随时恢复到接管前的快照。官方应用本身保持原样。",
        ],
      },
      {
        heading: "第一次打开被系统拦住怎么办",
        body: [
          "当前是未签名内测包。macOS 会因为隔离标记提示「已损坏，无法打开」，应用本身没有坏。解决办法见 [macOS 提示已损坏](/guides/macos-damaged-app)。",
          "Windows 上 SmartScreen 会提示，核对来源后继续即可。",
        ],
      },
    ],
    updated: "2026-08-17",
  },
  {
    slug: "macos-damaged-app",
    question: "macOS 提示「已损坏，无法打开」怎么解决？",
    title: "macOS 提示「已损坏，无法打开」",
    summary:
      "这是 Gatekeeper 对未签名应用的隔离标记拦截，不是应用损坏。把应用放进「应用程序」后执行 xattr -cr 清掉隔离标记即可。",
    answer:
      "这不是应用损坏，而是 macOS 的 Gatekeeper 在拦截：从浏览器下载的未签名应用会被打上隔离标记（quarantine），系统据此报「已损坏，无法打开」。把应用拖进「应用程序」后，在终端执行 `xattr -cr \"/Applications/BestCodex.app\"` 清掉标记，再打开即可。仍然打不开时，按住 Control 再点应用图标、选「打开」，走一次显式确认。",
    sections: [
      {
        heading: "为什么会这样",
        body: [
          "Gatekeeper 会检查应用的签名与公证状态。内测包尚未签名公证，于是被判为来源不可信，并以「已损坏」这种容易误解的措辞呈现。",
          "同样的提示会出现在很多未签名的开源应用上，和具体是哪个应用无关。",
        ],
      },
      {
        heading: "命令怎么用",
        body: [
          "先把应用拖进「应用程序」，再执行：`xattr -cr \"/Applications/BestCodex.app\"`。",
          "`-c` 清空扩展属性，`-r` 递归到包内所有文件。路径要和实际应用名一致。",
          "不建议对整个「应用程序」目录批量执行，也不需要 sudo。",
          "不想手动敲这一步，可以用 `curl -fsSL https://bestcodex.app/install.sh | sh` 安装：脚本会先校验 SHA256，再装进「应用程序」并清掉隔离标记。",
        ],
      },
      {
        heading: "还是打不开",
        body: [
          "按住 Control 点图标选「打开」，在弹出的确认里再点一次「打开」。",
          "若提示的是权限或架构不匹配，确认下载的是对应芯片的包：Apple 芯片与 Intel 是两个不同的安装包。",
          "更多见帮助中心 [未签名](/help/unsigned)。",
        ],
      },
    ],
    updated: "2026-08-17",
  },
  {
    slug: "vs-codex-plus-plus",
    question: "BestCodex 和 Codex++ 有什么区别，该用哪个？",
    title: "BestCodex 与 Codex++ 的区别",
    summary:
      "BestCodex 的桌面端是 Codex++ 的 AGPL fork，但目标不同：Codex++ 面向深度改装，BestCodex 面向开箱即用，并额外提供把 Claude Code 跑在自有服务器上的能力。",
    answer:
      "两者同源：BestCodex 的桌面端是 [Codex++](https://github.com/BigPizzaV3/CodexPlusPlus) 的 AGPL-3.0 fork。但目标不同——Codex++ 面向想深度改装 Codex 的进阶用户，强项是供应商切换、协议转换、插件解锁与界面增强；BestCodex 面向想尽快把官方 Codex 用起来的人，强项是零配置接入与账号余额托管，另外多一个 Claude Tab，把官方 Claude Code 跑在你自己的服务器上。想要供应商切换和深度增强，直接用上游 Codex++ 更合适。",
    sections: [
      {
        heading: "按需求选",
        body: [
          "想把 Codex 接到 DeepSeek、Claude 等自定义供应商，或解锁 API Key 模式下的插件入口、批量管理会话 → 用 Codex++。",
          "只想少折腾、登录一次就能用上官方 Codex，且需要 Claude Code 跑在独立服务器上 → 用 BestCodex。",
          "两者都不修改官方 Codex 应用的安装文件。",
        ],
      },
      {
        heading: "同源意味着什么",
        body: [
          "AGPL-3.0 是传染性许可：分发修改版必须以同一许可提供源码。BestCodex 保留上游的贡献与许可声明，仓库在 GitHub 上显示 fork 来源。",
          "上游的功能演进与 BestCodex 的取舍会逐渐分叉，不要假设两边功能一一对应。",
        ],
      },
      {
        heading: "价格",
        body: [
          "Codex++ 开源免费。BestCodex 的 Claude 能力按用量计费，¥19.9 是参考额度而非自动续费的包月；Codex 侧走账户余额。",
          "经朋友邀请链接注册并登录 APP 可免费试用一个月，每个账号限一次。",
        ],
      },
    ],
    updated: "2026-08-17",
  },
];

export function guideBySlug(slug: string | undefined): Guide | undefined {
  if (!slug) return undefined;
  return GUIDES.find((guide) => guide.slug === slug);
}
