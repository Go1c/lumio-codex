// @vitest-environment node
//
// 这个文件必须跑在 node 环境：没有 window / document / navigator，才等于构建期预渲染
// 的真实条件。放在 jsdom 里跑会走客户端分支，测不出任何东西。

import { describe, expect, it } from "vitest";

import { GUIDES } from "@/guides";
import { SEO_ROUTES, headDataFor, markdownPages, renderRoute } from "@/prerender";

describe("预渲染产出爬虫可读的正文", () => {
  it("没有 window 也能渲染，不抛异常", () => {
    expect(typeof window).toBe("undefined");
    for (const route of SEO_ROUTES) {
      expect(() => renderRoute(route.path), `${route.path} 渲染失败`).not.toThrow();
    }
  });

  it("首页正文进 HTML，不是空壳", () => {
    const html = renderRoute("/");
    expect(html).toContain("更快开始使用");
    expect(html).toContain("三步开始");
    expect(html).toContain("官方 Codex");
  });

  it("折叠的 FAQ 答案也在 HTML 里", () => {
    const html = renderRoute("/");
    // 手风琴默认只展开第一项，但所有答案都必须在 DOM 中，否则爬虫只看得到问题。
    expect(html).toContain("xattr -cr");
    expect(html).toContain("hidden");
  });

  it("不把 opacity:0 烙进静态 HTML", () => {
    // Reveal 的动画初始态是 opacity:0；若进了静态 HTML，爬虫读到的是一段隐藏正文。
    for (const path of ["/", "/claude", "/guides/claude-code-ban"]) {
      expect(renderRoute(path), `${path} 残留隐藏样式`).not.toMatch(/opacity:\s*0/);
    }
  });

  it("下载区不输出加载中文案，也不瞎猜平台", () => {
    const html = renderRoute("/");
    expect(html).not.toContain("读取最新版本");
    expect(html).toContain("内测版 · 未签名");
  });

  it("Claude 页带出定价与防封说明", () => {
    const html = renderRoute("/claude");
    expect(html).toContain("19.9");
    expect(html).toMatch(/封号|独立环境/);
  });

  it("每篇指南的答案前置段落都进 HTML", () => {
    for (const guide of GUIDES) {
      const html = renderRoute(`/guides/${guide.slug}`);
      // answer 里的 **粗体** / `代码` / [链接] 会渲染成标签，把文字切断，所以只比对
      // 第一个行内标记之前那段连续纯文字。
      const plain = guide.answer.split(/\*\*|`|\[/)[0].slice(0, 24);
      expect(plain.length, `${guide.slug} 开头可比对的纯文字太短`).toBeGreaterThanOrEqual(10);
      expect(html, `${guide.slug} 正文没进 HTML`).toContain(plain);
      expect(html).toContain(guide.sections[0].heading);
    }
  });

  it("未命中路由渲染 404 页而不是空白", () => {
    const html = renderRoute("/definitely-not-a-page");
    expect(html.length).toBeGreaterThan(200);
  });
});

describe("head 元数据", () => {
  it("每条路由都取到 head 数据，canonical 是绝对地址", () => {
    for (const route of SEO_ROUTES) {
      const head = headDataFor(route.path);
      expect(head, `${route.path} 没有 head 数据`).toBeDefined();
      expect(head!.canonical).toMatch(/^https?:\/\//);
    }
  });

  it("/codex 的 canonical 指向首页", () => {
    expect(headDataFor("/codex")!.canonical).toBe(headDataFor("/")!.canonical);
  });
});

describe("Markdown 镜像", () => {
  it("覆盖全部指南与帮助主题", () => {
    const pages = markdownPages();
    for (const guide of GUIDES) {
      expect(pages.some((page) => page.path === `/guides/${guide.slug}`)).toBe(true);
    }
    expect(pages.length).toBeGreaterThanOrEqual(GUIDES.length);
  });

  it("每份都是合法 Markdown 且带回源链接", () => {
    for (const page of markdownPages()) {
      expect(page.markdown.startsWith("# "), `${page.path} 缺 h1`).toBe(true);
      expect(page.markdown).toContain("来源：https://");
    }
  });
});
