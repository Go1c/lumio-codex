# 0014 · fns-server 源落仓内独立维护，不再绑定外部 git

- 日期：2026-08-20
- 状态：生效

## 背景

ADR-0013 把 fns-server 留在仓外，靠 `FNS_SERVER_GIT_URL` + pin commit 在构建时 clone。
产品要的是：这份 Go 源以后永远在 lumio-codex 里，由本仓单独改，不跟上游
`github.com/haierkeys/fast-note-sync-service` 绑定。

## 决策

**fns-server 源以仓内副本维护：`cchaven/services/fns-server`。不 submodule、不 subtree 跟踪上游，不要求 CI 变量拉外部仓。**

- 导入快照是联调 HEAD `39fadbfb`（记录在 `VENDOR.md` / `.imported-from-commit` / `fns-server.pin.json`）。之后只走本仓提交。
- `stage.mjs --build-remote` 默认编这份目录；`FNS_SERVER_SOURCE_DIR` 仅作覆盖，不是主路径。
- 真二进制仍不入库（0013 其余条款仍有效：占位由 `build.rs` 生成、`verify.mjs` 把关、dev 宽松 / 发布严格）。
- 被取代：0013 的「源留仓外 + CI clone」；0013 的「组件是构建产物、不入库」不推翻。

## 后果

- CI 不再依赖 `FNS_SERVER_GIT_URL`；ubuntu job 检出本仓即可编远端组件。
- 仓内多一份约 30MB 的 Go 源（含 embed 用的 frontend / docs）。
- 与上游协议分叉后，兼容责任在本仓。
