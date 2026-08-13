# 04 · 上线验收清单

按 [02-domains-and-dns.md](./02-domains-and-dns.md) §4 的顺序推进，每完成一步回来勾一段。
**没勾完不宣布上线**；勾选依据是实际操作结果，不是「应该没问题」。

## A. 三站可达且风格一致

- [ ] `https://lumiogame.com/` 打开是 Lumio 门户（不是 Workflow 页、不是维护页）
- [ ] `https://cc.lumiogame.com/` 打开是 CC避风港产品站
- [ ] `https://codex.lumiogame.com/` 打开是 Lumio Codex 产品站
- [ ] 三站证书有效、无混合内容告警；`www` 与裸域策略自洽
- [ ] 三站顶栏 / 页脚是同一套外壳与配色（共用 `@lumio/ui`，视觉不该有第二种风格）
- [ ] 深链接直接访问不 404（SPA 回退生效）：`/login`、`/signup`、`/account`、`/logout`（门户），
      `/pricing`、`/download`（CC）
- [ ] 浏览器控制台没有 CORS 报错

## B. 门户账号全链路

用一个**全新邮箱**在生产上真的跑一遍，不要只看页面能打开：

- [ ] 注册：`/signup` 能拿到验证码（若 Sub2API 开了邮箱验证）并注册成功
- [ ] 登录：`/login` 成功后停在账户中心
- [ ] 2FA：为测试账号开启 TOTP 后重新登录，能进 2FA 步骤并通过
- [ ] 账户中心 `/account` 显示邮箱、状态与余额（余额是真实数字，不是长期 `¥0.00`）
- [ ] 「充值」按钮打开 `https://api.lumio.games/purchase` 且页面可用
- [ ] 退出登录后刷新，三站都回到未登录态
- [ ] 注册 / 登录失败路径（错误口令、限流）显示的是可读中文提示，不是原始报错

## C. 跨站会话与回跳

- [ ] 在门户登录后，打开 CC 站与 Codex 站，右上角显示已登录邮箱
      （令牌在父域 cookie `.lumiogame.com`，见 `web/packages/auth/src/session.ts`）
- [ ] 在 CC 站点「登录」跳到门户，URL 带 `?next=`，登录后回到原来的 CC 页面
- [ ] 在 Codex 站点「账户」跳门户同理
- [ ] 手工把 `next` 改成站外地址（如 `?next=https://example.com`）后登录，
      **不会**跳到外站（开放重定向防护，`isAllowedNext()`）
- [ ] 门户退出登录后，CC / Codex 站刷新也变成未登录

## D. CC 控制面的迁移后行为

```bash
# 期望 410，body 里 details.portal_url 指向门户登录页
curl -isS -X POST https://api.cc.lumiogame.com/api/v1/auth/login \
  -H 'Content-Type: application/json' -d '{"email":"a@b.c","password":"x"}'

# 期望 303，Location = https://api.lumio.games/purchase
curl -isS -X POST https://api.cc.lumiogame.com/api/v1/billing/checkout

# 期望 401（乱写的令牌）
curl -isS -H 'Authorization: Bearer nope' https://api.cc.lumiogame.com/api/v1/me

# 期望 200
curl -isS https://api.cc.lumiogame.com/api/v1/health
```

- [ ] `/api/v1/auth/login` 等自有认证端点返回 **410 `auth_migrated`**
- [ ] `/api/v1/billing/checkout` 返回 **303**，`Location` 为 Sub2API 充值页
- [ ] 无效令牌返回 **401**；预发环境把 `CCHAVEN_SUB2API_BASE` 指向不可达地址时返回
      **503 `identity_unavailable`**（不是 200、也不是 401）
- [ ] 用门户登录得到的令牌调 `/api/v1/me`，首次访问即在 CC 侧开出影子账号并返回 200
- [ ] 运营后台 `https://admin.cchaven.cn` 登录 + TOTP 正常（未受身份收口影响）

## E. Codex 桌面端不受影响

- [ ] 存量客户端仍连 `https://api.lumio.games`（`API_BASE_URL` 未变，本次不动这个域名）
- [ ] 桌面端登录 / 刷新令牌 / 拉余额正常
- [ ] 桌面端「充值」打开 `https://api.lumio.games/purchase`
- [ ] 桌面端里的官网链接指向 `https://codex.lumiogame.com`（`SITE_BASE_URL`）

## F. 下载区能取到安装包

- [ ] Codex 站下载卡片显示真实版本号（不是「CDN 暂不可用 · GitHub 回退」）
- [ ] `https://codex.lumiogame.com/latest-internal.json` 返回最新指针，
      `version` 与本次发布一致（发布后要重新复制，见 [01](./01-web-sites-deploy.md) §4）
- [ ] 三个平台的下载链接都能真正下到文件（macOS arm64 / macOS x64 / Windows setup）
- [ ] 确认弹窗里的未签名说明与实际渠道一致（当前是 `internal-unsigned`）
- [ ] CC 站下载页：已配 `VITE_CC_DOWNLOAD_*` 时链接可下载；未配时是空态而不是坏链接

## G. 旧域名与收尾

- [ ] `https://lumio.games/` 跳到 `https://codex.lumiogame.com/`
- [ ] `https://cchaven.cn/` 跳到 `https://cc.lumiogame.com/`
- [ ] `https://api.cchaven.cn/<任意路径>` 跳到 `api.cc.lumiogame.com` 的**同一路径**
- [ ] 旧 CC 官网的注册 / 登录入口已不可达（那些端点已经 410，留着只会让用户点进死表单）
- [ ] 旧 GitHub Pages 站已停用自定义域名（见 [`codex/docs/ops/02-website-deploy.md`](../../codex/docs/ops/02-website-deploy.md)）
- [ ] 对外文案里的域名已更新（产品站文案、README、发版说明）

## H. 监控最低集

- [ ] `https://api.lumio.games/` 外部拨测（账号全链路的单点）
- [ ] `https://api.lumio.games/purchase` 拨测（充值入口故障按 P0 处理）
- [ ] 三站首页可用性拨测
- [ ] `https://api.cc.lumiogame.com/api/v1/health` 拨测
- [ ] 证书到期提醒（三个新子域 + API 域名）
