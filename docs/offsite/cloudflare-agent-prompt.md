# 交给其他 Agent 的提示词（Cloudflare 解封 + Pages 关 SPA）

复制下面「提示词」整段发给一个**能开浏览器、能操作你已登录的 Cloudflare 控制台**的 Agent。不要改目标，不要让它改仓库代码。

---

## 提示词（整段复制）

```
你要在 Cloudflare 控制台改 bestcodex.app 的边缘配置。不要改 Git 仓库。改完用 curl 验收，把命令和输出贴回来。

目标域名：https://bestcodex.app
仓库里的源站 robots.txt 立场是「全部允许」（检索 / AI 检索 / AI 训练都放开）。线上现在被边缘层覆盖了。

## 背景（已核实，2026-08-17）

curl https://bestcodex.app/robots.txt 会看到：
- 开头是 Cloudflare Content Signals 长文
- `# BEGIN Cloudflare Managed content`
- `Content-Signal: search=yes,ai-train=no,use=reference`
- `User-agent: GPTBot` / `ClaudeBot` / `Bytespider` / `Applebot-Extended` 等是 `Disallow: /`
- 文件后半才是我们源站的 `Allow: /` 和 `ai-train=yes`

robots.txt 对同一 UA 先匹配到的规则生效，前面的 Disallow 会盖掉后面的 Allow。
另外：
- 首页已经有预渲染正文（#root 不是空的）
- /guides/ 有正确 title
- /en、/en/guides 仍返回中文首页 title（SPA 回写或旧构建）
- /install.sh 和 /install 返回 text/html（首页），不能当安装脚本
- 未知路径返回 200，不是 404

## 任务 A — 关掉托管 robots.txt

1. 打开 https://dash.cloudflare.com 选中 bestcodex.app 这个 zone。
2. 左侧找 **AI Crawl Control**（有的账号叫 AI Audit / AI crawlers）。进 **Robots.txt** 页签。
3. 把 **Managed robots.txt** / **Set your preference to block training in robots.txt** / **Manage your robots.txt** 关掉。
   目标状态名称可能是：Disable robots.txt configuration / Off。
   不要留在 Content Signals Policy 或 Instruct AI bot traffic。
4. 若 Overview 里有 **Display Content Signals Policy**，也关掉。

备用路径（菜单名变了就搜）：
- Security → Settings，过滤 Bot traffic
- Security → Bots → AI Crawl Control → Robots.txt
- Overview → Control AI Crawlers

## 任务 B — 关掉 HTTP 层拦 AI 爬虫

同一 zone：
1. Security → Bots，或 AI Crawl Control → Crawlers。
2. **Block AI bots** / **Block AI training bots** 设为 **Do not block (allow crawlers)**。
   不要留在 Block on all pages。
3. 若开着 Bot Fight Mode / Super Bot Fight Mode，确认不会把 GPTBot、ClaudeBot、PerplexityBot、Bytespider、Baiduspider 打成挑战或 403。需要的话给这些 UA 加 Skip 规则。
4. Security → WAF → Managed Rules：关掉名字里带 Manage AI / AI scrapers 且在拦爬虫的规则。

## 任务 C — Cloudflare Pages 关掉 SPA 回写

bestcodex.app 挂在 Cloudflare Pages（或等价静态托管）。找到对应 Pages 项目（名字可能是 bestcodex / lumio-codex / 类似）。

1. 确认生产部署跟的是 Git 分支 **publish**，最近提交应包含 `52c50df`（英文落地页）或更新。
   若生产还停在更早的 commit：先 Retry / Redeploy 最新 publish，等完成再测。
2. 找到 **Not Found handling** / **Single-page application** / **Redirect all requests to a single page**。
   设为 **404 page**（用构建产物里的 404.html），不要 SPA / index.html fallback。
3. 构建输出目录必须是 `web/apps/bestcodex/dist`（或该 app 的 dist），构建命令应是产品站的 `npm run build`，不是只跑 vite 而不跑 prerender。
4. 不要在 Pages 控制台再加一条 `/* /index.html 200`。仓库 `_redirects` 已经故意没有这条。

## 验收（必须跑，把输出贴回来）

等边缘生效约 1 分钟后执行：

```bash
# 1. robots.txt 必须是源站版本：没有 Managed 块，GPTBot 是 Allow
curl -s https://bestcodex.app/robots.txt | head -40
# 失败：出现 "BEGIN Cloudflare Managed" 或 GPTBot + Disallow
# 成功：开头接近 "# bestcodex.app"，且有 "User-agent: GPTBot" 后面是 "Allow: /"

# 2. AI 爬虫 UA 不能 403
for ua in "GPTBot/1.0" "ClaudeBot/1.0" "PerplexityBot/1.0" "Baiduspider/2.0"; do
  printf '%-22s %s\n' "$ua" "$(curl -s -o /dev/null -w '%{http_code}' -A "$ua" https://bestcodex.app/)"
done
# 成功：全是 200（301 也可以）。失败：403 / 429 / challenge HTML

# 3. 预渲染与语言
curl -s https://bestcodex.app/ | grep -o '<title>[^<]*</title>'
# 成功：BestCodex · 零配置用上官方 Codex
curl -s https://bestcodex.app/en | grep -o '<title>[^<]*</title>'
# 成功：BestCodex · official Codex with zero configuration
# 失败：仍是中文首页 title

curl -s https://bestcodex.app/en/guides | grep -o '<title>[^<]*</title>'
# 成功：Guides · BestCodex

# 4. 安装脚本必须是脚本，不能是 HTML
curl -sI https://bestcodex.app/install.sh | grep -i content-type
curl -sI https://bestcodex.app/install | grep -i content-type
curl -s https://bestcodex.app/install.sh | head -3
# 成功：content-type 含 text/plain 或 text/x-shellscript；正文以 #!/bin/sh 开头
# 失败：text/html 或 <!doctype

# 5. 真 404
curl -s -o /dev/null -w '%{http_code}\n' https://bestcodex.app/definitely-not-a-page
# 成功：404。失败：200

# 6. 静态发现文件
curl -sI https://bestcodex.app/sitemap.xml | grep -i content-type
curl -sI https://bestcodex.app/llms.txt | grep -i content-type
# 成功：xml / text/plain，不是 text/html
```

## 完成标准

同时满足才算完成：
1. robots.txt 不再含 `BEGIN Cloudflare Managed content`
2. GPTBot / ClaudeBot 对首页是 200
3. `/en` title 是英文
4. `/install.sh` 或 `/install` 至少一条是脚本正文
5. 未知路径是 404

## 做不到时

登录墙、2FA、权限不够、找不到菜单：停下来，说明卡在哪一页、截图或 URL，不要猜着关别的安全规则。
不要改 DNS、不要删 Pages 项目、不要改仓库。
```

---

## 你怎么用

1. 自己先在浏览器登录 https://dash.cloudflare.com（这个 Agent 没有你的账号）。
2. 把上面提示词发给一个带浏览器 / computer-use 的 Agent，让它在**已登录的那次会话**里点。
3. 它跑完验收命令后，把输出发我，我对照仓库判断还差哪一步。

如果那个 Agent 进不去控制台，把 Cloudflare API Token（Zone 的 Bot Management / Account 的 Pages 写权限）给我，我可以用 API 改，不必再开控制台。
