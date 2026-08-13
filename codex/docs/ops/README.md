# Lumio Codex 运维与发布指引（总览）

本目录是 **Lumio Codex 桌面客户端** 的上线推送手册。写代码前看 `.spec/`；
**本机编译、打包、发版、后台契约**看这里。

> **官网不在本目录管**：产品站已迁到 `web/apps/codex`（`codex.lumiogame.com`），
> 三站部署与域名切换是跨产品的事，权威手册是根 [`docs/ops/`](../../../docs/ops/README.md)。
> 本目录的 [02](./02-website-deploy.md) 只剩旧 Pages 站的下线步骤。

| 文档 | 用途 |
|------|------|
| [01-local-build.md](./01-local-build.md) | 本机环境、编译、跑测、打内部安装包 |
| [02-website-deploy.md](./02-website-deploy.md) | 旧官网 `site/` → `lumio.games` 的**下线**与过渡期安排 |
| [03-release.md](./03-release.md) | 版本号、GitHub Release、更新提醒、签名门槛 |
| [04-backend.md](./04-backend.md) | 后台 `api.lumio.games`（Sub2API）与桌面端契约 |
| [05-maintenance.md](./05-maintenance.md) | 日常维护清单、文档如何保持同步 |
| [`docs/ops/`（根）](../../../docs/ops/README.md) | 统一官网三站部署、域名 / DNS 切换、上线验收 |

## 线上拓扑（当前）

```text
用户浏览器  ──► https://codex.lumiogame.com  （web/apps/codex，产品站）
用户浏览器  ──► https://lumiogame.com        （门户：注册 / 登录 / 账户中心）
用户桌面 App ──► https://api.lumio.games/    （Sub2API，独立仓库；地址硬编码，不可变更）
用户桌面 App ──► 本机官方 Codex 应用
更新提醒     ──► GitHub Releases (Go1c/lumio-codex)
下载制品     ──► S3 CDN（latest-internal.json 指针）/ GitHub Releases
```

| 组件 | 仓库 / 路径 | 生产域名 |
|------|-------------|----------|
| 桌面客户端 | 本仓 `codex/apps/codex-plus-manager` | 安装包分发，无独立域名 |
| 产品站 | 本仓 `web/apps/codex` | `https://codex.lumiogame.com` |
| 总门户 / 账号中心 | 本仓 `web/apps/portal` | `https://lumiogame.com` |
| 旧官网（待下线） | 本仓 `codex/site/` | `https://lumio.games` → 301 到产品站 |
| API / 账户 / 计费后台 | [`Go1c/sub2api`](https://github.com/Go1c/sub2api)（或你们的 Sub2API 部署仓） | `https://api.lumio.games` |
| 支付页 | Sub2API 同源 | App 打开 `https://api.lumio.games/purchase` |

桌面端里的官网链接取自 `crates/codex-plus-core/src/lumio/product.rs` 的
`SITE_BASE_URL`（现为 `https://codex.lumiogame.com`）；`API_BASE_URL` 保持
`https://api.lumio.games/` 不变。

## 发布通道（务必分清）

1. **内部未签名包（当前可走）**  
   推 `publish` 或跑 workflow `Internal unsigned build artifacts`，产物带 `-internal-unsigned`，保留 14 天。用于受控内测。

2. **公开正式包（签名门槛未开）**  
   workflow `Public release gate` 会故意失败，直到 Apple Developer ID + 公证、Windows 代码签名、受保护 CI 凭据、S3 更新源与回滚演练齐备。**未满足前不要宣称「正式公开安装包已上线」。**

3. **更新提醒（已接通）**  
   客户端 `lumio_check_update` 读 `https://api.github.com/repos/Go1c/lumio-codex/releases/latest`。只有打出 GitHub Release（带 semver tag）后，首页才会提示新版本。

## 推荐上线顺序

1. 后台 API 在 `api.lumio.games` 健康（见 [04](./04-backend.md)）  
2. 产品站部署到 `codex.lumiogame.com`；充值页确认 `https://api.lumio.games/purchase` 可达（见根 [`docs/ops/01`](../../../docs/ops/01-web-sites-deploy.md)、[04](./04-backend.md)）  
3. 本机或 CI 打出四平台内部包并做冒烟（见 [01](./01-local-build.md)）  
4. 在 GitHub 创建 Release / Tag，验证 App 更新提醒（见 [03](./03-release.md)）  
5. 把发布指针复制到产品站的 `/latest-internal.json`（见根 [`docs/ops/01`](../../../docs/ops/01-web-sites-deploy.md) §4）  
6. 旧 `lumio.games` 站按 [02](./02-website-deploy.md) 下线并配 301  
7. 按 [05](./05-maintenance.md) 进入日常维护节奏  

## 秘密与合规

- 生产密钥、签名证书、S3、数据库密码、`.env` **永不进本仓库**。  
- 许可证：`AGPL-3.0-only`。公开分发修改版须提供对应源码。  
- 品牌与上游声明见根目录 [README.md](../../README.md)、[THIRD_PARTY_NOTICES.md](../../THIRD_PARTY_NOTICES.md)。
