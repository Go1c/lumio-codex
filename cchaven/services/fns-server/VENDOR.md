# Lumio 维护说明

这份目录是 **lumio-codex 仓内独立维护的 fns-server 源**，不是上游的 git submodule / subtree。

- 导入快照：`39fadbfb`（`complete workspace v2 sync lifecycle`）
- 之后的改动只走本 monorepo 的提交，不跟踪 `github.com/haierkeys/fast-note-sync-service`
- 构建入口：`node codex/scripts/sync-components/stage.mjs --build-remote`（`CGO_ENABLED=0 GOOS=linux GOARCH=amd64`）
- 真二进制仍然不入库；这里只放 Go 源
