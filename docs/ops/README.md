# Lumio Codex 运维与发布指引（总览）

本目录是**正式上线推送**的操作手册。写代码前看 `.spec/`；**部署、打包、发版、维护后台**看这里。

| 文档 | 用途 |
|------|------|
| [01-local-build.md](./01-local-build.md) | 本机环境、编译、跑测、打内部安装包 |
| [02-website-deploy.md](./02-website-deploy.md) | 官网 `site/` → `lumio.games` |
| [03-release.md](./03-release.md) | 版本号、GitHub Release、更新提醒、签名门槛 |
| [04-backend.md](./04-backend.md) | 后台 `api.lumio.games`（Sub2API）与桌面端契约 |
| [05-maintenance.md](./05-maintenance.md) | 日常维护清单、文档如何保持同步 |

## 线上拓扑（当前）

```text
用户浏览器 ──► https://lumio.games          （本仓 site/ 静态站）
用户桌面 App ──► https://api.lumio.games/   （Sub2API，独立仓库）
用户桌面 App ──► 本机官方 Codex 应用
更新提醒     ──► GitHub Releases (Go1c/lumio-codex)
下载制品     ──► GitHub Actions artifact / Releases
```

| 组件 | 仓库 / 路径 | 生产域名 |
|------|-------------|----------|
| 桌面客户端 | 本仓 `apps/codex-plus-manager` | 安装包分发，无独立域名 |
| 官网 | 本仓 `site/` | `https://lumio.games` |
| API / 账户 / 计费后台 | [`Go1c/sub2api`](https://github.com/Go1c/sub2api)（或你们的 Sub2API 部署仓） | `https://api.lumio.games` |
| 支付页 | 后台同源或官网反代 | App 打开 `https://lumio.games/payment` |

## 发布通道（务必分清）

1. **内部未签名包（当前可走）**  
   推 `publish` 或跑 workflow `Internal unsigned build artifacts`，产物带 `-internal-unsigned`，保留 14 天。用于受控内测。

2. **公开正式包（签名门槛未开）**  
   workflow `Public release gate` 会故意失败，直到 Apple Developer ID + 公证、Windows 代码签名、受保护 CI 凭据、S3 更新源与回滚演练齐备。**未满足前不要宣称「正式公开安装包已上线」。**

3. **更新提醒（已接通）**  
   客户端 `lumio_check_update` 读 `https://api.github.com/repos/Go1c/lumio-codex/releases/latest`。只有打出 GitHub Release（带 semver tag）后，首页才会提示新版本。

## 推荐上线顺序

1. 后台 API 在 `api.lumio.games` 健康（见 [04](./04-backend.md)）  
2. 官网部署到 `lumio.games`，并确认 `/payment` 可达（见 [02](./02-website-deploy.md)）  
3. 本机或 CI 打出四平台内部包并做冒烟（见 [01](./01-local-build.md)）  
4. 在 GitHub 创建 Release / Tag，验证 App 更新提醒（见 [03](./03-release.md)）  
5. 按 [05](./05-maintenance.md) 进入日常维护节奏  

## 秘密与合规

- 生产密钥、签名证书、S3、数据库密码、`.env` **永不进本仓库**。  
- 许可证：`AGPL-3.0-only`。公开分发修改版须提供对应源码。  
- 品牌与上游声明见根目录 [README.md](../../README.md)、[THIRD_PARTY_NOTICES.md](../../THIRD_PARTY_NOTICES.md)。
