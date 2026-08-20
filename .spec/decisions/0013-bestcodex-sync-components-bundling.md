# 0013 · 同步组件为构建产物不入库，占位由 build.rs 生成，fns-server 源留仓外 pin

- 日期：2026-08-19
- 状态：生效

## 背景

BestCodex 桌面 Claude 四步 sheet 要「装组件」（把 Linux x86_64 远端对拷到服务器）再「首次同步」（本机 sidecar）。仓库里曾经是占位文件，打包脚本也不带这两套产物，安装包是空壳；文档与排障只能叫人手工 stage。用户装好 App 后组件应已在包内，不应再把「请先 stage」写成安装或开发主路径。

fns-server 源在仓外（`github.com/haierkeys/fast-note-sync-service`），本机联调 commit 是 `39fadbfb`。把它 subtree 进 monorepo 会占体积、双份维护；不解决源可达则发布打不出带远端组件的包。

## 决策

**同步组件是构建产物，不入库。占位由 `src-tauri/build.rs` 在缺失时生成。fns-server 源留在仓外，构建时 copy / deploy，不 subtree。**

- 两套产物分开：本机 sidecar（宿主 triple）与远端 `linux-x86_64` 对；真文件由 `codex/scripts/sync-components/stage.mjs` 覆盖 `src-tauri/binaries/` 与 `src-tauri/resources/remote/`，gitignore 挡住，git 永远干净。
- 占位不足 1024 字节，运行时 `is_real_artifact` / `is_real_sidecar` 拒绝；`cargo test` 仍能解析 `externalBin`。
- fns-server 用仓内 `codex/scripts/sync-components/fns-server.pin.json` 钉 commit `39fadbfb`。CI 用仓库变量 `FNS_SERVER_GIT_URL` 拉源（**变量待配**：用户尚未提供具体 URL；未配则 sync-components job 失败，不打空壳包）。不把该源 subtree 进本仓。
- **dev 宽松 / 发布严格**：`npm run dev` / `tauri` 的 before*Command 走 `stage.mjs --dev`——sidecar 必须编出（仓内 Rust 源），远端缺了告警、第三步诚实失败，避免外部源堵死无关开发。打内部包与 CI 走严格暂存 + `verify.mjs`（魔数 / 体积 / provenance），缺组件必须构建失败。
- `BESTCODEX_CLAUDE_REMOTE_DIR` / `BESTCODEX_CLAUDE_SIDECAR` 只保留排障，不是安装步骤。
- 被取代关系：无（新增）。

## 后果

- 干净克隆没有真二进制；开发机缺远端组件时「装组件」会诚实失败，补齐要本机已有 Go 与仓外源（不擅装）。
- `FNS_SERVER_GIT_URL` 未配齐之前，带 Task 7 门闸的 PR 构建会红——这是设计，不是漏做。
- ubuntu-latest 编出的 Linux sidecar 动态链接较新 glibc；旧发行版兼容（musl）另开决策。
- 打包脚本与产物文件名（`LumioCodex-*`）不变；`tauri.conf.json` 的 `bundle.active` 保持 false。
