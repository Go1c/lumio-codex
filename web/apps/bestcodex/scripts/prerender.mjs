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

const { SEO_ROUTES, renderRoute, headDataFor, markdownPages, absoluteUrl, siteOrigin } =
  await import(ssrEntry);

const ORIGIN = siteOrigin();
const OG_IMAGE = absoluteUrl("/bestcodex-icon.jpg");

/**
 * 各搜索引擎的站点归属验证。令牌从环境变量注入，只写进首页——各家后台都只校验首页。
 * 没配的引擎自动跳过，不会产出空 meta（空 content 会被判成无效验证）。
 */
const VERIFICATIONS = [
  ["google-site-verification", process.env.SITE_VERIFY_GOOGLE],
  ["msvalidate.01", process.env.SITE_VERIFY_BING],
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

function headFor(head, { isHome = false } = {}) {
  const title = escapeHtml(head.title);
  const description = escapeHtml(head.description);
  const canonical = escapeHtml(head.canonical);

  return [
    `    <title>${title}</title>`,
    ...(isHome
      ? VERIFICATIONS.map(
          ([name, token]) =>
            `    <meta name="${name}" content="${escapeHtml(token)}" />`,
        )
      : []),
    `    <meta name="description" content="${description}" />`,
    `    <link rel="canonical" href="${canonical}" />`,
    `    <meta property="og:type" content="website" />`,
    `    <meta property="og:site_name" content="BestCodex" />`,
    `    <meta property="og:locale" content="zh_CN" />`,
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
  const withHead = withoutDefaults.replace("</head>", `${headFor(head, options)}\n  </head>`);
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
    .map((route) =>
      [
        "  <url>",
        `    <loc>${absoluteUrl(route.path)}</loc>`,
        `    <lastmod>${route.lastmod}</lastmod>`,
        `    <changefreq>${route.changefreq}</changefreq>`,
        `    <priority>${route.priority.toFixed(1)}</priority>`,
        "  </url>",
      ].join("\n"),
    )
    .join("\n");

  return `<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n${entries}\n</urlset>\n`;
}

function llmsTxt(routes, markdown) {
  const mdByPath = new Map(markdown.map((page) => [page.path, page]));
  const lines = [
    "# BestCodex",
    "",
    "> 一个启动器，两种工作方式：零配置接入官方 Codex，以及把官方 Claude Code 跑在你自己的服务器上（独立环境、固定 IP、持久会话、双向同步）。",
    "",
    "BestCodex 是独立项目，与 OpenAI、Anthropic 无从属、赞助或认可关系。桌面端是",
    "https://github.com/BigPizzaV3/CodexPlusPlus 的 AGPL-3.0 fork。",
    "网上存在同名但无关的第三方服务，本项目只有 bestcodex.app 一个站点。",
    "",
    "## 页面",
    "",
  ];

  for (const route of routes) {
    if (route.canonicalPath !== route.path) continue;
    const note = route.llmsNote ?? route.description;
    const md = mdByPath.get(route.path);
    const suffix = md ? ` （纯文本：${absoluteUrl(`${route.path}.md`)}）` : "";
    lines.push(`- [${route.title}](${absoluteUrl(route.path)})：${note}${suffix}`);
  }

  lines.push("", "## 其他", "", `- [源码仓库](https://github.com/Go1c/lumio-codex)`, "");
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
    buildPage(template, notFoundHead, renderRoute("/__not_found__")),
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
