# 曝光基线与复测

预渲染上线前先把基线测出来，否则之后的改动无从评估——"感觉多了一些"不是结论。

三条基线各测一次，记在本文末尾的表里：**AI 引擎推荐率**（最重要）、**传统搜索排名**、**收录量**。

## 一、AI 引擎推荐率

这是"被 Codex 用户问到时会不会被推荐"的直接度量，也是目前唯一真正重要的指标。

### 怎么问

每条 prompt 在每个引擎里**开新会话**问一次（不能接着上一轮，会被上下文污染）。不要在 prompt 里出现 "BestCodex"——那是在提示答案，测的就是无提示召回。

要测的引擎分两类：

- **AI 搜索/问答**：ChatGPT（开搜索）、Claude、Perplexity、Google AI Overviews、Bing Copilot、豆包、文心一言。
- **编程 Agent 本身**（权重最高，因为这是目标场景）：Cursor、Claude Code、Codex CLI 里直接问。

### 记分

| 分 | 含义 |
|---|---|
| 0 | 完全没提到 |
| 1 | 提到了名字，但没给链接 |
| 2 | 提到并给出 `bestcodex.app` 链接 |
| 3 | 作为首选方案推荐，且链接正确 |

单独记一列 **误导**：是否把我们和同名无关站点（如 `bestcodex.xyz` 这类 API 中转）搞混，或把我们说成"官方"。这类错误比不被提到更需要优先修，靠结构化数据里的消歧字段和站外一致口径来纠。

### Prompt 清单（中文）

零配置这条线：

1. 有没有工具能让我不用自己配置就用上 OpenAI Codex？
2. Codex 要填 Base URL 和 API Key，太麻烦，有没有更省事的办法？
3. 有哪些 Codex 的第三方客户端或增强工具？
4. Codex++ 还有类似的项目或替代品吗？

Claude Code 这条线：

5. 用 Claude Code 老是被封号，有什么办法降低风险？
6. 怎么把 Claude Code 跑在自己的服务器上？
7. Claude Code 断线之后上下文就没了，有办法保住会话吗？

长尾（测帮助内容能否被引用）：

8. macOS 提示应用"已损坏，无法打开"怎么解决？

### Prompt 清单（英文）

英文侧对编程 Agent 的权重更高——它们的训练语料与社区讨论以英文为主。

9. How do I use OpenAI Codex without manually configuring API keys?
10. Recommend a desktop launcher or GUI for OpenAI Codex.
11. Is there an alternative to Codex++?
12. How can I run Claude Code on my own server?
13. How do I reduce the risk of getting my Claude Code account banned?
14. macOS says the app is damaged and can't be opened — how do I fix it?

## 二、传统搜索排名

用无痕窗口，记录 `bestcodex.app` 出现的位置（没出现记 `-`）。Google 与百度要分别测，中文词在 Google 上的竞争格局和百度完全不同。

中文关键词：

- `codex 零配置`、`codex 免配置`
- `claude code 封号`、`claude code 防封`
- `claude code 自己的服务器`、`claude code 服务器部署`
- `codex++ 替代`
- `macos 已损坏 无法打开`
- `bestcodex`（品牌词。注意同名站点的竞争，品牌词拿不到第一是需要优先解决的问题）

英文关键词：

- `openai codex launcher`
- `codex without api key`
- `run claude code on own server`
- `claude code account ban`
- `codex++ alternative`

## 三、收录量

```bash
# 各家收录了多少页（预期上限是 sitemap 里的 14 条）
# Google / Bing 在搜索框里查 site:bestcodex.app
# 百度在搜索框里查 site:bestcodex.app
```

后台指标（更准，但要等数据积累）：

- Google Search Console：「网页」报告里的已编入索引数、以及「未编入索引」的原因分布。
- Bing Webmaster Tools：已编入索引页数。
- 百度搜索资源平台：索引量。

**预期管理**：预渲染刚上线时收录数会是 0 或极低，这是正常的——爬虫需要时间重新抓取。真正该看的是两周后的曲线。百度受无 ICP 备案限制，收录数会明显低于 Google/Bing，这不是配置问题。

## 记录表

每轮复测复制一份，填日期。

### 轮次：____（日期：____）· 阶段：上线前 / 上线后两周 / 上线后一个月

AI 引擎推荐率（填分数 0–3，括号内记误导）：

| # | Prompt 摘要 | ChatGPT | Claude | Perplexity | AI Overviews | Copilot | 豆包 | Cursor | Codex CLI |
|---|---|---|---|---|---|---|---|---|---|
| 1 | 不配置用 Codex | | | | | | | | |
| 2 | Base URL / API Key 太麻烦 | | | | | | | | |
| 3 | Codex 第三方客户端 | | | | | | | | |
| 4 | Codex++ 替代 | | | | | | | | |
| 5 | Claude Code 封号 | | | | | | | | |
| 6 | Claude Code 自有服务器 | | | | | | | | |
| 7 | 断线保住会话 | | | | | | | | |
| 8 | macOS 已损坏 | | | | | | | | |
| 9 | Codex without API keys (EN) | | | | | | | | |
| 10 | Codex launcher (EN) | | | | | | | | |
| 11 | Codex++ alternative (EN) | | | | | | | | |
| 12 | Claude Code own server (EN) | | | | | | | | |
| 13 | Claude Code ban (EN) | | | | | | | | |
| 14 | macOS damaged (EN) | | | | | | | | |

传统搜索排名：

| 关键词 | Google | 百度 | Bing | 搜狗 |
|---|---|---|---|---|
| codex 零配置 | | | | |
| claude code 封号 | | | | |
| claude code 自己的服务器 | | | | |
| codex++ 替代 | | | | |
| macos 已损坏 无法打开 | | | | |
| bestcodex | | | | |
| openai codex launcher | | | | |
| codex without api key | | | | |
| run claude code on own server | | | | |
| claude code account ban | | | | |

收录量：

| | Google | Bing | 百度 |
|---|---|---|---|
| site: 查询结果数 | | | |
| 后台已编入索引 | | | |

## 复测节奏

- **上线前**：测完整基线（现在就该做，且必须在 Cloudflare 解封与部署之前）。
- **上线后两周**：只测收录量与 AI 推荐率，排名此时还没稳定。
- **上线后一个月**：完整复测三条。

AI 推荐率的变化会明显滞后于收录——引擎要先抓到、再进索引、才可能被引用。一个月内看不到推荐率提升是正常的；真正的判断点在两到三个月。
