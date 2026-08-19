/**
 * 把客户端构建产物变成「每条路由一份真静态 HTML」。
 *
 * 前置：先跑 `vite build`（产出 dist/ 与 index.html 模板），再跑
 * `vite build --ssr src/prerender.tsx --outDir dist-ssr`（产出可在 Node 里跑的渲染器）。
 *
 * 本脚本负责：
 * - 逐路由 renderToString，把正文写进 #root，让不执行 JS 的爬虫也能读到内容；
 * - 按路由注入 title / description / canonical / OG / Twitter / JSON-LD；
 * - 生成 sitemap.xml、llms.txt、每页 .md 镜像与 404.html。
 *
 * 客户端仍用 createRoot（不是 hydrateRoot）：React 会丢弃这份 HTML 重新渲染，
 * 代价是一次极小的重绘，换来彻底规避会话状态 / 设备探测 / 动效三类 hydration 不一致。
 */

import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const appRoot = join(here, "..");
const distDir = join(appRoot, "dist");
const ssrEntry = join(appRoot, "dist-ssr", "prerender.js");

const {
  BING_SITE_VERIFICATION,
  SEO_ROUTES,
  renderRoute,
  headDataFor,
  markdownPages,
  absoluteUrl,
  siteOrigin,
} = await import(ssrEntry);

const ORIGIN = siteOrigin();
const OG_IMAGE = absoluteUrl("/bestcodex-icon.jpg");

/**
 * 各搜索引擎的站点归属验证。固定令牌来自 SEO 真值；其他引擎仍从环境变量注入。
 * 只写进首页——各家后台都只校验首页。没配的引擎自动跳过，不会产出空 meta。
 */
const VERIFICATIONS = [
  ["google-site-verification", process.env.SITE_VERIFY_GOOGLE],
  ["msvalidate.01", BING_SITE_VERIFICATION],
  ["baidu-site-verification", process.env.SITE_VERIFY_BAIDU],
  ["sogou_site_verification", process.env.SITE_VERIFY_SOGOU],
  ["360-site-verification", process.env.SITE_VERIFY_360],
  ["yandex-verification", process.env.SITE_VERIFY_YANDEX],
].filter(([, token]) => Boolean(token));

function escapeHtml(value) {
  return String(value)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

/** JSON-LD 内联进 <script> 时必须把 < 转义，否则正文里的 </script> 会提前闭合标签。 */
function jsonLdScript(node) {
  const json = JSON.stringify(node).replace(/</g, "\\u003c");
  return `    <script type="application/ld+json">${json}</script>`;
}

function headFor(head, { isHome = false, noindex = false } = {}) {
  const title = escapeHtml(head.title);
  const description = escapeHtml(head.description);
  const canonical = escapeHtml(head.canonical);

  return [
    `    <title>${title}</title>`,
    // 摘要与预览尺寸默认由引擎自行裁剪，显式放开：AI 摘要与 SERP 富摘要都靠它取到完整段落。
    // 404 反过来必须 noindex，否则会以「页面不存在」的标题进索引。
    noindex
      ? `    <meta name="robots" content="noindex, follow" />`
      : `    <meta name="robots" content="index, follow, max-snippet:-1, max-image-preview:large, max-video-preview:-1" />`,
    ...(isHome
      ? VERIFICATIONS.map(
          ([name, token]) =>
            `    <meta name="${name}" content="${escapeHtml(token)}" />`,
        )
      : []),
    `    <meta name="description" content="${description}" />`,
    `    <link rel="canonical" href="${canonical}" />`,
    ...(head.alternates ?? []).map(
      (alt) =>
        `    <link rel="alternate" hreflang="${alt.hreflang}" href="${escapeHtml(alt.href)}" />`,
    ),
    `    <meta property="og:type" content="website" />`,
    `    <meta property="og:site_name" content="BestCodex" />`,
    `    <meta property="og:locale" content="${head.locale === "en" ? "en_US" : "zh_CN"}" />`,
    `    <meta property="og:title" content="${title}" />`,
    `    <meta property="og:description" content="${description}" />`,
    `    <meta property="og:url" content="${canonical}" />`,
    `    <meta property="og:image" content="${escapeHtml(OG_IMAGE)}" />`,
    `    <meta name="twitter:card" content="summary_large_image" />`,
    `    <meta name="twitter:title" content="${title}" />`,
    `    <meta name="twitter:description" content="${description}" />`,
    `    <meta name="twitter:image" content="${escapeHtml(OG_IMAGE)}" />`,
    ...head.jsonLd.map(jsonLdScript),
  ].join("\n");
}

/** 模板里已有的 title / description 必须先摘掉，否则每页会出现两份。 */
function stripTemplateHead(template) {
  return template
    .replace(/[ \t]*<title>[\s\S]*?<\/title>\n?/i, "")
    .replace(/[ \t]*<meta\s+name="description"[\s\S]*?\/>\n?/i, "");
}

function buildPage(template, head, appHtml, options) {
  const withoutDefaults = stripTemplateHead(template);
  // `<html lang>` 必须跟着页面语言走：搜索引擎与朗读器都以它为准，模板里写死的是 zh-CN。
  const withLang = withoutDefaults.replace(
    /<html([^>]*)\slang="[^"]*"/i,
    `<html$1 lang="${head.locale === "en" ? "en" : "zh-CN"}"`,
  );
  const withHead = withLang.replace("</head>", `${headFor(head, options)}\n  </head>`);
  const marker = '<div id="root"></div>';
  if (!withHead.includes(marker)) {
    throw new Error(`dist/index.html 里找不到 ${marker}，预渲染无法注入正文`);
  }
  return withHead.replace(marker, `<div id="root">${appHtml}</div>`);
}

