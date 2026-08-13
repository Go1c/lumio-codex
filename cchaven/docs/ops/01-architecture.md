# 01 · 线上拓扑与组件边界

## 账号体系：唯一真源是 Sub2API

**终端用户的邮箱、口令、账号状态只存在于 Lumio 账号中心（Sub2API，`api.lumio.games`）。**
控制面 `cchaven-control` 不再自持终端用户账号，它保管的只有 CC 专属业务：
订阅与权益、邀请与归因、设备与心跳、订单历史、运营后台。

由此产生的边界：

| 关注点 | 归谁 |
| --- | --- |
| 注册 / 登录 / 找回密码 / 改邮箱改口令 | Sub2API（门户 `lumiogame.com` 上的页面） |
| 终端用户身份校验 | 控制面拿请求里的 `Authorization: Bearer <Sub2API access token>` 调 `GET {sub2api}/api/v1/auth/me` |
| CC 会话（桌面端 access / refresh token） | 控制面仍是 issuer，授权时的身份由 Sub2API 令牌决定 |
| 充值 | `https://api.lumio.games/purchase`（与 Lumio Codex 共用） |
| **管理员账号 + 强制 TOTP** | **仍然完全在控制面本地，未改动** |

要点：

- 自有终端用户认证端点（`/api/v1/auth/register|login|verify-email|password/*|refresh`、
  `/api/v1/me/password|email-change*`）**保留路由但返回 410 `auth_migrated`**，
  `details.portal_url` 指向账号中心，便于存量客户端解释与引导。
- 身份映射表 `sub2api_identities`（`sub2api_user_id ↔ users.id`）是业务侧的身份主键；
  `users` 原有列与历史数据一并保留，`users.sub2api_user_id` 是便于查询的冗余列。
  存量用户的映射由 `cmd/migrate-identities` 一次性补齐（**高风险，需单独确认后执行**）。
- Sub2API 不可达时控制面返回 **503 `identity_unavailable`**，不静默放行、也不伪装成 401。
  校验结果带短 TTL 缓存（`CCHAVEN_SUB2API_CACHE_TTL`，默认 1 分钟）。

## 推荐域名

产品站与门户挂在 `lumiogame.com` 下，控制面与 CC 产品站同站（cookie 仍可用 `SameSite=Lax`）：

| 主机 | 用途 | 仓库产物 |
| --- | --- | --- |
| `https://lumiogame.com` | 统一门户 / 账号中心 / 桌面 APP 授权确认页 | `web/`（另行部署，对接 Sub2API） |
| `https://cc.lumiogame.com` | CC 产品站（营销 / 下载 / 账户页） | `web/`（旧 `apps/web` 逐步退役） |
| `https://api.cc.lumiogame.com` | 控制面 HTTP API | `services/cchaven-control` → `bin/control` |
| `https://admin.cchaven.cn` | 运营后台（独立入口，与官网隔离） | `apps/admin` → `dist/` |
| `https://api.lumio.games` | Sub2API：账号真源 + 充值页 | 本仓库之外 |

> 迁移期 `cchaven.cn` 的三个主机应保留 301 到对应新域名，存量书签与安装包才不会断。
> 新域名的 DNS / 证书需要运维预先配好——桌面端的默认值已经指向它们。

桌面 APP 不托管在域名上：用户从产品站下载页获取安装包；运行时访问
`CCHAVEN_API_BASE`（默认 `https://api.cc.lumiogame.com`）、`CCHAVEN_WEB_BASE`
（默认 `https://cc.lumiogame.com`）与 `CCHAVEN_PORTAL_BASE`
（默认 `https://lumiogame.com`，用于打开 `/authorize`）。

