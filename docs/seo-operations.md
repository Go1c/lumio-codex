# 搜索引擎与 AI 引擎收录运维

面向 `bestcodex.app`。分三部分：构建已经自动做掉的、必须你在各家后台手动做的、以及上线后怎么核对。

效果怎么量、复测怎么记 → [`geo-baseline.md`](geo-baseline.md)。**基线要在解封与部署之前测**，否则失去对照。

## 一、构建已经自动做掉的

跑 `npm run build -w @lumio/bestcodex` 会产出：

| 产物 | 作用 |
|---|---|
| 21 份静态 HTML（每条路由一份，正文已在 `#root` 里） | 不执行 JS 的爬虫也能读到内容。这是所有收录的前提 |
| `sitemap.xml` | 20 条可索引正本；`/codex` 因 canonical 指向 `/` 被排除。中英配对的页面带 `xhtml:link` |
| `robots.txt` | 全部放开，含 Google / Bing / 百度 / 搜狗 / 360 / 神马 与各家 AI 爬虫 |
| `llms.txt` | 给 AI 引擎的站点摘要与页面清单 |
| 15 份 `.md` 纯文本镜像 | 编程 Agent 更愿意吃 Markdown |
| `404.html` | 未命中路径返回真 404，而不是全站软 200；带 `noindex` |
| `_redirects` | `/pricing`、`/download` 走真 301 |
| 每页 JSON-LD | Organization / WebSite / SoftwareApplication / FAQPage / TechArticle / BreadcrumbList |
| 每页 `<meta name="robots">` | 放开 `max-snippet:-1` 与 `max-image-preview:large`，让 SERP 与 AI 摘要能取完整段落 |
| `<html lang>` + `hreflang` | 中英指南双向互指，`x-default` 指中文；`<head>` 与 sitemap 各一份 |

## 二、必须手动做的

### 0. 先解 Cloudflare 的封锁（前置，最重要）

**这一步不做，下面全部无效。** Cloudflare 的托管规则在边缘层覆盖源站，源站的 `robots.txt` 写什么都不算。

在 Cloudflare 控制台关掉：

- **Managed robots.txt**（会覆盖我们的 `robots.txt`）
- **AI Scrapers and Crawlers / Block AI bots**（在 HTTP 层直接挡掉 GPTBot、ClaudeBot 等）

关完后用第三节的命令核对线上实际返回的 `robots.txt`。

### 1. Google Search Console

1. 添加资源（选「网址前缀」，填 `https://bestcodex.app`）。
2. 验证：把令牌写进环境变量 `SITE_VERIFY_GOOGLE`，重新构建部署即可（会注入首页 `<meta name="google-site-verification">`）。
3. 提交 `https://bestcodex.app/sitemap.xml`。
4. 用「网址检查」对 `/`、`/claude`、`/guides/claude-code-ban` 各跑一次，确认「网页可编入索引」且渲染后的 HTML 有正文。
5. 在「国际定位」里核对 hreflang 无报错。报「没有返回标记」通常是单向互指，检查 `seo.ts` 里两侧的 `alternatePath`。

Google **没有开放的推送端点**——Indexing API 只对招聘与直播类结构化数据开放，普通页面只能靠 sitemap 加 URL 检查里的「请求编入索引」。

### 2. Bing Webmaster Tools

1. 可以直接从 Google Search Console 导入，省一次验证。
2. 手动验证走 `SITE_VERIFY_BING`（注入 `<meta name="msvalidate.01">`）。
3. 提交 sitemap。
4. 开 IndexNow（见第 6 条），Bing 是 IndexNow 的主要消费方。

### 3. 百度搜索资源平台（ziyuan.baidu.com）

1. 添加站点，验证走 `SITE_VERIFY_BAIDU`（注入 `<meta name="baidu-site-verification">`）。
2. 提交 sitemap。注意百度基本不读 `robots.txt` 里的 `Sitemap:` 指令，必须在后台提交。
3. 在「普通收录 → 主动推送」拿 token，配到 `BAIDU_PUSH_TOKEN`，用 `npm run seo:push` 推。

4. 百度不读 hreflang，`/en/` 那一层在百度眼里就是另一批页面；只按中文页做优化即可。

**关于预期，说实话：** `bestcodex.app` 是境外托管、`.app` 域名、**没有 ICP 备案**。百度对这类站点的排名有明显天花板——能收录，但很难拿到靠前位置。想认真做百度流量，前提是备案 + 国内节点，那是另一个决策，不是 SEO 能补的。搜狗、360 同理，程度轻一些。

