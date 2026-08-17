---
name: seo-geo
description: 搜索引擎与 AI 引擎可见性——预渲染契约、SSR 守卫、元数据单一权威与回答型内容约定;改产品站页面、路由或文案时查
metadata:
  type: doc
  status: 已交付
---

# 搜索引擎与 AI 引擎可见性

产品站 `bestcodex.app` 要同时被两类消费者读懂：传统爬虫（Google / Bing / 百度 / 搜狗 / 360 / 神马）与 AI 引擎（ChatGPT、Claude、Perplexity 及编程 Agent）。两者的共同前提只有一个——**内容必须在首屏 HTML 里**。

各家后台的提交步骤、验证令牌与推送用法在 [`docs/seo-operations.md`](../../../docs/seo-operations.md)，本文只写改代码时必须遵守的约束。

## 硬约束

**站点是构建期预渲染的，不是纯 SPA。** `npm run build -w @lumio/bestcodex` 会 `renderToString` 每条路由，把正文写进 `#root`。破坏预渲染就等于让全站对爬虫隐形，而且不会有任何报错提示——所以下面几条由测试守着。

1. **渲染路径不得直接摸浏览器 API。** `window` / `document` / `navigator` 在构建期不存在。需要分环境时问 `isServerRender()`（`@lumio/ui`），服务端分支一律选「内容已经在 HTML 里」的那一支：不要 loading 文案、不要初始 `opacity:0`、不要依赖设备探测的结果。
2. **正文不得条件渲染。** 折叠、手风琴、Tab 里的内容要**始终进 DOM**，用 `hidden` 或 CSS 收起。条件渲染的那部分正文对爬虫等于不存在。
3. **`useLayoutEffect` 在服务端会告警**，需要时按 `App.tsx` 里 `useIsomorphicLayoutEffect` 的写法退化成 `useEffect`。
4. **不用 `hydrateRoot`。** 客户端仍是 `createRoot`，React 会丢弃预渲染的 HTML 重新渲染。代价是一次极小的重绘，换来彻底规避会话状态 / 设备探测 / 动效三类 hydration 不一致。改成 hydrate 前先想清楚这三类怎么对齐。

## 元数据的单一权威

`web/apps/bestcodex/src/seo.ts` 是唯一权威，被三处消费：预渲染注入 `<head>`、客户端换页同步 `document.title`、构建脚本生成 `sitemap.xml` 与 `llms.txt`。

**新增路由必须同时在 `seo.ts` 登记**，否则预渲染会直接报错（这是有意的：漏登记等于新页面不被收录）。登记时注意：

- 同内容的不同路径（如 `/codex` 与 `/`）用 `canonicalPath` 指向正本，重复页不进 sitemap。
- 可索引正本之间**标题不得重复**，否则两页互抢同一组关键词。
- `description` 中文控制在 130 字、英文 160 字以内。SERP 按像素宽度截断，中文字符是英文的两倍宽。
- `locale` 决定预渲染产出的 `<html lang>` 与 `og:locale`，必填。

## 中英双语与 hreflang

英文层覆盖落地页（`/en`、`/en/claude`）与指南（`/en/guides`）。约束：

- **`guides.en.ts` 的 slug 必须与 `guides.ts` 一一对应**，`alternatePath` 靠相同 slug 互指。
- **hreflang 必须双向。** 单向声明会被搜索引擎直接忽略，等于没写。`x-default` 一律指中文版。只有一种语言存在的页面不发 hreflang。
- 英文页正文不得残留中文，站内链接不得指回中文页——语言混排会让引擎判不准页面语言。
- 免责声明、价格、承诺的**口径两种语言必须一致**，英文是翻译而不是另写一版。
- 百度不读 hreflang，`/en/` 在它眼里是另一批页面，只按中文页优化。

## 站内链接图

**新页面必须从某个落地页或页脚链得到。** 只进 sitemap 的页面对爬虫是孤岛：抓得慢、权重低，百度尤其依赖站内链接图去发现和评估页面。页脚互链由 `SiteShell` 的 `footerLinks` 提供，产品站顶栏的 `nav` 条目渲染在账号区之前。

## 回答型内容约定

指南正文在 `web/apps/bestcodex/src/guides.ts`。这类页面是被 AI 引擎引用的主力，写法有讲究：

- `question` 就是用户的原话问句，用户/Agent 怎么问就怎么写。
- `answer` 必须**自包含**：被摘出来单独引用时也说得通，且第一句就给结论。
- **不做无法兑现的承诺。** 封号只能说降低风险，不能说保证不封。过度承诺在 AI 引擎里反而更难被引用，也更容易被用户当场证伪。
- 价格与能力口径必须与 `content.ts`、帮助中心、Sub2API 收银台一致。`seo.ts` 是第三处引用价格的地方，改价要同步全部。

## 边缘层会覆盖源站

Cloudflare 的 **Managed robots.txt** 会覆盖我们的 `public/robots.txt`，**Block AI bots** 会在 HTTP 层直接挡掉 AI 爬虫。这两个开关不关，仓库里的 `robots.txt` 写什么都不算，且本地怎么测都测不出来——只能对线上域名发请求核对。核对命令见运维文档。

## 测试守着什么

- `src/__tests__/prerender.node.test.tsx` 跑在 **node 环境**（`// @vitest-environment node`），等于构建期的真实条件：断言正文进 HTML、折叠答案也在、没有 `opacity:0`、下载区不输出 loading 文案也不瞎猜平台、指南层不是孤岛、hreflang 双向且 `x-default` 指中文。放到 jsdom 里跑会走客户端分支，测不出任何东西。
- `src/__tests__/seo.test.ts` 断言元数据约束：标题唯一、canonical 自洽、JSON-LD 可序列化、描述长度按语言分档、locale 与路径一致、中英 slug 对齐、英文正文无中文残留、指南答案自包含。
