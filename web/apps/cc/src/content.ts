/** 文案迁自 cchaven/apps/web（zh-CN 字典与营销页），保持与线上 cc 站一致。 */

export const TERMINAL_SHOT = [
  "$ ssh dev-server",
  "Attached to session cc-my-project",
  "",
  "> claude",
  "╭──────────────────────────────────────────╮",
  "│  Claude Code · my-project                │",
  "│  How can I help you today?               │",
  "╰──────────────────────────────────────────╯",
  "",
  "> Refactor the sync engine to batch writes_",
];

export const VALUES = [
  {
    icon: "🛡️",
    title: "防封方案",
    body: "独立服务器环境＋固定 IP＋持久 tmux 会话，与本机环境完全隔离，大幅降低封号风险。即使连接中断，对话上下文也不会丢失。",
  },
  {
    icon: "🔄",
    title: "双向安全同步",
    body: "远端改动实时回到本机，本机编辑实时上到服务器。机密文件（.env、密钥）默认永不同步；出现冲突时并排对比，绝不静默覆盖。",
  },
];

/** 单一套餐。价格改动需要同步这里与 Sub2API 收银台。 */
export const PLAN = {
  tag: "唯一套餐",
  name: "CC避风港包月",
  price: "¥68",
  per: "每月 · 随时取消",
  features: [
    "防封服务器环境",
    "双向安全同步",
    "持久终端（tmux）",
    "不限工作区数量",
    "邮件支持",
  ],
  noLimits: "没有其他限制，就一个价钱。",
  inviteNote: "🎁 经朋友邀请注册并登录 APP，首月免费（每个账号限一次）。",
};

export const FAQS: Array<[string, string]> = [
  [
    "有没有免费版？",
    "没有免费版，但经朋友的邀请链接注册并登录 APP，即可免费试用一个月，功能与正式订阅完全一样。",
  ],
  [
    "首月免费试用怎么领取？",
    "经朋友的邀请链接下载、注册并首次登录 APP 后自动发放。每个账号一生只可享用一次。",
  ],
  ["订阅有什么限制？", "没有。不限工作区数量、不限同步用量，所有功能全开，就一个价钱。"],
  [
    "你们会存储我的源代码吗？",
    "文件内容只在你的 Mac 与你的服务器之间直接同步；控制面只存储账号与工作区元数据。",
  ],
  ["可以随时取消吗？", "可以。订阅会维持到本期结束，之后不再扣款；你的文件不会被删除。"],
];

export const DOWNLOAD_STEPS = [
  "打开 DMG，将「CC避风港」拖入「应用程序」文件夹。",
  "启动应用，点「通过浏览器登录」完成授权。",
  "连接你的服务器，创建第一个工作区。",
];

export interface DownloadLinks {
  arm?: string;
  intel?: string;
  version?: string;
}

/**
 * 安装包地址由部署环境注入：产品站是纯静态站点，不调用 cchaven 控制面的接口。
 * 未配置时下载页给空态，绝不渲染坏链接。
 */
export function ccDownloadLinks(): DownloadLinks {
  const env = import.meta.env as Record<string, string | undefined>;
  const pick = (key: string) => (env[key]?.trim() ? env[key]?.trim() : undefined);
  return {
    arm: pick("VITE_CC_DOWNLOAD_ARM_URL"),
    intel: pick("VITE_CC_DOWNLOAD_INTEL_URL"),
    version: pick("VITE_CC_VERSION"),
  };
}
