/**
 * 每条路由的 SEO 元数据与结构化数据，单一权威。
 *
 * 同一份数据被三处消费：
 * 1. 构建期预渲染把它注入静态 HTML 的 `<head>`（爬虫只读得到这一份）；
 * 2. 客户端换页时同步 `document.title`；
 * 3. `scripts/prerender.mjs` 据此生成 `sitemap.xml` 与 `llms.txt`。
 *
 * 事实口径必须与 `content.ts`、`guides.ts`、帮助中心一致——这里是第三处引用价格与能力的
 * 地方，改价或改能力要同步全部，并同步 Sub2API 收银台。
 */

import { HELP_TOPICS, productSiteOrigin } from "@lumio/ui";

import { CLAUDE_FAQS, CODEX_FAQS, PLAN } from "@/content";
import { CLAUDE_FAQS_EN, CODEX_FAQS_EN, PLAN_EN } from "@/content.en";
import { GUIDES } from "@/guides";
import { GUIDES_EN } from "@/guides.en";

/** GitHub 仓库：结构化数据用它做实体绑定，和同名无关站点区分开。 */
export const REPO_URL = "https://github.com/LumioGames/lumio-codex";
export const BING_SITE_VERIFICATION = "48232FF4A9EAB80D49C7A5AE2D009539";
const UPSTREAM_URL = "https://github.com/BigPizzaV3/CodexPlusPlus";
const LAST_REVIEWED = "2026-08-17";

export type Locale = "zh-CN" | "en";

export interface RouteSeo {
  /** 站内路径，同时是预渲染产物的目录名。 */
  path: string;
  title: string;
  description: string;
  /** 规范路径。`/codex` 与 `/` 内容相同，统一指向 `/` 消除重复内容。 */
  canonicalPath: string;
  lastmod: string;
  changefreq: "daily" | "weekly" | "monthly";
  priority: number;
  /** 注入 `<head>` 的 JSON-LD。 */
  jsonLd: Record<string, unknown>[];
  /** llms.txt 用的一句话说明；缺省用 description。 */
  llmsNote?: string;
  /** 页面语言，决定预渲染产出的 `<html lang>`。 */
  locale: Locale;
  /**
   * 另一语言版本的路径。只在两种语言都存在时填，双向互指——单向 hreflang 会被引擎忽略。
   * x-default 一律指中文版（主语言）。
   */
  alternatePath?: string;
}

export function siteOrigin(): string {
  return productSiteOrigin();
}

export function absoluteUrl(path: string): string {
  const origin = siteOrigin();
  return path === "/" ? `${origin}/` : `${origin}${path}`;
}

function faqPage(pairs: Array<[string, string]>): Record<string, unknown> {
  return {
    "@context": "https://schema.org",
    "@type": "FAQPage",
    mainEntity: pairs.map(([question, answer]) => ({
      "@type": "Question",
      name: question,
      acceptedAnswer: { "@type": "Answer", text: answer },
    })),
  };
}

function organization(): Record<string, unknown> {
  return {
    "@context": "https://schema.org",
    "@type": "Organization",
    name: "BestCodex",
    url: absoluteUrl("/"),
    logo: absoluteUrl("/bestcodex-icon.jpg"),
    // sameAs 把品牌钉在这两个来源上。网上存在同名但无关的站点（例如 bestcodex.xyz
    // 这类 API 中转站），实体绑定是让引擎不要混淆的主要手段。
    sameAs: [REPO_URL],
    disambiguatingDescription:
      "BestCodex 是 bestcodex.app 提供的桌面启动器，与任何同名的第三方 API 中转服务无关。与 OpenAI、Anthropic 无从属、赞助或认可关系。",
  };
}

function website(): Record<string, unknown> {
  return {
    "@context": "https://schema.org",
    "@type": "WebSite",
    name: "BestCodex",
    url: absoluteUrl("/"),
    inLanguage: "zh-CN",
    publisher: { "@type": "Organization", name: "BestCodex", url: absoluteUrl("/") },
  };
}

/**
 * 桌面启动器本体。`offers` 只在 Claude 页给出：Claude 侧是充值制（¥19.9 为参考额度，
 * 不是自动续费包月），Codex 侧走账户余额、营销站不报价，所以首页不带 offers。
 */
