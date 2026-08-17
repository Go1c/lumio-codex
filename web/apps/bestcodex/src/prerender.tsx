/**
 * 构建期预渲染入口。
 *
 * 由 `vite build --ssr` 单独打成 Node 可执行的包，再由 `scripts/prerender.mjs` 调用。
 * 这里**不能** import `main.tsx`（含 BrowserRouter 与 createRoot），也不 import CSS——
 * 样式由客户端构建产物里的 `<link>` 负责。
 */

import { renderToString } from "react-dom/server";
import { StaticRouter } from "react-router-dom/server";

import { HELP_TOPICS } from "@lumio/ui";

import { App } from "./App";
import { GUIDES } from "./guides";
import { SEO_ROUTES, absoluteUrl, seoForPath, siteOrigin } from "./seo";

export { SEO_ROUTES, absoluteUrl, siteOrigin };

export function renderRoute(path: string): string {
  return renderToString(
    <StaticRouter location={path}>
      <App />
    </StaticRouter>,
  );
}

export interface MarkdownPage {
  /** 站内路径，`.md` 产物按此落盘。 */
  path: string;
  title: string;
  markdown: string;
}

/**
 * 纯文本（Markdown）镜像。
 *
 * 编程 Agent 与部分抓取器更愿意吃 Markdown 而不是 HTML；这些文件同时被 llms.txt 引用。
 * 内容由源数据生成，不从 HTML 反解，避免正文与镜像漂移。
 */
export function markdownPages(): MarkdownPage[] {
  const pages: MarkdownPage[] = [];

  for (const guide of GUIDES) {
    const body = [
      `# ${guide.title}`,
      "",
      `> ${guide.question}`,
      "",
      guide.answer,
      "",
      ...guide.sections.flatMap((section) => [`## ${section.heading}`, "", ...section.body, ""]),
      `---`,
      "",
      `来源：${absoluteUrl(`/guides/${guide.slug}`)} · 最后更新 ${guide.updated}`,
      "",
    ].join("\n");
    pages.push({ path: `/guides/${guide.slug}`, title: guide.title, markdown: body });
  }

  for (const topic of HELP_TOPICS) {
    const body = [
      `# ${topic.title}`,
      "",
      topic.summary,
      "",
      ...topic.body.flatMap((paragraph) => [paragraph, ""]),
      `---`,
      "",
      `来源：${absoluteUrl(`/help/${topic.slug}`)}`,
      "",
    ].join("\n");
    pages.push({ path: `/help/${topic.slug}`, title: topic.title, markdown: body });
  }

  return pages;
}

/** `<head>` 需要的元数据；供构建脚本拼接，不在这里生成 HTML 字符串。 */
export function headDataFor(path: string) {
  const route = seoForPath(path);
  if (!route) return undefined;
  return {
    title: route.title,
    description: route.description,
    canonical: absoluteUrl(route.canonicalPath),
    jsonLd: route.jsonLd,
  };
}
