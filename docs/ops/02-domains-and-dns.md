# 02 · 域名与 DNS 切换清单

本篇只管**域名侧**：新增记录、证书、旧域名 301、切换顺序与回滚。
构建与分发看 [01-web-sites-deploy.md](./01-web-sites-deploy.md)，
Sub2API 与控制面的服务端要求看 [03-service-prerequisites.md](./03-service-prerequisites.md)。

## 1. 需要新增 / 变更的记录

| 域名 | 记录 | 指向 | 备注 |
| --- | --- | --- | --- |
| `lumiogame.com` | `A`/`AAAA` 或 `CNAME` | 门户静态托管入口 | **当前挂着 Workflow 介绍页，必须先迁离**，见 §2 |
| `www.lumiogame.com` | `CNAME` | 同上 | 与裸域策略二选一并保持自洽（301 到裸域即可） |
| `cc.lumiogame.com` | `A`/`AAAA` 或 `CNAME` | CC 产品站静态托管入口 | 新增 |
| `codex.lumiogame.com` | `A`/`AAAA` 或 `CNAME` | Codex 产品站静态托管入口 | 新增 |
| `api.cc.lumiogame.com` | `A`/`AAAA` | 控制面所在主机 / 网关 | 新增；**域名本身待用户最终确认**，已写进代码默认值 |
| `api.lumio.games` | —— | **保持不动** | 存量桌面客户端硬编码，任何变更都会让老版本客户端整体失联 |
| `lumio.games` | 改指向 | 承载 301 的入口 | 旧 Codex 官网（GitHub Pages）下线后改成跳转，见 §3 |
| `cchaven.cn` | 改指向 | 承载 301 的入口 | 旧 CC 官网下线后改成跳转 |
| `admin.cchaven.cn` | —— | **保持不动** | 运营后台不随本次迁移换域名 |

证书：`lumiogame.com` 建议直接签一张覆盖 `lumiogame.com` + `*.lumiogame.com` 的证书，
省得每加一个子域重签。用 Cloudflare Pages / 托管平台的自动证书时，三个站各自签发即可。
`api.cc.lumiogame.com` 的证书由控制面前面的网关持有（Let's Encrypt 或云证书）。

## 2. 前置：`lumiogame.com` 上的 Workflow 页面必须先迁离

`lumiogame.com` 目前指向的是「Workflow」产品介绍页——那是与本仓无关的第三个产品，
**代码不在本仓库**，本仓没有任何改动能腾出这个域名。

上线门户前必须先完成（由域名 / 托管的负责人操作）：

- [ ] Workflow 页面确认新家（另一个域名或子域），并在那边可正常访问
- [ ] Workflow 的存量入口（外链、二维码、渠道投放）已指到新地址，或接受 301
- [ ] `lumiogame.com` 的 DNS 与托管配置可以被改写（有账号权限、无锁定）

这三条没完成之前，门户只能先部署到临时地址做验收；**不要**为了赶进度把门户挤在
Workflow 页面的子路径下——`web/apps/portal` 是按站点根路径构建的，
`packages/ui/src/config.ts` 里的跨站链接也按根域拼接。

## 3. 旧域名 301

| 旧地址 | 新地址 |
| --- | --- |
| `https://lumio.games/*` | `https://codex.lumiogame.com/` |
| `https://cchaven.cn/*` | `https://cc.lumiogame.com/` |
| `https://api.cchaven.cn/*` | `https://api.cc.lumiogame.com/`（保留路径） |

- 旧站的路径结构与新站对不上（旧 Codex 官网是单页锚点站，旧 CC 官网还有注册 / 登录页），
  所以 **`lumio.games` 与 `cchaven.cn` 一律跳新站首页，不做路径映射**；只有 API 域名要保留
  路径与查询串，否则存量客户端的接口调用会全部落到根路径。
- `cchaven.cn` 旧站上的注册 / 登录 / 找回密码入口在 301 之后自然消失——这正是要的效果，
  控制面对应端点已经返回 410（见 [03-service-prerequisites.md](./03-service-prerequisites.md)）。
- 301 是永久跳转，浏览器会缓存。**先用 302 验证一轮**，确认目标正确后再改 301。

nginx 示意：

```nginx
server {
  listen 443 ssl;
  server_name lumio.games www.lumio.games;
  return 301 https://codex.lumiogame.com/;
}

server {
  listen 443 ssl;
  server_name cchaven.cn www.cchaven.cn;
  return 301 https://cc.lumiogame.com/;
}

server {
  listen 443 ssl;
  server_name api.cchaven.cn;
  return 301 https://api.cc.lumiogame.com$request_uri;   # API 必须保留路径
}
```

旧的 Codex 官网托在 GitHub Pages 上（`codex/site/CNAME` 写着 `lumio.games`），
下线步骤见 [`codex/docs/ops/02-website-deploy.md`](../../codex/docs/ops/02-website-deploy.md)。

## 4. 切换顺序

按「新地址先可用、旧地址后失效」的顺序推进，任何一步失败都可以停在原地：

1. **加子域记录**：`cc.` 与 `codex.` 指向新托管，证书生效，两站可用 https 打开。
   此时旧域名照常服务，两套并存，用户无感。
2. **验收产品站**：按 [04-golive-checklist.md](./04-golive-checklist.md) 的产品站部分逐条过。
   账号入口此刻会跳向还没上线的门户——**这一步允许失败，别当作阻塞**。
3. **完成 Workflow 迁离**（§2），切 `lumiogame.com` 到门户，验收账号全链路。
4. **旧域名改 302** 到新站，观察一到两天（看访问日志与用户反馈）。
5. **302 改 301**，并在旧托管上停掉源站（GitHub Pages 关掉自定义域名、旧 CC 站停发布）。
6. **收尾**：更新对外文案里的域名（产品站下载页、README、发版说明），
   把桌面端安装包里的默认地址与实际域名核对一遍。

`api.lumio.games` 全程不动；`api.cc.lumiogame.com` 要在**桌面端发新包之前**就位，
因为桌面端默认值已经指向它（`CCHAVEN_API_BASE`）。

## 5. 回滚

| 出问题的环节 | 回滚办法 |
| --- | --- |
| 新子域内容有问题 | 重新发布上一份 `dist/`；DNS 不动 |
| 门户切换后账号链路不通 | 把 `lumiogame.com` 指回原托管（Workflow 页或维护页），产品站保持在线——产品站不依赖门户可用性，只是账号入口点不进去 |
| 301 跳错目标 | 因为浏览器会缓存 301，只能改 301 的目标地址（不能简单删掉）；这也是 §3 要求先跑 302 的原因 |
| `api.cc.lumiogame.com` 不可用 | 恢复 `api.cchaven.cn` 的原有服务并撤掉它的 301；桌面端已发包的默认值改不了，只能靠这个旧域名兜底 |

DNS TTL 在切换前调低（建议 300 秒），切换稳定后再调回去，否则回滚要等旧记录过期。
