import { helpCanonicalUrl, helpProductUrl } from "../config";

export interface HelpTopic {
  slug: string;
  title: string;
  summary: string;
  body: string[];
}

export const HELP_TOPICS: HelpTopic[] = [
  {
    slug: "install",
    title: "安装",
    summary: "下载安装包、放到应用程序、第一次打开。",
    body: [
      "BestCodex 只有一个安装包。按你的系统选 Mac · Apple 芯片、Mac · Intel 或 Windows。",
      "macOS：打开 DMG，把 BestCodex 拖进「应用程序」，再从启动台打开。",
      "Windows：运行安装器，按提示完成。若 SmartScreen 拦截，先核对来源再继续。",
      "官方 Codex 应用不捆绑在这个安装包里，需要的话请另外安装。",
    ],
  },
  {
    slug: "unsigned",
    title: "未签名",
    summary: "未签名提示、隔离标记、Windows SmartScreen。",
    body: [
      "内测包尚未签名。系统会把从浏览器下载的应用标上隔离标记，看起来像坏了，其实没有。",
      "这是 Gatekeeper / SmartScreen 的拦截，不是 BestCodex 自己的错误提示。",
      "把应用放到「应用程序」后，在终端执行：xattr -cr \"/Applications/BestCodex.app\"",
      "然后再次打开。仍不行时，按住 Control 再点图标，选「打开」。",
    ],
  },
  {
    slug: "login",
    title: "登录",
    summary: "两个 Tab 共用一次登录，账号在现有门户完成。",
    body: [
      "BestCodex 不做第二套登录。点「账户」或「登录」会回到现有门户。",
      "Codex 与 Claude 共用这一次登录。登录后本机配置会自动写好，不用填服务地址或密钥。",
      "桌面端右上角「?」打开帮助中心；账号、充值、邀请仍在门户账户中心。",
    ],
  },
  {
    slug: "repair",
    title: "修复",
    summary: "配置被改过、冲突提示、恢复到接管前的快照。",
    body: [
      "若本机配置与 BestCodex 要写的内容冲突，启动器会进入修复页，不会在主窗口里硬改。",
      "写入前会先备份。修好后再回到 Codex / Claude 两个 Tab。",
      "可以随时恢复到接管前的快照。仍不行时，到设置 → 支持 → 导出诊断日志。",
    ],
  },
  {
    slug: "claude-server",
    title: "Claude 连服务器",
    summary: "独立环境、固定 IP、首次同步。",
    body: [
      "Claude Tab 把官方 Claude Code 跑在你自己的服务器上：独立环境、固定 IP、持久会话。",
      "第一次会先说明，再连服务器，不会一进来就甩出一长串 SSH 表单。",
      "文件在你的电脑和你的服务器之间双向同步；机密文件默认不同步，冲突不会被静默覆盖。",
    ],
  },
];

export function helpTopicBySlug(slug: string | undefined): HelpTopic | undefined {
  if (!slug) return undefined;
  return HELP_TOPICS.find((topic) => topic.slug === slug);
}

export function helpCanonicalNote(): { canonical: string; product: string } {
  return { canonical: helpCanonicalUrl(), product: helpProductUrl() };
}