```
                    ┌──────────────────┐      ┌──────────────────┐
   浏览器 ─────────►│  lumiogame.com   │─────►│    Sub2API       │
                    │ 门户 / 账号中心  │      │ api.lumio.games  │
                    └────────┬─────────┘      │ 账号真源 + 充值  │
                             │                └────────▲─────────┘
                    ┌────────▼─────────┐               │ 令牌校验
   浏览器 ─────────►│ cc.lumiogame.com │  静态          │ /auth/me
                    └────────┬─────────┘               │
                             │ /api/* 反代             │
                             ▼                         │
                    ┌──────────────────┐               │
   浏览器 ─────────►│admin.cchaven.cn  │  静态（admin）│
                    └────────┬─────────┘               │
                             │ /api/* 反代             │
                             ▼                         │
   桌面 APP ───────►┌──────────────────┐───────────────┘
                    │api.cc.lumiogame  │     ┌────────────┐
                    │     control      │────►│ PostgreSQL │
                    └──────────────────┘     └────────────┘
                             │
                             ▼（可选）SMTP
```

## 为什么官网与后台必须拆开

交互设计第 7 章要求后台独立入口。控制面用 `CCHAVEN_PUBLIC_URL` +
`CCHAVEN_ADMIN_URL` + `CCHAVEN_PORTAL_URL` 组成可信来源（CORS + CSRF 同源校验共用同一列表）。
漏配 `CCHAVEN_ADMIN_URL` 时：生产环境后台拿不到 CORS，写操作被 403——本地
dev 放行 localhost，**测不出来**。漏配 `CCHAVEN_PORTAL_URL` 同理会让账号中心读不到
CC 的权益与邀请数据。详见控制面 README「部署前必须想清楚的两件事」。

## 网关约定

CC 产品站与后台都通过**同源 `/api`** 访问控制面，避免跨站 cookie 问题：

| 前端 | 浏览器请求 | 网关转到 |
| --- | --- | --- |
| CC 产品站 | `https://cc.lumiogame.com/api/v1/...` | `https://api.cc.lumiogame.com/api/v1/...` |
| 后台 | `https://admin.cchaven.cn/api/admin/v1/...` | `https://api.cc.lumiogame.com/api/admin/v1/...` |

构建前端时：

- CC 产品站：`VITE_API_BASE_URL` **留空**（默认同源 `/api/v1`）
- 后台：生产同样走同源 `/api`；开发才用 Vite proxy + `VITE_API_ORIGIN`

门户 `lumiogame.com` 是跨源访问控制面的（带 Sub2API 令牌，不依赖 cookie），
因此它必须在 `CCHAVEN_PORTAL_URL` 里登记。桌面 APP 没有「同源」概念，
直接打 `https://api.cc.lumiogame.com`。

## 数据与秘密边界

| 数据 | 位置 | 说明 |
| --- | --- | --- |
| **终端用户邮箱 / 口令 / 账号状态** | **Sub2API（本仓库之外）** | 控制面只存映射，不存口令 |
| 身份映射 `sub2api_identities` | PostgreSQL（控制面） | `sub2api_user_id ↔ users.id` |
| 影子账号 / 订阅 / 订单 / 审计 | PostgreSQL（控制面） | 迁移随二进制自动执行 |
| 管理员账号 + TOTP 种子 | 同库，独立表；无自助注册 | `admin-bootstrap` 创建，未受身份收口影响 |
| Sub2API 管理员令牌（仅迁移脚本用） | 运维手上的环境变量 | 不入库、不进 argv、不打日志 |
| OAuth refresh（桌面） | macOS 钥匙串 `cn.cchaven.desktop` | 不落盘 |
| SSH 密码（桌面） | 同上 | ASKPASS + unix socket |
| agent token | 用户服务器上 `0600` 文件 | 永不进 git |

`users.password_hash` 仍然存在（历史数据不删），但收口后新建的影子账号写入的是一个
不合法的 argon2 占位值，永远匹配不上任何口令——本地登录已无入口。

## 同步链路（与控制面正交）

控制面只负责影子账号与订阅。双向文件同步走：

1. 桌面进程内 `fns-sync-core` + 会话监督器（已接通）
2. 用户云主机上的 `fns-agent`（loopback `workspace-sync-v2`）

agent 的交叉编译见 [03-build-package.md](./03-build-package.md)；向导自动上传
仍属缺口（`apps/desktop/docs/spec-gaps.md` B2），上线桌面时可先提供手动安装说明。
