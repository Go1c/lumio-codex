# 01 · 线上拓扑与组件边界

## 推荐域名（同站部署）

控制面与两套前端共用 eTLD+1 `cchaven.cn`，cookie 用 `SameSite=Lax`（默认，最安全）：

| 主机 | 用途 | 仓库产物 |
| --- | --- | --- |
| `https://cchaven.cn` | 官网（营销 / 注册登录 / 账户 / APP 授权） | `apps/web` → `dist/` |
| `https://admin.cchaven.cn` | 运营后台（独立入口，与官网隔离） | `apps/admin` → `dist/` |
| `https://api.cchaven.cn` | 控制面 HTTP API | `services/cchaven-control` → `bin/control` |

桌面 APP 不托管在域名上：用户从官网下载页获取安装包；运行时访问
`CCHAVEN_API_BASE`（指向 `https://api.cchaven.cn`）与 `CCHAVEN_WEB_BASE`
（指向 `https://cchaven.cn`，用于打开 `/authorize`）。

```
                    ┌─────────────┐
   浏览器 ─────────►│  cchaven.cn │  静态（web）
                    └──────┬──────┘
                           │ /api/* 反代
                           ▼
                    ┌─────────────┐
   浏览器 ─────────►│admin.cchaven│  静态（admin）
                    └──────┬──────┘
                           │ /api/* 反代
                           ▼
   桌面 APP ───────►┌─────────────┐     ┌────────────┐
                    │api.cchaven  │────►│ PostgreSQL │
                    │  control    │     └────────────┘
                    └─────────────┘
                           │
                           ▼（可选）SMTP
```

## 为什么官网与后台必须拆开

交互设计第 7 章要求后台独立入口。控制面用 `CCHAVEN_PUBLIC_URL` +
`CCHAVEN_ADMIN_URL` 组成可信来源（CORS + CSRF 同源校验共用同一列表）。
漏配 `CCHAVEN_ADMIN_URL` 时：生产环境后台拿不到 CORS，写操作被 403——本地
dev 放行 localhost，**测不出来**。详见控制面 README「部署前必须想清楚的两件事」。

## 网关约定

两套前端都通过**同源 `/api`** 访问控制面，避免跨站 cookie 问题：

| 前端 | 浏览器请求 | 网关转到 |
| --- | --- | --- |
| 官网 | `https://cchaven.cn/api/v1/...` | `https://api.cchaven.cn/api/v1/...` |
| 后台 | `https://admin.cchaven.cn/api/admin/v1/...` | `https://api.cchaven.cn/api/admin/v1/...` |

构建前端时：

- 官网：`VITE_API_BASE_URL` **留空**（默认同源 `/api/v1`）
- 后台：生产同样走同源 `/api`；开发才用 Vite proxy + `VITE_API_ORIGIN`

桌面 APP 没有「同源」概念，直接打 `https://api.cchaven.cn`。

## 数据与秘密边界

| 数据 | 位置 | 说明 |
| --- | --- | --- |
| 用户 / 订阅 / 订单 / 审计 | PostgreSQL（控制面） | 迁移随二进制自动执行 |
| 管理员账号 | 同库，独立表；无自助注册 | `admin-bootstrap` 创建 |
| OAuth refresh（桌面） | macOS 钥匙串 `cn.cchaven.desktop` | 不落盘 |
| SSH 密码（桌面） | 同上 | ASKPASS + unix socket |
| agent token | 用户服务器上 `0600` 文件 | 永不进 git |

## 同步链路（与控制面正交）

控制面只负责账号与订阅。双向文件同步走：

1. 桌面进程内 `fns-sync-core` + 会话监督器（已接通）
2. 用户云主机上的 `fns-agent`（loopback `workspace-sync-v2`）

agent 的交叉编译见 [03-build-package.md](./03-build-package.md)；向导自动上传
仍属缺口（`apps/desktop/docs/spec-gaps.md` B2），上线桌面时可先提供手动安装说明。
