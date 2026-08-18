import { Link } from "react-router-dom";

export type LegalKind = "privacy" | "terms";
export type LegalLocale = "zh" | "en";

interface LegalCopy {
  title: string;
  lede: string;
  otherLanguage: string;
  otherHref: string;
  home: string;
  homeHref: string;
  sections: Array<{ heading: string; body: string[] }>;
}

const PAGES: Record<LegalLocale, Record<LegalKind, LegalCopy>> = {
  zh: {
    privacy: {
      title: "隐私政策",
      lede: "适用于 BestCodex（bestcodex.app）。运营主体、地址、联系邮箱与备案/ICP 号将补充。",
      otherLanguage: "English",
      otherHref: "/en/privacy",
      home: "返回首页",
      homeHref: "/",
      sections: [
        {
          heading: "本机会改写哪些文件",
          body: [
            "本产品会改写官方 Codex 用户目录里的 ~/.codex/config.toml 和 ~/.codex/auth.json。若设置了 $CODEX_HOME，则写到那个目录。",
            "写入内容包括 model、model_provider=lumio、[model_providers.lumio]，以及 auth.json 里的 OPENAI_API_KEY（BestCodex 签发的访问密钥，不是官方 ChatGPT 登录态）。这样会把官方 Codex 的请求指到远程中转 https://api.lumio.games/v1。",
          ],
        },
        {
          heading: "不捆绑、不修改官方应用",
          body: [
            "不捆绑、不下载、不修改、不注入官方 Codex / ChatGPT 应用本身，只检测并启动你已经安装的官方应用。",
          ],
        },
        {
          heading: "账号存在哪",
          body: [
            "BestCodex 账号令牌存在本机自己的 credentials.json，不是系统钥匙串，也不是 Windows Credential Manager。",
          ],
        },
        {
          heading: "退出登录不会自动恢复官方配置",
          body: ["退出登录只删除 BestCodex 凭据，不会自动恢复官方 Codex 配置。恢复是应用内的单独操作。"],
        },
        {
          heading: "本机不做网络劫持",
          body: ["本机不起代理、不改 hosts、不装证书。"],
        },
        {
          heading: "本站收集什么",
          body: [
            "bestcodex.app 是产品介绍站。注册、登录与充值在账号门户完成，本站不另建一套账号。",
          ],
        },
      ],
    },
    terms: {
      title: "服务条款",
      lede: "适用于 BestCodex（bestcodex.app）。运营主体、地址、联系邮箱与备案/ICP 号将补充。",
      otherLanguage: "English",
      otherHref: "/en/terms",
      home: "返回首页",
      homeHref: "/",
      sections: [
        {
          heading: "这是什么",
          body: [
            "BestCodex 是独立的辅助接入工具，帮你完成账号登录和本机连接配置，让你更快用上已经安装的官方 Codex。你日常使用的始终是官方 Codex 应用。本产品与 OpenAI、Codex、ChatGPT、Anthropic 无从属关系。",
          ],
        },
        {
          heading: "本机配置（你需要知情并同意）",
          body: [
            "使用本产品即表示你同意它改写官方 Codex 用户目录里的 ~/.codex/config.toml 和 ~/.codex/auth.json（若设置了 $CODEX_HOME 则写那里），把官方 Codex 的请求指到远程中转 https://api.lumio.games/v1。写入包括 model、model_provider=lumio、[model_providers.lumio]，以及 auth.json 的 OPENAI_API_KEY（BestCodex 签发的访问密钥，不是官方 ChatGPT 登录态）。",
          ],
        },
        {
          heading: "不捆绑、不修改官方应用",
          body: [
            "不捆绑、不下载、不修改、不注入官方 Codex / ChatGPT 应用本身，只检测并启动你已安装的官方应用。",
          ],
        },
        {
          heading: "账号、退出与恢复",
          body: [
            "BestCodex 账号令牌存在本机 credentials.json（不是钥匙串 / Credential Manager）。退出登录只删除 BestCodex 凭据，不会自动恢复官方 Codex 配置；恢复是应用内单独操作。",
          ],
        },
        {
          heading: "本机网络",
          body: ["本机不起代理、不改 hosts、不装证书。"],
        },
        {
          heading: "开源与免责",
          body: [
            "本软件以 AGPL-3.0-only 开源。官方 Codex 的可用性与账号政策由其各自所有者决定；本工具只做接入配置，不保证官方应用始终可用。",
          ],
        },
      ],
    },
  },
  en: {
    privacy: {
      title: "Privacy Policy",
      lede: "This applies to BestCodex (bestcodex.app). Operator, address, contact email, and filing numbers will be added.",
      otherLanguage: "中文",
      otherHref: "/privacy",
      home: "Back to home",
      homeHref: "/en",
      sections: [
        {
          heading: "What this app rewrites on your machine",
          body: [
            "BestCodex rewrites the official Codex user directory files ~/.codex/config.toml and ~/.codex/auth.json. If $CODEX_HOME is set, it writes there instead.",
            "What it writes includes model, model_provider=lumio, [model_providers.lumio], and the OPENAI_API_KEY in auth.json (a BestCodex-issued access key, not an official ChatGPT login). That points official Codex requests at the relay https://api.lumio.games/v1.",
          ],
        },
        {
          heading: "It does not bundle or modify the official app",
          body: [
            "It does not bundle, download, modify, or inject the official Codex / ChatGPT app. It only detects and launches the official app you already installed.",
          ],
        },
        {
          heading: "Where the BestCodex account lives",
          body: [
            "BestCodex account tokens live in a local credentials.json file, not the system keychain or Windows Credential Manager.",
          ],
        },
        {
          heading: "Signing out does not restore official config",
          body: [
            "Signing out only deletes BestCodex credentials. It does not restore official Codex config. Restore is a separate in-app action.",
          ],
        },
        {
          heading: "No local network interception",
          body: ["It does not start a local proxy, edit hosts, or install certificates."],
        },
        {
          heading: "What this site collects",
          body: [
            "bestcodex.app is the product site. Sign-up, sign-in, and top-up happen on the account portal. This site does not keep a second account system.",
          ],
        },
      ],
    },
    terms: {
      title: "Terms of Service",
      lede: "This applies to BestCodex (bestcodex.app). Operator, address, contact email, and filing numbers will be added.",
      otherLanguage: "中文",
      otherHref: "/terms",
      home: "Back to home",
      homeHref: "/en",
      sections: [
        {
          heading: "What this is",
          body: [
            "BestCodex is an independent helper that signs you in and writes the local connection config so you can start the official Codex you already installed. Day to day you use the official Codex app. This product is not affiliated with OpenAI, Codex, ChatGPT, or Anthropic.",
          ],
        },
        {
          heading: "Local config (you need to know and agree)",
          body: [
            "By using this product you agree that it rewrites ~/.codex/config.toml and ~/.codex/auth.json in the official Codex user directory (or $CODEX_HOME if set) and points official Codex requests at the relay https://api.lumio.games/v1. That includes model, model_provider=lumio, [model_providers.lumio], and the OPENAI_API_KEY in auth.json (a BestCodex-issued access key, not an official ChatGPT login).",
          ],
        },
        {
          heading: "It does not bundle or modify the official app",
          body: [
            "It does not bundle, download, modify, or inject the official Codex / ChatGPT app. It only detects and launches the official app you already installed.",
          ],
        },
        {
          heading: "Account, sign-out, and restore",
          body: [
            "BestCodex account tokens live in local credentials.json (not the keychain / Credential Manager). Signing out only deletes BestCodex credentials and does not restore official Codex config; restore is a separate in-app action.",
          ],
        },
        {
          heading: "Local network",
          body: ["It does not start a local proxy, edit hosts, or install certificates."],
        },
        {
          heading: "Open source and disclaimer",
          body: [
            "This software is open source under AGPL-3.0-only. Availability of official Codex and its account policies are decided by their owners. This tool only writes access config and does not guarantee the official app stays available.",
          ],
        },
      ],
    },
  },
};

