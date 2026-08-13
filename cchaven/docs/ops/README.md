# CC避风港 · 运维与发布文档

本目录是**正式上线、本地编译打包、版本发布、文档维护**的唯一入口。
各组件 README 只保留开发说明；部署相关一律指向这里，避免两处说法打架。

| 文档 | 用途 |
| --- | --- |
| [01-architecture.md](./01-architecture.md) | 线上拓扑、域名、组件边界 |
| [02-deploy-production.md](./02-deploy-production.md) | 生产环境首次部署与日常运维 |
| [03-build-package.md](./03-build-package.md) | 本地编译、产物、桌面/agent 打包 |
| [04-release.md](./04-release.md) | 发版清单、版本号、回滚、CI |
| [05-maintain-docs.md](./05-maintain-docs.md) | 后台/运维文档怎么持续更新 |

## 建议阅读顺序（首次上线）

1. 读完 [架构](./01-architecture.md)，确认域名与可信来源。
2. 按 [生产部署](./02-deploy-production.md) 起 PostgreSQL → 控制面 → 网关 → 官网 → 管理后台 → 首个管理员。
3. 用 [编译打包](./03-build-package.md) 在本机或 CI 打出可发布产物。
4. 按 [发版流程](./04-release.md) 走第一次正式推送。
5. 把 [文档维护](./05-maintain-docs.md) 订进团队习惯，避免文档落后于代码。

## 当前仓库状态（写文档时的事实）

| 组件 | 路径 | 生产就绪度 |
| --- | --- | --- |
| 控制面 API | `services/cchaven-control` | 可部署；需自备进程管理与反向代理（仓库内尚无 Dockerfile） |
| 官网 | `apps/web` | 静态站点，`npm run build` → `dist/` |
| 管理后台 | `apps/admin` | 静态站点，`npm run build` → `dist/` |
| 桌面 APP | `apps/desktop` | Tauri 2 macOS；需本机打 DMG/APP |
| Linux agent | `bins/fns-agent` | 可交叉编译；**尚未接入桌面向导上传/分发**（见 `apps/desktop/docs/spec-gaps.md` B2） |

权威产品规范仍是 [`docs/design/interaction-design.md`](../design/interaction-design.md)（只读，勿在运维文档里复制整章）。
