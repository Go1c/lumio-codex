---
name: ops-release
description: 部署/打包/发版/后台维护入口——准备上线推送、打安装包、发 Release 或改运维流程时查
metadata:
  type: doc
  status: 已交付
---

# 运维与发布（索引）

权威操作手册在仓库 [`docs/ops/`](../../../docs/ops/README.md)，不在本文件展开步骤。

## 何时查 docs/ops

- 部署官网 `site/` → `lumio.games`
- 本机或 CI 编译、打 `internal-unsigned` 包
- 打 Git tag / GitHub Release、验证更新提醒
- 对接或验收 `api.lumio.games`（Sub2API）
- 日常维护节奏与「改了代码要改哪份运维文档」

## 阅读顺序

1. [docs/ops/README.md](../../../docs/ops/README.md)
2. 01 本地编译 → 02 官网 → 03 发版 → 04 后台 → 05 维护

## 硬事实（防漂移）

- 公开签名发布闸门未开前，只分发 `-internal-unsigned`。
- 生产 API 基址与官网域名以 `lumio/product.rs` 与 `docs/ops` 为准。
- 运维步骤变更必须同步 `docs/ops/`，并更新本索引的 description 若触发场景变了。