const CODE_PATTERN = /(`[^`]+`|https?:\/\/[^\s]+)/g;

function renderInline(text: string, keyPrefix: string) {
  const parts = text.split(CODE_PATTERN);
  return parts.map((part, index) => {
    const key = `${keyPrefix}-${index}`;
    if (part.startsWith("`") && part.endsWith("`")) {
      return <code key={key}>{part.slice(1, -1)}</code>;
    }
    if (/^https?:\/\//.test(part)) {
      return (
        <code key={key}>{part}</code>
      );
    }
    return <span key={key}>{part}</span>;
  });
}

export function LegalPage({
  kind,
  locale = "zh",
}: {
  kind: LegalKind;
  locale?: LegalLocale;
}) {
  const copy = PAGES[locale][kind];

  return (
    <article className="help-page">
      <h1>{copy.title}</h1>
      <p className="help-lede">{copy.lede}</p>
      {copy.sections.map((section) => (
        <section key={section.heading}>
          <h2>{section.heading}</h2>
          {section.body.map((paragraph, index) => (
            <p key={paragraph}>{renderInline(paragraph, `${section.heading}-${index}`)}</p>
          ))}
        </section>
      ))}
      <p className="help-canonical">
        <Link to={copy.homeHref}>{copy.home}</Link>
        {" · "}
        <Link to={copy.otherHref}>{copy.otherLanguage}</Link>
      </p>
    </article>
  );
}