async function writeFileEnsured(relativePath, contents) {
  const target = join(distDir, relativePath);
  await mkdir(dirname(target), { recursive: true });
  await writeFile(target, contents, "utf8");
}

function outputPathFor(routePath) {
  return routePath === "/" ? "index.html" : `${routePath.replace(/^\//, "")}/index.html`;
}

function sitemapXml(routes) {
  const entries = routes
    // canonical 不指向自己的路由（例如 /codex → /）不进 sitemap，避免自相矛盾的信号。
    .filter((route) => route.canonicalPath === route.path)
    .map((route) => {
      // sitemap 里的 xhtml:link 是 Google 认可的第二种 hreflang 写法。<head> 里已经有一份，
      // 两处都给能提高被正确解析的概率，代价只是 sitemap 变长。
      const head = headDataFor(route.path);
      const alternates = (head?.alternates ?? []).map(
        (alt) =>
          `    <xhtml:link rel="alternate" hreflang="${alt.hreflang}" href="${alt.href}" />`,
      );
      return [
        "  <url>",
        `    <loc>${absoluteUrl(route.path)}</loc>`,
        ...alternates,
        `    <lastmod>${route.lastmod}</lastmod>`,
        `    <changefreq>${route.changefreq}</changefreq>`,
        `    <priority>${route.priority.toFixed(1)}</priority>`,
        "  </url>",
      ].join("\n");
    })
    .join("\n");

  return `<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9" xmlns:xhtml="http://www.w3.org/1999/xhtml">\n${entries}\n</urlset>\n`;
}