### 4. 搜狗站长平台

验证走 `SITE_VERIFY_SOGOU`，提交 sitemap。搜狗**没有公开的推送接口**，只能等抓取。

### 5. 360 与神马

- 360：`SITE_VERIFY_360`，在 360 站长平台提交 sitemap。
- 神马（移动端，UC / 阿里系）：`robots.txt` 已放开 `Yisouspider`；神马主要靠移动端抓取，站长平台入口时有变动，优先保证移动端可访问。

### 6. IndexNow（一次提交到 Bing、DuckDuckGo、Yandex、Seznam、Naver）

1. 生成一个 8–128 位的十六进制密钥（例如 `openssl rand -hex 16`）。
2. 配 `INDEXNOW_KEY`，重新构建——会在站点根产出 `<key>.txt`，内容就是密钥本身，用来证明域名归属。
3. 部署后先 `npm run seo:push` 看 dry-run，确认 URL 列表无误，再 `npm run seo:push -- --live`。

### 环境变量一览

构建期读取，都是可选的；没配就跳过，不会产出空标签。

```bash
SITE_VERIFY_GOOGLE=...
SITE_VERIFY_BING=...
SITE_VERIFY_BAIDU=...
SITE_VERIFY_SOGOU=...
SITE_VERIFY_360=...
SITE_VERIFY_YANDEX=...
INDEXNOW_KEY=...          # 同时用于产出 <key>.txt
BAIDU_PUSH_TOKEN=...      # 仅 seo:push 用，不进构建产物
```

## 三、上线后核对

```bash
# robots.txt 是我们的版本，不是 Cloudflare 托管的那份
curl -s https://bestcodex.app/robots.txt | head -20

# 首页 HTML 里有正文（不是空 div#root）。数字应远大于 0
curl -s https://bestcodex.app/ | grep -c "三步开始"

# 折叠的 FAQ 答案也在
curl -s https://bestcodex.app/ | grep -c "xattr -cr"

# canonical 与结构化数据
curl -s https://bestcodex.app/codex | grep -o '<link rel="canonical"[^>]*>'   # 应指向 /
curl -s https://bestcodex.app/ | grep -o '"@type":"[A-Za-z]*"' | sort -u

# 真 301 而不是客户端跳转
curl -sI https://bestcodex.app/pricing | head -3

# 未知路径是真 404，不是 200 软 404
curl -sI https://bestcodex.app/definitely-not-a-page | head -1

# 语言标记与 hreflang（中英两侧应给出同一组三条）
curl -s https://bestcodex.app/guides/claude-code-ban | grep -o '<html lang="[^"]*"'
curl -s https://bestcodex.app/en/guides/claude-code-ban | grep -o 'hreflang="[^"]*"'

# AI 引擎入口
curl -s https://bestcodex.app/llms.txt | head -10
curl -s https://bestcodex.app/guides/claude-code-ban.md | head -5
```

模拟各家爬虫（重点验 Cloudflare 没在 HTTP 层挡人）：

```bash
for ua in "GPTBot/1.0" "ClaudeBot/1.0" "Baiduspider/2.0" "Sogou web spider/4.0" "bingbot/2.0"; do
  printf '%-24s %s\n' "$ua" "$(curl -s -o /dev/null -w '%{http_code}' -A "$ua" https://bestcodex.app/)"
done
```

全部应返回 `200`。出现 `403` 就是 Cloudflare 还在挡。

## 四、内容侧的约定

新增页面时同步三处，否则会漂移：

1. `web/apps/bestcodex/src/seo.ts` —— 路由的 title / description / canonical / locale / JSON-LD（sitemap 与 llms.txt 都由它生成）。
2. `web/apps/bestcodex/src/guides.ts` 与 `guides.en.ts` —— 指南正文。两边 **slug 必须一一对应**，hreflang 靠它配对。`answer` 必须**自包含**：被引擎单独摘出来引用时也说得通。
3. 价格与能力口径要与 `content.ts`、帮助中心、Sub2API 收银台一致，中英两版口径也必须一致。
4. 新页面必须从某个落地页或页脚链得到。只进 sitemap 的页面是孤岛，百度尤其依赖站内链接图。

写回答型内容的要点：标题就是用户的原话问句；第一段直接给结论；不做无法兑现的承诺（例如封号只能说降低风险，不能说保证不封）——过度承诺在 AI 引擎里反而更难被引用。
