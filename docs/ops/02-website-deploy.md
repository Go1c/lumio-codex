# 02 · 官网部署（lumio.games）

## 1. 内容来源

| 项 | 路径 |
|----|------|
| 静态站点根 | [`site/`](../../site/) |
| 入口 | `site/index.html` |
| 样式 / 脚本 | `site/styles.css`、`site/site.js` |
| 域名提示 | `site/CNAME` → `lumio.games` |
| 交互规格 | [`docs/specs/2026-08-12-lumio-ux-interaction-design.md`](../specs/2026-08-12-lumio-ux-interaction-design.md) §3 |

官网**不承载**登录 / 注册 / 账户入口。下载确认后跳转 GitHub Releases。

## 2. 本地预览

```bash
# 任意静态服务器均可，例如：
cd site
python3 -m http.server 8080
# 浏览器打开 http://127.0.0.1:8080/
```

或直接用浏览器打开 `site/index.html`（部分字体 CDN 需联网）。

## 3. 推荐部署：GitHub Pages

仓库设置 → Pages：

1. Source：Deploy from a branch  
2. Branch：`publish`（或你们锁定的发版分支）  
3. Folder：`/site`  
4. 自定义域名：`lumio.games`  
5. 启用 HTTPS（DNS 生效后勾选）

DNS（域名提供商）：

| 类型 | 主机 | 值 |
|------|------|-----|
| `A` / `AAAA` 或 `CNAME` | `@` / `www` | 按 [GitHub Pages 自定义域名文档](https://docs.github.com/pages/configuring-a-custom-domain-for-your-github-pages-site) 填写 |

仓库内已有 `site/CNAME`，推送后 Pages 会识别。

> 若使用 Cloudflare / 自建 Nginx / S3+CloudFront，把 `site/` 整目录当作文档根即可；确保 `index.html` 为默认页。

## 4. 支付路径（API 站，不是官网）

桌面端在线充值打开：

```text
https://api.lumio.games/purchase
```

常量见 `crates/codex-plus-core/src/lumio/product.rs`（`API_BASE_URL` + `PAYMENT_PATH`）。**不要**配成 `https://lumio.games/...`。

支付页由 Sub2API / `api.lumio.games` 提供。官网 `site/` 只做营销与下载，不承载充值。  
（规格里的一次性 payment-handoff API 尚未作为强制依赖；当前是「打开网站」。）

## 5. 部署后验收

- [ ] `https://lumio.games/` 打开为 Lumio 官网（非旧 Codex++ 页）  
- [ ] 顶栏「下载」滚到下载区；确认层后能进 Releases  
- [ ] FAQ 三条可用；无「登录 / 注册」导航  
- [ ] `https://api.lumio.games/purchase` 可达  
- [ ] HTTPS 有效；`www` 与裸域策略自洽  

## 6. 改官网内容时

1. 改 `site/`（保持 §3 五块结构，勿加账户入口）  
2. 本地预览  
3. 提交并推送触发 Pages 更新  
4. 在 [05-maintenance.md](./05-maintenance.md) 的文档义务中同步规格若文案基线变了  