function llmsTxt(routes, markdown) {
  const mdByPath = new Map(markdown.map((page) => [page.path, page]));

  // 中英分节，各用本语言的标点与措辞。混排会让引擎判不准每条条目的语言。
  const sections = [
    {
      heading: "## 页面（中文）",
      locale: "zh-CN",
      entry: (title, url, note, md) =>
        `- [${title}](${url})：${note}${md ? ` （纯文本：${md}）` : ""}`,
    },
    {
      heading: "## Pages (English)",
      locale: "en",
      entry: (title, url, note, md) =>
        `- [${title}](${url}): ${note}${md ? ` (plain text: ${md})` : ""}`,
    },
  ];

  const lines = [
    "# BestCodex",
    "",
    "> 一个启动器，两种工作方式：零配置接入官方 Codex，以及把官方 Claude Code 跑在你自己的服务器上（独立环境、固定 IP、持久会话、双向同步）。",
    "",
    "> One launcher, two ways to work: use the official Codex with zero configuration, and run the official Claude Code on your own server (isolated environment, stable IP, persistent sessions, two-way sync).",
    "",
    "BestCodex 是独立项目，与 OpenAI、Anthropic 无从属、赞助或认可关系。桌面端是",
    "https://github.com/BigPizzaV3/CodexPlusPlus 的 AGPL-3.0 fork。",
    "网上存在同名但无关的第三方服务，本项目只有 bestcodex.app 一个站点。",
    "",
    "BestCodex is an independent project, not affiliated with, sponsored by, or endorsed by OpenAI",
    "or Anthropic. The desktop app is an AGPL-3.0 fork of the repository above. Unrelated services",
    "share this name; bestcodex.app is the only site belonging to this project.",
    "",
    // 安装命令放在页面清单之前：Agent 被问「怎么装」时，最有用的回答是一条可直接执行的命令。
    "## 安装 / Install",
    "",
    "```",
    "# macOS (Apple silicon and Intel)",
    "curl -fsSL https://bestcodex.app/install.sh | sh",
    "",
    "# Windows (PowerShell)",
    "irm https://bestcodex.app/install.ps1 | iex",
    "```",
    "",
    "脚本按芯片挑安装包、校验 SHA256、装进「应用程序」，并清掉未签名包的隔离标记。",
    "The script picks the build for your chip, verifies SHA256, installs into /Applications, and",
    "clears the quarantine attribute that unsigned builds carry. No sudo. It writes no configuration.",
    "",
    "macOS 13+ / Windows 10-11 x64. 官方 Codex 应用需单独安装 / the official Codex app is not bundled.",
    "",
  ];

  for (const section of sections) {
    lines.push(section.heading, "");
    for (const route of routes) {
      if (route.canonicalPath !== route.path) continue;
      if (route.locale !== section.locale) continue;
      const md = mdByPath.get(route.path) ? absoluteUrl(`${route.path}.md`) : undefined;
      lines.push(
        section.entry(
          route.title,
          absoluteUrl(route.path),
          route.llmsNote ?? route.description,
          md,
        ),
      );
    }
    lines.push("");
  }

  lines.push("## 其他 / Other", "", `- [源码仓库 / Source](https://github.com/LumioGames/lumio-codex)`, "");
  return lines.join("\n");
}

async function main() {
  const template = await readFile(join(distDir, "index.html"), "utf8");

  let count = 0;
  for (const route of SEO_ROUTES) {
    const head = headDataFor(route.path);
    if (!head) throw new Error(`路由 ${route.path} 缺少 SEO 元数据`);
    const html = buildPage(template, head, renderRoute(route.path), {
      isHome: route.path === "/",
    });
    await writeFileEnsured(outputPathFor(route.path), html);
    count += 1;
  }

  // 未命中路由的真 404：静态托管把它当 404 文档返回，避免全站软 404。
  const notFoundHead = {
    title: "页面不存在 · BestCodex",
    description: "链接可能已过期，或地址输错了。",
    canonical: absoluteUrl("/"),
    jsonLd: [],
  };
  await writeFileEnsured(
    "404.html",
    buildPage(template, notFoundHead, renderRoute("/__not_found__"), { noindex: true }),
  );

  const markdown = markdownPages();
  for (const page of markdown) {
    await writeFileEnsured(`${page.path.replace(/^\//, "")}.md`, page.markdown);
  }

  await writeFileEnsured("sitemap.xml", sitemapXml(SEO_ROUTES));
  await writeFileEnsured("llms.txt", llmsTxt(SEO_ROUTES, markdown));

  // IndexNow 要求站点根目录托管一个以密钥命名、内容就是密钥本身的文本文件，用来证明
  // 提交方确实控制这个域。密钥不入库，从环境变量来。
  const indexNowKey = process.env.INDEXNOW_KEY;
  if (indexNowKey) {
    await writeFileEnsured(`${indexNowKey}.txt`, indexNowKey);
  }

  console.log(
    `prerender: ${count} 页 + 404 + ${markdown.length} 份 .md + sitemap.xml + llms.txt → ${ORIGIN}`,
  );
  console.log(
    `  站点验证: ${VERIFICATIONS.length ? VERIFICATIONS.map(([name]) => name).join(", ") : "未配置（见仓库根 docs/seo-operations.md）"}`,
  );
  console.log(`  IndexNow 密钥文件: ${indexNowKey ? `${indexNowKey}.txt` : "未配置"}`);
}

await main();
