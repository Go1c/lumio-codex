# ADR-0001：控制面新建 `cchaven-control`，不改造 `fast-note-sync-service`

- 日期：2026-08-12
- 状态：已采纳
- 背景：M1 要求「扩展现有服务或新建 cchaven-control」，二选一。

## 决策

新建 `services/cchaven-control`，与 `fns-workspace` 同仓，复用现有服务的**约定与经验**而非代码。

## 理由

| 维度 | `fast-note-sync-service` 现状 | M1 要求 | 结论 |
| --- | --- | --- | --- |
| 仓库 | 独立仓库（`~/Sites/fast-note-sync-service`），与桌面端、M2/M3/M4 不同源 | M1–M4 需协同交付、共享类型与文案 | 同仓更利于交付 |
| 数据库 | SQLite 默认，GORM AutoMigrate + 按用户分库/分 schema 的租户路由 | PostgreSQL 单库 + 迁移脚本随代码 | 租户路由与控制面模型冲突 |
| 口令哈希 | bcrypt cost 10 | Argon2id | 需替换 |
| OAuth | OAuth **客户端** / OIDC RP / MCP 资源服务器（Stytch） | OAuth **授权服务器**（授权码 + PKCE） | 方向相反，无可复用面 |
| 邮件流 | `pkg/email` 未接线，无注册验证/找回密码 | 完整验证码与找回链路 | 需从零实现 |
| 依赖面 | Bleve 全文检索、S3/OSS/R2、WebDAV、go-git、MCP、Prometheus…… | 控制面不需要 | 引入无关攻击面与构建负担 |

在既有服务上叠加会同时承担「拆掉租户路由 + 换哈希算法 + 反向实现授权服务器」三项改造，成本高于新建。

## 复用什么

- 分层约定：`handler → service → repository → domain`
- 统一响应封装与错误码 + i18n 文案表的做法
- testify + 表驱动测试、handler 用 `httptest` 的测试风格
- 限频中间件与审计的设计思路

## 不复用什么

- GORM / `gorm.io/gen` 代码生成（改为 pgx + 手写 SQL，迁移脚本显式随代码）
- 按用户分库的 DAO 路由
- 任何与 note/vault/MCP 相关的领域代码

## 后果

- 两个服务各自独立部署；若后续需要打通用户体系，`cchaven-control` 作为**唯一身份源**，`fast-note-sync-service` 以 OAuth 资源服务器身份校验其签发的令牌（它已具备 JWT 校验能力，改造面小）。