function softwareApplication(
  withOffer: boolean,
  locale: Locale = "zh-CN",
): Record<string, unknown> {
  const en = locale === "en";
  const base: Record<string, unknown> = {
    "@context": "https://schema.org",
    "@type": "SoftwareApplication",
    name: "BestCodex",
    applicationCategory: "DeveloperApplication",
    operatingSystem: "macOS 13+, Windows 10/11 64-bit",
    url: absoluteUrl(en ? "/en" : "/"),
    downloadUrl: absoluteUrl(en ? "/en#downloads" : "/#downloads"),
    softwareHelp: absoluteUrl(en ? "/en/guides" : "/help"),
    isBasedOn: UPSTREAM_URL,
    license: "https://www.gnu.org/licenses/agpl-3.0.html",
    inLanguage: locale,
    description: en
      ? "One launcher, two ways to work: official Codex with zero configuration, and official Claude Code on your own server."
      : "一个启动器，两种工作方式：零配置接入官方 Codex，以及把官方 Claude Code 跑在你自己的服务器上。",
    publisher: { "@type": "Organization", name: "BestCodex", url: absoluteUrl("/") },
  };

  if (withOffer) {
    const plan = en ? PLAN_EN : PLAN;
    base.offers = {
      "@type": "Offer",
      price: plan.price.replace(/[^\d.]/g, ""),
      priceCurrency: "CNY",
      url: absoluteUrl(en ? "/en/claude#pricing" : "/claude#pricing"),
      description: en
        ? `${plan.name}: usage-based, ${plan.price} is a reference top-up, not an auto-renewing monthly plan.`
        : `${plan.name}：按用量计费，${plan.price} 为参考额度，不是自动续费的包月。`,
    };
  }
  return base;
}

function breadcrumb(trail: Array<{ name: string; path: string }>): Record<string, unknown> {
  return {
    "@context": "https://schema.org",
    "@type": "BreadcrumbList",
    itemListElement: trail.map((item, index) => ({
      "@type": "ListItem",
      position: index + 1,
      name: item.name,
      item: absoluteUrl(item.path),
    })),
  };
}

function techArticle(fields: {
  headline: string;
  description: string;
  path: string;
  updated: string;
  locale: Locale;
}): Record<string, unknown> {
  return {
    "@context": "https://schema.org",
    "@type": "TechArticle",
    headline: fields.headline,
    description: fields.description,
    url: absoluteUrl(fields.path),
    inLanguage: fields.locale,
    dateModified: fields.updated,
    author: { "@type": "Organization", name: "BestCodex" },
    publisher: { "@type": "Organization", name: "BestCodex", url: absoluteUrl("/") },
  };
}

const HOME_TITLE = "BestCodex · 零配置用上官方 Codex";
const HOME_DESCRIPTION =
  "下载一次、登录一次，本机配置自动写好，启动的是官方 Codex 应用本身。窗口里另一个 Tab 把官方 Claude Code 跑在你自己的服务器上。";
const HOME_TITLE_EN = "BestCodex · official Codex with zero configuration";
const HOME_DESCRIPTION_EN =
  "Sign in once and the local config is written for you. What launches is the official Codex app. A second tab runs Claude Code on your own server.";

const CLAUDE_TITLE = "把 Claude Code 跑在自己的服务器上 · BestCodex";
const CLAUDE_DESCRIPTION =
  "官方 Claude Code 跑在你自己的服务器上：独立环境、固定 IP、持久会话，显著降低封号风险。文件双向同步，机密文件默认不同步，冲突不静默覆盖。";
const CLAUDE_TITLE_EN = "Run Claude Code on your own server · BestCodex";
const CLAUDE_DESCRIPTION_EN =
  "Official Claude Code on your server: isolated environment, stable IP, persistent sessions. Two-way sync; secrets stay out; no silent overwrite.";

