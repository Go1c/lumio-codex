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
import { GUIDES } from "@/guides";

/** GitHub 仓库：结构化数据用它做实体绑定，和同名无关站点区分开。 */
export const REPO_URL = "https://github.com/Go1c/lumio-codex";
const UPSTREAM_URL = "https://github.com/BigPizzaV3/CodexPlusPlus";
const LAST_REVIEWED = "2026-08-17";

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
function softwareApplication(withOffer: boolean): Record<string, unknown> {
  const base: Record<string, unknown> = {
    "@context": "https://schema.org",
    "@type": "SoftwareApplication",
    name: "BestCodex",
    applicationCategory: "DeveloperApplication",
    operatingSystem: "macOS 13+, Windows 10/11 64-bit",
    url: absoluteUrl("/"),
    downloadUrl: absoluteUrl("/#downloads"),
    softwareHelp: absoluteUrl("/help"),
    isBasedOn: UPSTREAM_URL,
    license: "https://www.gnu.org/licenses/agpl-3.0.html",
    description:
      "一个启动器，两种工作方式：零配置接入官方 Codex，以及把官方 Claude Code 跑在你自己的服务器上。",
    publisher: { "@type": "Organization", name: "BestCodex", url: absoluteUrl("/") },
  };

  if (withOffer) {
    base.offers = {
      "@type": "Offer",
      price: PLAN.price.replace(/[^\d.]/g, ""),
      priceCurrency: "CNY",
      url: absoluteUrl("/claude#pricing"),
      description: `${PLAN.name}：按用量计费，${PLAN.price} 为参考额度，不是自动续费的包月。`,
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
}): Record<string, unknown> {
  return {
    "@context": "https://schema.org",
    "@type": "TechArticle",
    headline: fields.headline,
    description: fields.description,
    url: absoluteUrl(fields.path),
    inLanguage: "zh-CN",
    dateModified: fields.updated,
    author: { "@type": "Organization", name: "BestCodex" },
    publisher: { "@type": "Organization", name: "BestCodex", url: absoluteUrl("/") },
  };
}

const HOME_TITLE = "BestCodex · 零配置用上官方 Codex";
const HOME_DESCRIPTION =
  "下载一次、登录一次，本机配置自动写好，启动的是官方 Codex 应用本身。窗口里另一个 Tab 把官方 Claude Code 跑在你自己的服务器上。";

const CLAUDE_TITLE = "把 Claude Code 跑在自己的服务器上 · BestCodex";
const CLAUDE_DESCRIPTION =
  "官方 Claude Code 跑在你自己的服务器上：独立环境、固定 IP、持久会话，显著降低封号风险。文件双向同步，机密文件默认不同步，冲突不静默覆盖。";

function staticRoutes(): RouteSeo[] {
  return [
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
    },
  ];
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
    jsonLd: [
      techArticle({
        headline: topic.title,
        description: topic.summary,
        path: `/help/${topic.slug}`,
        updated: LAST_REVIEWED,
      }),
      breadcrumb([
        { name: "帮助", path: "/help" },
        { name: topic.title, path: `/help/${topic.slug}` },
      ]),
    ],
    llmsNote: topic.summary,
  }));
}

function guideRoutes(): RouteSeo[] {
  return GUIDES.map((guide) => ({
    path: `/guides/${guide.slug}`,
    // 标题里已经带品牌就不再加后缀，避免「BestCodex 与 Codex++ 的区别 · BestCodex」这种重复。
    // 关键词在前、品牌在后：SERP 截断时丢掉的是品牌而不是查询词。
    title: guide.title.includes("BestCodex") ? guide.title : `${guide.title} · BestCodex`,
    description: guide.summary,
    canonicalPath: `/guides/${guide.slug}`,
    lastmod: guide.updated,
    changefreq: "monthly" as const,
    priority: 0.7,
    jsonLd: [
      techArticle({
        headline: guide.title,
        description: guide.summary,
        path: `/guides/${guide.slug}`,
        updated: guide.updated,
      }),
      // 指南本身就是在回答一个具体问题，用 FAQPage 把「问题 → 自包含答案」显式喂给引擎。
      faqPage([[guide.question, guide.answer]]),
      breadcrumb([
        { name: "指南", path: "/guides" },
        { name: guide.title, path: `/guides/${guide.slug}` },
      ]),
    ],
    llmsNote: guide.question,
  }));
}

/** 需要预渲染并进 sitemap 的全部路由。 */
export const SEO_ROUTES: RouteSeo[] = [...staticRoutes(), ...helpRoutes(), ...guideRoutes()];

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
  if (pathname.startsWith("/help/")) return "没有这篇说明 · BestCodex";
  if (pathname.startsWith("/guides/")) return "没有这篇指南 · BestCodex";
  return "页面不存在 · BestCodex";
}
