/** Claude 站文案。价格改动需要同步这里与 Sub2API 收银台。 */

export const TERMINAL_SHOT = [
  "BestCodex · Claude · my-project",
  "attached  session cc-my-project  ·  tmux",
  "",
  "> claude",
  "Claude Code · my-project",
  "How can I help you today?",
  "",
  "> _",
];

export const VALUES = [
  {
    title: "防封方案",
    body: "独立服务器环境、固定 IP、持久会话，与本机完全隔离，大幅降低封号风险。即使连接中断，对话上下文也不会丢失。",
  },
  {
    title: "双向安全同步",
    body: "远端改动回到本机，本机编辑上到服务器。机密文件默认不同步；出现冲突时并排对比，绝不静默覆盖。",
  },
];

export const PLAN = {
  tag: "唯一套餐",
  name: "Claude 包月",
  price: "¥19.9",
  per: "随时取消",
  features: ["防封服务器环境", "双向安全同步", "持久终端", "不限工作区数量"],
  noLimits: "没有其他限制，就一个价钱。",
  inviteLine: "🎁 经朋友邀请注册并登录 APP",
  inviteOnce: "首月免费（每个账号限一次）",
};

export const FAQS: Array<[string, string]> = [
  [
    "有没有免费版？",
    "没有免费版。经朋友的邀请链接注册并登录后，可免费试用一个月，功能与正式订阅一样。",
  ],
  ["订阅有什么限制？", "没有。不限工作区、不限同步用量，所有功能全开，就一个价钱。"],
  [
    "你们会存储我的源代码吗？",
    "文件只在你的电脑和你的服务器之间同步。我们只存账号与工作区元数据。",
  ],
  ["可以随时取消吗？", "可以。订阅维持到本期结束，之后不再扣款。你的文件不会被删除。"],
];