function staticRoutes(): RouteSeo[] {
  const routes: Omit<RouteSeo, "locale">[] = [
    {
      path: "/",
      title: HOME_TITLE,
      description: HOME_DESCRIPTION,
      canonicalPath: "/",
      lastmod: LAST_REVIEWED,
      changefreq: "weekly",
      priority: 1,
      jsonLd: [organization(), website(), softwareApplication(false), faqPage(CODEX_FAQS)],
      llmsNote: "产品首页：BestCodex 是什么、三步开始、下载与常见问题。",
      alternatePath: "/en",
    },
    {
      // 与首页同内容，canonical 指回 `/`，避免重复内容分散权重。
      path: "/codex",
      title: HOME_TITLE,
      description: HOME_DESCRIPTION,
      canonicalPath: "/",
      lastmod: LAST_REVIEWED,
      changefreq: "weekly",
      priority: 0.5,
      jsonLd: [softwareApplication(false)],
    },
    {
      path: "/claude",
      title: CLAUDE_TITLE,
      description: CLAUDE_DESCRIPTION,
      canonicalPath: "/claude",
      lastmod: LAST_REVIEWED,
      changefreq: "weekly",
      priority: 0.9,
      jsonLd: [softwareApplication(true), faqPage(CLAUDE_FAQS)],
      llmsNote: "Claude Tab：防封思路、双向同步、定价（充值制）与常见问题。",
      alternatePath: "/en/claude",
    },
    {
      path: "/en",
      title: HOME_TITLE_EN,
      description: HOME_DESCRIPTION_EN,
      canonicalPath: "/en",
      lastmod: LAST_REVIEWED,
      changefreq: "weekly",
      priority: 1,
      jsonLd: [softwareApplication(false, "en"), faqPage(CODEX_FAQS_EN)],
      llmsNote: "English home: what BestCodex is, three steps, download, FAQ.",
      alternatePath: "/",
    },
    {
      path: "/en/claude",
      title: CLAUDE_TITLE_EN,
      description: CLAUDE_DESCRIPTION_EN,
      canonicalPath: "/en/claude",
      lastmod: LAST_REVIEWED,
      changefreq: "weekly",
      priority: 0.9,
      jsonLd: [softwareApplication(true, "en"), faqPage(CLAUDE_FAQS_EN)],
      llmsNote: "English Claude tab: ban-risk approach, sync, usage-based pricing, FAQ.",
      alternatePath: "/claude",
    },
    {
      path: "/help",
      title: "帮助中心 · BestCodex",
      description: "安装、未签名提示、登录、修复本机配置、Claude 连服务器——五个主题的简明说明。",
      canonicalPath: "/help",
      lastmod: LAST_REVIEWED,
      changefreq: "monthly",
      priority: 0.7,
      jsonLd: [breadcrumb([{ name: "帮助", path: "/help" }])],
      llmsNote: "帮助中心索引。",
    },
    {
      path: "/guides",
      title: "指南 · BestCodex",
      description: "封号风险怎么降、Claude Code 怎么跑在自有服务器、Codex 零配置、macOS 已损坏的解法。",
      canonicalPath: "/guides",
      lastmod: LAST_REVIEWED,
      changefreq: "weekly",
      priority: 0.8,
      jsonLd: [breadcrumb([{ name: "指南", path: "/guides" }])],
      llmsNote: "指南索引：比帮助中心更长的回答型内容。",
      alternatePath: "/en/guides",
    },
    {
      path: "/en/guides",
      title: "Guides · BestCodex",
      description:
        "Lowering Claude Code ban risk, running it on your own server, zero-config Codex, and the macOS “damaged app” fix.",
      canonicalPath: "/en/guides",
      lastmod: LAST_REVIEWED,
      changefreq: "weekly",
      priority: 0.8,
      jsonLd: [breadcrumb([{ name: "Guides", path: "/en/guides" }])],
      llmsNote: "English guide index.",
      alternatePath: "/guides",
    },
    {
      path: "/privacy",
      title: "隐私政策 · BestCodex",
      description: "BestCodex 会改写官方 Codex 用户目录配置并把请求指到中转接口；不捆绑、不修改官方应用。",
      canonicalPath: "/privacy",
      lastmod: LAST_REVIEWED,
      changefreq: "monthly",
      priority: 0.4,
      jsonLd: [breadcrumb([{ name: "隐私政策", path: "/privacy" }])],
      llmsNote: "隐私政策：本机改写哪些文件、账号存在哪、不捆绑官方应用。",
      alternatePath: "/en/privacy",
    },
    {
      path: "/terms",
      title: "服务条款 · BestCodex",
      description: "使用 BestCodex 即同意它改写官方 Codex 本机配置并经中转接口转发请求。不捆绑官方应用。",
      canonicalPath: "/terms",
      lastmod: LAST_REVIEWED,
      changefreq: "monthly",
      priority: 0.4,
      jsonLd: [breadcrumb([{ name: "服务条款", path: "/terms" }])],
      llmsNote: "服务条款：本机配置知情同意、退出不恢复官方配置、开源免责。",
      alternatePath: "/en/terms",
    },
    {
      path: "/en/privacy",
      title: "Privacy Policy · BestCodex",
      description:
        "BestCodex rewrites official Codex user-directory config and relays requests. It does not bundle or modify the official app.",
      canonicalPath: "/en/privacy",
      lastmod: LAST_REVIEWED,
      changefreq: "monthly",
      priority: 0.4,
      jsonLd: [breadcrumb([{ name: "Privacy Policy", path: "/en/privacy" }])],
      llmsNote: "Privacy policy: which local files are rewritten, where the account lives, no official-app bundling.",
      alternatePath: "/privacy",
    },
    {
      path: "/en/terms",
      title: "Terms of Service · BestCodex",
      description:
        "Using BestCodex means you agree it rewrites official Codex local config and relays requests. It does not bundle the official app.",
      canonicalPath: "/en/terms",
      lastmod: LAST_REVIEWED,
      changefreq: "monthly",
      priority: 0.4,
      jsonLd: [breadcrumb([{ name: "Terms of Service", path: "/en/terms" }])],
      llmsNote: "Terms: informed consent for local config, sign-out does not restore official config.",
      alternatePath: "/terms",
    },
  ];
  return routes.map((route) => ({
    ...route,
    locale: route.path.startsWith("/en") ? "en" : "zh-CN",
  }));
}

