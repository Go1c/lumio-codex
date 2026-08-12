# 02 · 旧官网（lumio.games）下线与过渡

> **官网已经搬家。** Lumio Codex 的产品站现在是 `web/apps/codex`，域名
> `codex.lumiogame.com`，构建与分发写在根 [`docs/ops/`](../../../docs/ops/README.md)：
>
> - 构建与静态托管 → [`docs/ops/01-web-sites-deploy.md`](../../../docs/ops/01-web-sites-deploy.md)
> - 域名、证书、301 → [`docs/ops/02-domains-and-dns.md`](../../../docs/ops/02-domains-and-dns.md)
> - 上线验收 → [`docs/ops/04-golive-checklist.md`](../../../docs/ops/04-golive-checklist.md)
>
> **本文只剩两件事**：旧 GitHub Pages 站怎么下线，以及过渡期怎么安排。
> 新站的步骤不要复制到这里，避免两份互相打架的说法。

## 1. 旧站是什么

| 项 | 路径 |
|----|------|
| 静态站点根 | [`site/`](../../site/) |
| 入口 | `site/index.html` |
| 样式 / 脚本 | `site/styles.css`、`site/site.js` |
| 域名提示 | `site/CNAME` → `lumio.games` |
| 发布指针 | `site/latest-internal.json`（新站同名文件的格式参考） |

部署方式是 GitHub Pages（仓库设置 → Pages，源为发版分支的 `/site` 目录，自定义域名
`lumio.games`）。旧站不承载登录 / 注册 / 账户入口，下载确认后跳 GitHub Releases 或 CDN。

## 2. 过渡期（新站已上线、旧站还没停）

两套站可以并存一段时间，期间：

- **内容以新站为准**。旧站只做兜底，不要再往 `site/` 加新功能或新文案。
- 发内测包后，发布指针要同时更新新站的 `/latest-internal.json`
  （见 [`docs/ops/01-web-sites-deploy.md`](../../../docs/ops/01-web-sites-deploy.md) §4）；
  旧站读的是 CDN 上的同一份，不需要单独发布。
- 桌面端里的官网链接已经指向新站（`crates/codex-plus-core/src/lumio/product.rs`
  的 `SITE_BASE_URL` = `https://codex.lumiogame.com`），所以新装的客户端不会再把用户带回旧站。

## 3. 下线步骤

前提：`codex.lumiogame.com` 已可用并通过验收（[`docs/ops/04-golive-checklist.md`](../../../docs/ops/04-golive-checklist.md) A / F 两节）。

1. 在 DNS 侧把 `lumio.games` 改成 301 到 `https://codex.lumiogame.com/`
   （先跑一轮 302 验证，见 [`docs/ops/02-domains-and-dns.md`](../../../docs/ops/02-domains-and-dns.md) §3）。
2. 仓库设置 → Pages：移除自定义域名 `lumio.games`，或直接把 Pages 关掉。
3. 确认 `https://lumio.games/` 跳到新站，且新站的下载区正常。
4. `site/` 目录暂时保留（历史与文案参考，`latest-internal.json` 还是格式基准），
   不再作为部署来源；真要删除需单独决策。

## 4. 不变的事

- 充值仍然是 `https://api.lumio.games/purchase`，常量在
  `crates/codex-plus-core/src/lumio/product.rs`（`API_BASE_URL` + `PAYMENT_PATH`）。
  **不要**配成任何官网域名。
- `api.lumio.games` 本身不迁移、不改动——存量桌面客户端硬编码了它（见 [04](./04-backend.md)）。
