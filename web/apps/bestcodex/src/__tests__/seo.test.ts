import { describe, expect, it } from "vitest";

import { GUIDES } from "@/guides";
import { REPO_URL, SEO_ROUTES, pageTitle, seoForPath } from "@/seo";

/** canonical 指向自己的路由才是「可索引正本」，重复内容页（/codex）不参与唯一性约束。 */
const canonicalRoutes = SEO_ROUTES.filter((route) => route.canonicalPath === route.path);

describe("SEO 路由元数据", () => {
  it("每条路由都有标题与描述，且描述长度可控", () => {
    for (const route of SEO_ROUTES) {
      expect(route.title.length, `${route.path} 缺标题`).toBeGreaterThan(0);
      expect(route.description.length, `${route.path} 缺描述`).toBeGreaterThan(0);
      // 过长的 description 会被搜索结果截断，等于白写。
      expect(route.description.length, `${route.path} 描述过长`).toBeLessThanOrEqual(130);
    }
  });

  it("可索引正本之间标题互不相同，避免自己和自己抢关键词", () => {
    const titles = canonicalRoutes.map((route) => route.title);
    expect(new Set(titles).size).toBe(titles.length);
  });

  it("路径不重复", () => {
    const paths = SEO_ROUTES.map((route) => route.path);
    expect(new Set(paths).size).toBe(paths.length);
  });

  it("/codex 与首页同内容，canonical 指回 / 消除重复", () => {
    const codex = seoForPath("/codex");
    expect(codex?.canonicalPath).toBe("/");
    expect(codex?.title).toBe(seoForPath("/")?.title);
  });

  it("结构化数据都可序列化且带 @context", () => {
    for (const route of SEO_ROUTES) {
      expect(route.jsonLd.length, `${route.path} 没有结构化数据`).toBeGreaterThan(0);
      for (const node of route.jsonLd) {
        expect(node["@context"]).toBe("https://schema.org");
        expect(() => JSON.stringify(node)).not.toThrow();
      }
    }
  });

  it("首页给出 Organization / WebSite / SoftwareApplication / FAQPage 四类实体", () => {
    const types = seoForPath("/")!.jsonLd.map((node) => node["@type"]);
    expect(types).toEqual(
      expect.arrayContaining(["Organization", "WebSite", "SoftwareApplication", "FAQPage"]),
    );
  });

  it("Organization 绑定仓库并显式消歧，与同名无关站点区分", () => {
    const org = seoForPath("/")!.jsonLd.find((node) => node["@type"] === "Organization")!;
    expect(org.sameAs).toContain(REPO_URL);
    expect(String(org.disambiguatingDescription)).toMatch(/无关/);
  });

  it("只有 Claude 页报价，首页不带 offers", () => {
    const home = seoForPath("/")!.jsonLd.find((node) => node["@type"] === "SoftwareApplication")!;
    const claude = seoForPath("/claude")!.jsonLd.find(
      (node) => node["@type"] === "SoftwareApplication",
    )!;
    expect(home.offers).toBeUndefined();
    expect(claude.offers).toBeDefined();
    // 充值制不是自动续费包月，结构化数据的措辞必须与页面一致。
    expect(String((claude.offers as Record<string, unknown>).description)).toMatch(/不是自动续费/);
  });

  it("sitemap 的 lastmod 是合法 ISO 日期", () => {
    for (const route of SEO_ROUTES) {
      expect(route.lastmod, `${route.path} 日期格式不对`).toMatch(/^\d{4}-\d{2}-\d{2}$/);
      expect(Number.isNaN(Date.parse(route.lastmod))).toBe(false);
    }
  });
});

describe("页面标题", () => {
  it("每条路由取到自己的标题", () => {
    for (const route of SEO_ROUTES) {
      expect(pageTitle(route.path)).toBe(route.title);
    }
  });

  it("末尾斜杠不影响命中", () => {
    expect(pageTitle("/guides/")).toBe(seoForPath("/guides")!.title);
  });

  it("未知路径落到 404 标题", () => {
    expect(pageTitle("/nope")).toMatch(/不存在/);
    expect(pageTitle("/help/nope")).toMatch(/没有这篇/);
    expect(pageTitle("/guides/nope")).toMatch(/没有这篇/);
  });
});

describe("指南内容", () => {
  it("slug 唯一", () => {
    const slugs = GUIDES.map((guide) => guide.slug);
    expect(new Set(slugs).size).toBe(slugs.length);
  });

  it("答案自包含：足够长，且不只是标题的复述", () => {
    for (const guide of GUIDES) {
      // 被引擎摘出来单独引用时也要说得通，太短的答案做不到。
      expect(guide.answer.length, `${guide.slug} 答案过短`).toBeGreaterThanOrEqual(80);
      expect(guide.answer).not.toBe(guide.title);
    }
  });

  it("每篇都有正文分节，且分节不空", () => {
    for (const guide of GUIDES) {
      expect(guide.sections.length, `${guide.slug} 没有分节`).toBeGreaterThan(0);
      for (const section of guide.sections) {
        expect(section.heading.length).toBeGreaterThan(0);
        expect(section.body.length, `${guide.slug} / ${section.heading} 空节`).toBeGreaterThan(0);
      }
    }
  });

  it("封号这篇必须写明无法保证，不做过度承诺", () => {
    const ban = GUIDES.find((guide) => guide.slug === "claude-code-ban")!;
    expect(ban.answer).toMatch(/没有任何方案能保证不被封/);
  });

  it("指南标题不与落地页标题重复", () => {
    const landingTitles = new Set(["/", "/claude"].map((path) => seoForPath(path)!.title));
    for (const guide of GUIDES) {
      const routeTitle = seoForPath(`/guides/${guide.slug}`)!.title;
      expect(landingTitles.has(routeTitle), `${guide.slug} 标题与落地页撞了`).toBe(false);
    }
  });

  it("品牌名在标题里只出现一次", () => {
    for (const guide of GUIDES) {
      const routeTitle = seoForPath(`/guides/${guide.slug}`)!.title;
      expect(routeTitle.match(/BestCodex/g)?.length ?? 0, `${guide.slug} 品牌名重复`).toBeLessThanOrEqual(1);
    }
  });
});
