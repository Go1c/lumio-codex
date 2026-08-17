# 0008 · 规范账号主机是 bestcodex.app，遗留 lumiogame.com 回跳并用 hash 一次性交接会话

- 日期：2026-08-17
- 状态：生效

## 背景

线上同时挂着两份门户：`bestcodex.app` 按路径把 `/login` `/account` 指到门户产物（与产品站共享 `.bestcodex.app` Cookie），`lumiogame.com` 仍是完整门户部署。`cookieDomainFor` 只认 `bestcodex.app`，在 `lumiogame.com` 上只能写 host-only Cookie。同一浏览器里会出现「一边已登录、一边要再登」——见 B-00011。

跨注册域无法靠父域 Cookie 打通。Sub2API 的 refresh 是轮换式，把同一 refresh 复制到两个主机长期并存会互相作废。

## 决策

1. **规范账号 origin** 是 `siteUrl("portal")`（默认 `https://bestcodex.app`）。产品站顶栏账号入口继续指向这里。
2. **遗留官方主机**（`lumiogame.com` / `www.lumiogame.com`）上的门户整页搬到规范 origin，保留 path / query。本地 `localhost` 不回跳，避免打乱双端口联调。
3. **跨官方入口跳转**若本地已有会话，用 URL **片段**带 `lumio_at` / `lumio_rt` / `lumio_exp`；只写到官方主机，落地 `replaceState` 抹掉。不把 refresh 放进查询串，不交给外站。
4. **运维应对 `lumiogame.com` 做 301**（保留路径）。前端回跳是 DNS 未切时的兜底，不是长期双活 SSO。
5. 不引入 Sub2API 新端点；充值进 LumioAPI 的 `/auth/bridge` 仍按上游文档另做。

## 后果

- 用户从书签打开 `lumiogame.com` 且该主机没有 Cookie 时，会被送到 `bestcodex.app` 对应路径；若规范主机已登录，看起来就像「会话跟上了」。
- 在遗留主机上登录后的下一次跳转会把令牌交到规范主机；之后应只在规范主机续期。
- 地址栏可能短暂出现片段令牌；历史记录依赖立刻抹掉。真正的 301 比这段前端更干净。
