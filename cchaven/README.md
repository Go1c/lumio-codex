# CC避风港（CCHaven）/ FNS Workspace

用户自有服务器上跑 Claude Code，本机与远端双向安全同步；账号、订阅与运营后台由
独立控制面提供。对用户可见名称为 **CC避风港 / CCHaven**（域名 `cchaven.cn`）；
仓库与内部 crate 仍使用 `fns-` 前缀。

## 仓库地图

| 路径 | 说明 |
| --- | --- |
| `services/cchaven-control` | 控制面（Go）：账号 / 订阅 / 邀请 / 管理 API |
| `services/fns-server` | 同步服务（Go）：本仓独立维护的副本，BestCodex 远端组件由此编出 |
| `apps/web` | 官网（React） |
| `apps/admin` | 运营后台（React） |
| `apps/desktop` | macOS 桌面 APP（Tauri 2 + Rust） |
| `bins/fns-agent` | 用户服务器上的同步 agent |
| `crates/*` | 协议、同步引擎、文件系统、传输层 |
| `docs/design/` | 产品交互规范（权威，工程侧只读） |
| `docs/ops/` | **正式部署 / 编译打包 / 发版 / 文档维护** |

Workspace sync v2 协议权威参考：`fast-note-sync-service@ba4caa45bb766dc4f1bc983e134d6b272a70cd05`。

## 正式上线请从这里开始

**运维与发布文档入口：** [`docs/ops/README.md`](./docs/ops/README.md)

1. [线上拓扑](./docs/ops/01-architecture.md)  
2. [生产部署](./docs/ops/02-deploy-production.md)  
3. [本地编译与打包](./docs/ops/03-build-package.md)  
4. [版本发布与更新](./docs/ops/04-release.md)  
5. [运维/后台文档如何维护](./docs/ops/05-maintain-docs.md)  

各组件日常开发说明仍在各自 README；**部署步骤以 `docs/ops` 为准**。

## 本地开发（摘要）

```bash
# 控制面
cd services/cchaven-control && cp .env.example .env && make db-up && make run

# 官网 / 后台（默认 MSW mock）
cd apps/web && npm ci && npm run dev
cd apps/admin && npm ci && npm run dev

# 桌面 UI（浏览器 mock）或完整 Tauri
cd apps/desktop && npm ci && npm run dev
# cargo tauri dev

# Rust 质量门禁
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
```

细节与排障见各目录 README 与 `docs/ops/03-build-package.md`。