function helpRoutes(): RouteSeo[] {
  return HELP_TOPICS.map((topic) => ({
    path: `/help/${topic.slug}`,
    title: `${topic.title} · BestCodex 帮助`,
    description: topic.summary,
    canonicalPath: `/help/${topic.slug}`,
    lastmod: LAST_REVIEWED,
    changefreq: "monthly" as const,
    priority: 0.6,
    locale: "zh-CN" as const,
    jsonLd: [
      techArticle({
        headline: topic.title,
        description: topic.summary,
        path: `/help/${topic.slug}`,
        updated: LAST_REVIEWED,
        locale: "zh-CN",
      }),
      breadcrumb([
        { name: "帮助", path: "/help" },
        { name: topic.title, path: `/help/${topic.slug}` },
      ]),
    ],
    llmsNote: topic.summary,
  }));
}

/**
 * 指南路由，中英同构。slug 两种语言一致，所以 `alternatePath` 直接互指，
 * 满足 hreflang 必须双向的要求。
 */
function guideRoutes(locale: Locale): RouteSeo[] {
  const en = locale === "en";
  const guides = en ? GUIDES_EN : GUIDES;
  const base = en ? "/en/guides" : "/guides";
  const otherBase = en ? "/guides" : "/en/guides";
  const crumb = en ? "Guides" : "指南";

  return guides.map((guide) => ({
    path: `${base}/${guide.slug}`,
    // 标题里已经带品牌就不再加后缀，避免「BestCodex 与 Codex++ 的区别 · BestCodex」这种重复。
    // 关键词在前、品牌在后：SERP 截断时丢掉的是品牌而不是查询词。
    title: guide.title.includes("BestCodex") ? guide.title : `${guide.title} · BestCodex`,
    description: guide.summary,
    canonicalPath: `${base}/${guide.slug}`,
    lastmod: guide.updated,
    changefreq: "monthly" as const,
    priority: 0.7,
    locale,
    alternatePath: `${otherBase}/${guide.slug}`,
    jsonLd: [
      techArticle({
        headline: guide.title,
        description: guide.summary,
        path: `${base}/${guide.slug}`,
        updated: guide.updated,
        locale,
      }),
      // 指南本身就是在回答一个具体问题，用 FAQPage 把「问题 → 自包含答案」显式喂给引擎。
      faqPage([[guide.question, guide.answer]]),
      breadcrumb([
        { name: crumb, path: base },
        { name: guide.title, path: `${base}/${guide.slug}` },
      ]),
    ],
    llmsNote: guide.question,
  }));
}

/** 需要预渲染并进 sitemap 的全部路由。 */
export const SEO_ROUTES: RouteSeo[] = [
  ...staticRoutes(),
  ...helpRoutes(),
  ...guideRoutes("zh-CN"),
  ...guideRoutes("en"),
];

export function seoForPath(pathname: string): RouteSeo | undefined {
  const normalized = pathname !== "/" && pathname.endsWith("/") ? pathname.slice(0, -1) : pathname;
  return SEO_ROUTES.find((route) => route.path === normalized);
}

/** 客户端换页与预渲染共用的标题来源。 */
export function pageTitle(pathname: string): string {
  const route = seoForPath(pathname);
  if (route) return route.title;
  if (pathname === "/download") return HOME_TITLE;
  if (pathname === "/pricing") return CLAUDE_TITLE;
  if (pathname.startsWith("/en/guides/")) return "No such guide · BestCodex";
  if (pathname.startsWith("/help/")) return "没有这篇说明 · BestCodex";
  if (pathname.startsWith("/guides/")) return "没有这篇指南 · BestCodex";
  return "页面不存在 · BestCodex";
}
