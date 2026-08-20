# FNS Workspace Sync v2 Protocol and v1 Compatibility Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use subagent-driven-development to implement this plan task-by-task (hosts without subagents: its Inline Fallback section). Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 `fast-note-sync-service` 中并行锁住现有 `/api/user/sync` 的鉴权、`Action|JSON`、`NoteSync` 与 `FileSync` 行为，并以可被 Go Task 2 和 Rust Task 4 直接消费的 DTO、协议文档和固定 fixtures 定义 workspace sync v2。

**Architecture:** Wave 1 在两个文件集互斥的 worktree 并行：Task 1A 只增加 v1 characterization tests，Task 1B 只增加 v2 DTO/tests/fixtures/doc。v1 tests 只从外部观察当前 WebSocket 行为，不抽取、不重写任何 v1 生产逻辑；v2 使用独立的 `Action|JSON` 控制协议、强类型值对象和固定 64-byte 二进制 header。`docs/workspace-sync-v2.md` 是 wire contract 权威，根级 `testdata/workspace-sync-v2/` 是 Go/Rust 兼容向量权威。

**Tech Stack:** Go 1.26.2、`github.com/lxzan/gws`、`github.com/google/uuid`、标准库 `encoding/json`/`encoding/binary`、现有 `pkg/json`、Testify。

## Global Constraints

- 整个计划只能触碰 Exact File Map；Task 1A 只新增其中 3 个 test files，Task 2 允许修改 `go.mod`/`go.sum` 并新增其余 v2 files。不得修改任何 v1 production file、现有 v1 DTO 或 `docs/ws_api.md` 的行为。
- v1 endpoint 保持 `GET /api/user/sync`；v1 文本帧继续只在第一个 `|` 分隔 action/data；v1 鉴权继续是 `Authorization|<JWT>`；v1 二进制 `00` frame 保持不变。
- v2 endpoint 由 Task 3 注册为 `GET /api/user/workspace-sync/v2`；Task 1 不提前注册路由或 handler。
- v2 HTTP upgrade 必须带 `Authorization: Bearer <token>`，复用现有 token scope/client/IP/UA 校验；v2 action 集不包含 v1 的 `Authorization` 和 `ClientInfo`。
- 所有 v2 revision 在 JSON 中是 canonical 十进制字符串；所有内容 hash 是 `blake3:<64 lowercase hex>`。
- Task 2 只允许新增 pure-Go `github.com/zeebo/blake3 v0.2.4`，并同步修改 `go.mod`/`go.sum`；除此之外不新增 Go module dependency。
- 除 Exact File Map 外零改动；不重构现有 WebSocket server、router、handler、DTO、mock 或 error-code 基础设施。
- 每个 v2 生产类型或校验函数必须先有能以预期原因失败的测试；v1 baseline 是对已存在行为的 characterization 例外，应在未改生产代码时直接通过，不得为了制造 RED 改坏断言或生产行为。

## Exact File Map

- Create: `pkg/app/websocket_v1_regression_test.go` — 真实 `gws` client/server 固化 v1 首个 `|` 分隔、未鉴权拒绝、有效/无效 `Authorization` 行为。
- Create: `internal/routers/router_websocket_test.go` — 通过 production router table 固化 `GET /api/user/sync` 仍被注册。
- Create: `internal/routers/websocket_router/ws_v1_regression_test.go` — 现有 handlers、service mocks 和真实 WebSocket 帧固化 `NoteSync`/`FileSync` envelope、计数和消息顺序。
- Modify: `go.mod` — Task 2 增加 `github.com/zeebo/blake3 v0.2.4`。
- Modify: `go.sum` — 记录上述模块的校验和。
- Create: `internal/dto/workspace_v2_dto_ws.go` — Task 2/3 消费的 primitives、固定 15-action DTO、envelope、校验和 binary header codec。
- Create: `internal/dto/workspace_v2_dto_ws_test.go` — JSON round-trip、action registry、cross-field validation、binary header tests。
- Create: `internal/dto/workspace_v2_fixtures_test.go` — 加载根级 fixtures，验证 action/type 映射、canonical JSON、非法向量与 manifest。
- Create: `testdata/workspace-sync-v2/manifest.json`
- Create: `testdata/workspace-sync-v2/valid/control-frames.jsonl`
- Create: `testdata/workspace-sync-v2/valid/error-envelopes.jsonl`
- Create: `testdata/workspace-sync-v2/invalid/revisions.jsonl`
- Create: `testdata/workspace-sync-v2/invalid/hashes.jsonl`
- Create: `testdata/workspace-sync-v2/invalid/paths.jsonl`
- Create: `testdata/workspace-sync-v2/binary/header-vectors.json`
- Create: `docs/workspace-sync-v2.md` — endpoint、auth、actions、validation、ordering、limits、binary framing 和 fixture governance 的单一权威。

## Wave 1 Parallel Worktree Boundaries

- **Task 1A worktree（v1 baseline arm）只拥有：** `pkg/app/websocket_v1_regression_test.go`、`internal/routers/router_websocket_test.go`、`internal/routers/websocket_router/ws_v1_regression_test.go`。它不得创建或修改 v2 DTO、fixtures、协议文档或任何 v1 production file。
- **Task 1B worktree（v2 contract arm）只拥有：** `go.mod`、`go.sum`、`internal/dto/workspace_v2_dto_ws.go`、`internal/dto/workspace_v2_dto_ws_test.go`、`internal/dto/workspace_v2_fixtures_test.go`、`testdata/workspace-sync-v2/**`、`docs/workspace-sync-v2.md`。它不得修改 Task 1A tests 或任何 v1 production file。
- 两个 worktree 从同一 clean commit 建立并同时执行。1A 的 characterization tests 与 1B 的 v2 TDD 不存在代码依赖，因此不要求 1A 人为先于 1B 完成；两个 arm 各自 review/commit 后由主 loop 合入同一 integration worktree。
- integration worktree 只合入两个已审 arm，不手工重写其文件。合入后先运行本机 arm64 Mac/amd64 Go 的 `GOMAXPROCS=1 go test -p 1 ./...`，CI/native amd64 环境仍运行 `go test ./...` 证明默认并行配置可用。

## Locked Wire Contract

### Framing and envelopes

文本控制帧是 UTF-8 `Action|JSON`，只按第一个 ASCII `|` 分割。action 必须是下文精确值且最长 64 bytes。完整 text frame（action、分隔符、JSON）最大 65,536 bytes。JSON 必须是单个 object；拒绝 trailing data、重复 key、未知字段、NaN/Infinity 和 number 形式 revision。

客户端请求：

```json
{"requestId":"10000000-0000-4000-8000-000000000001","data":{}}
```

服务端成功响应：

```json
{"requestId":"10000000-0000-4000-8000-000000000001","status":true,"data":{}}
```

服务端 push：

```json
{"status":true,"data":{}}
```

失败：

```json
{"requestId":"10000000-0000-4000-8000-000000000001","status":false,"error":{"code":"invalid_path","message":"path must be a canonical workspace-relative POSIX path","retryable":false,"fields":[{"field":"data.path","reason":"parent_segment"}]}}
```

`status=true` 时必须有 `data` 且没有 `error`；`status=false` 时必须有 `error` 且没有 `data`。`requestId` 是 lowercase canonical UUID，正常响应必须原样回显，push 不带；只有 JSON 无法解出 requestId 的 `invalid_json`/`invalid_request` 失败可以省略它。错误 message 是稳定英文安全句子，不得包含 token、用户文件内容或 workspace 之外的绝对路径；机器逻辑只读 code。

请求级失败沿用收到的 action token 回包：已知 action 和 unknown action 都返回 `<received-action>|<failure-envelope>`，unknown token 只允许 ASCII `[A-Za-z][A-Za-z0-9]{0,63}`，仅作一次错误回显，不注册 handler，因此不增加第 16 个 action。缺少 `|`、action token 非法、非 UTF-8、text frame 超限时无法安全回显 envelope：分别用 WebSocket close 1002（protocol error）或 1009（message too big）；HTTP Bearer 失败在 upgrade 前返回 401/403。服务端 push 本身不存在“失败响应”。

稳定 error code 完整集合：

`invalid_frame`、`invalid_json`、`unknown_action`、`unauthenticated`、`forbidden`、`invalid_request`、`invalid_revision`、`invalid_hash`、`invalid_path`、`workspace_not_found`、`workspace_limit_exceeded`、`client_not_registered`、`stale_base_revision`、`operation_reused`、`blob_required`、`blob_not_found`、`blob_hash_mismatch`、`blob_size_mismatch`、`blob_transfer_out_of_order`、`blob_limit_exceeded`、`conflict_not_found`、`conflict_revision_stale`、`server_busy`、`internal`。

只有 `server_busy` 和 `internal` 的 `retryable=true`；`blob_required` 明确为 false，其余也均为 false。业务 stale/conflict/blob upload 按规定状态机推进，不以通用 retryable 表达。

### Primitive and cross-field validation

- `WorkspaceRevision`：JSON string 匹配 `0|[1-9][0-9]{0,19}`，数值不超过 `18446744073709551615`。服务端分配从 1 开始；0 只表示从未 ack、路径不存在或 rename 目标不存在。
- `WorkspaceConflictRevision`：独立公开 opaque comparable wrapper，wire 是 canonical positive decimal string `[1-9][0-9]{0,19}` 且不超过 `18446744073709551615`。它只作 conflict generation 的 equality guard，不是 `WorkspaceRevision`；公共 surface 不暴露可排序/算术的数值类型，也不得用于 ordering、Ack、retention floor 或 hub/tree revision key。
- `workspaceId`、`clientId`、`operationId`、`requestId`、`streamId`、`transferId`、`conflictId`：lowercase canonical UUID。`operationId` 在同一 client 内永久唯一。
- `WorkspaceContentHash`：非空值精确匹配 `blake3:[0-9a-f]{64}`。file/symlink/merged 必须非空；directory/delete/tombstone 必须 JSON null，但 key 仍必须存在。
- `WorkspacePath`：NFC-normalized UTF-8、1..4096 bytes、workspace-relative POSIX path。禁止 leading/trailing slash、`//`、空/`.`/`..` segment、backslash、NUL、C0/C1 control、Windows drive/UNC prefix、segment 尾部 space/dot、字符 `< > : " | ? *`、Windows device basename `CON PRN AUX NUL COM1..COM9 LPT1..LPT9`（大小写不敏感，扩展名前 basename 也检查）。DTO 仅做 lexical validation；Task 3 仍须 secure-join 并拒绝根外 symlink。
- `WorkspaceFileMetadata` 固定为 `{"size":uint64,"modifiedAtMs":int64,"executable":bool}`；`size<=5368709120`，timestamp `0..253402300799999`。directory/delete size 必须 0，executable 必须 false。
- `WorkspaceEntryKind`：`file|directory|symlink|tombstone`。`WorkspaceMutationKind`：`upsert_file|mkdir|upsert_symlink|delete|rename`。
- rename 的 `path` 是源路径，必须带 `newPath` 和 `targetBasePathRevision`；其他 mutation 禁止这两个字段。源/目标不得相同，directory 不得移入自身子树。
- upsert file/symlink 必须有 hash；mkdir/delete 必须 hash null。所有 mutation 都显式带 metadata。

### All 15 fixed actions and exact data schemas

action registry 必须与主计划列出的 15 个名字逐字一致，不增加别名。需要双向语义的 blob 下载和 conflict resolve 通过 action direction、request/response/push envelope 选择不同 concrete DTO，不新增 action token。

冻结总数保持 exact 15 actions / 25 legal action-flow mappings；counted conflict phases 复用第 14/15 action，不增加 action 或 flow。二进制 header 仍固定 64 bytes。

1. `WorkspaceHello` C→S request data：`protocolVersion:"2"`、`clientId`、`clientVersion`、`capabilities:["binary_chunks","conflicts","snapshot_v1"]`。S→C response data：`protocolVersion:"2"`、`serverVersion`、`maxControlFrameBytes:65536`、`maxBinaryChunkBytes:1048576`、`maxBlobBytes:5368709120`、`maxTransfersPerConnection:4`、`heartbeatSeconds:25`。
2. `WorkspaceSubscribe` C→S：`workspaceId`、`clientId`、`lastAckRevision`。成功后直接开始第 3 项 stream，不增加 subscribe-ack action。
3. `WorkspaceSnapshotBegin` S→C：`workspaceId`、`streamId`、`mode:"snapshot"|"incremental"`、`fromRevision`、`finalRevision`、`entryCount`、`eventCount`、`conflictCount`。snapshot 时 eventCount=0；incremental 时 entryCount=0；三个 count 都是 required uint32，`conflictCount` 两种 mode 都允许。
4. `WorkspaceSnapshotEntry` S→C：`workspaceId`、`streamId`、`index`（从 0 连续）、`entry`。entry 固定为 `path,pathRevision,kind,contentHash,metadata,tombstone`；tombstone 和 kind 必须一致。
5. `WorkspaceSnapshotEnd` S→C：`workspaceId`、`streamId`、`mode`、`deliveredCount`、`finalRevision`。snapshot 的 deliveredCount 是 checked `entryCount+conflictCount`，incremental 是 checked `eventCount+conflictCount`；uint32 加法溢出或不相等都拒绝。End 后才可 ack finalRevision。
6. `WorkspaceMutation` C→S：`workspaceId`、`clientId`、`operationId`、`path`、`basePathRevision`、`kind`、`contentHash`、`metadata`；rename 另带 `newPath,targetBasePathRevision`。contentHash key 始终存在。
7. `WorkspaceMutationAccepted` S→C：`workspaceId`、`clientId`、`operationId`、`revision`、`pathState`；rename 另带 `oldPathState,newPathState`。重复 operation 返回首次的同一 payload/revision。
8. `WorkspaceMutationRejected` S→C：`workspaceId`、`clientId`、`operationId`、`reason:"stale_base_revision"|"operation_reused"|"blob_required"|"conflict_created"`、`currentPathState`（可 null）、`conflictId`（仅 conflict_created 非 null）、`requiredHash`（仅 blob_required 非 null）。它是 `status:true` 的业务结果；frame/schema 错误才用失败 envelope。
9. `WorkspaceEvent` S→C：`workspaceId`、`streamId`、`index`、`revision`、`operationId`、`originClientId`、`mutation`、`pathState`；rename 另带 `oldPathState,newPathState`。revision 在 incremental revision-item union 中严格递增；index 只在 `WorkspaceEvent` 子序列连续，穿过 `WorkspaceConflictResolved` item 时不消耗 index。
10. `WorkspaceAck` C→S：`workspaceId`、`clientId`、`revision`。revision 只能前进且不得超过已完整收到的 SnapshotEnd finalRevision；成功 data 回显三字段。
11. `WorkspaceBlobNeed` 双向。S→C upload push data：`workspaceId`、`direction:"upload"`、`operationId`（UUID）、`contentHash`、`size`（uint64）。C→S download request data：`workspaceId`、`direction:"download"`、`operationId:null`、`contentHash`、`size:null`；两个 null key 都 required，missing 与 non-null 均拒绝。S→C correlated success response 回显 `workspaceId,direction:"download",operationId:null,contentHash` 并给实际 `size`，随后才发送 BlobBegin。blob 不存在以同 action 的 `blob_not_found` 失败 envelope 返回。
12. `WorkspaceBlobBegin` 双向：`workspaceId`、`transferId`、`direction:"upload"|"download"`、`contentHash`、`size`、`chunkSize:1048576`、`chunkCount`。upload 是 client request，server 同 action success 后才允许 binary；download 是 server push。chunkCount=ceil(size/chunkSize)，zero-byte 为 0。
13. `WorkspaceBlobEnd` 双向，data 固定为 `workspaceId,transferId,direction,contentHash,size,chunkCount`。upload：client 发送 request，server 校验后回 correlated success。download：server 先发送不带 requestId 的 push；client 验证完整 blob 后，以新的 requestId 发送同 action acknowledgement request，`direction:"download"` 且 transfer/hash/size/count 与 push 完全相同；server 再回 correlated success。客户端不得把 server push 当成需要直接 response 的 request。hash/size/order 失败以对应 client request 的同 action 失败 envelope 返回，失败 transfer 不可复用。
14. `WorkspaceConflictCreated` S→C：`workspaceId`、`conflictId`、`conflictRevision`、`path`、`kind:"content"|"delete_modify"|"rename"|"binary"`、`ancestor,current,incoming`、`createdByOperationId`。每个 side 固定为 `path|null,pathRevision,contentHash|null,metadata,tombstone`；DTO 不带 `streamId`/`index`。创建/刷新 conflict 不推进 global/path/tree revision，也不写 revision item；side 的 pathRevision 是显式独立值，不能从 conflictRevision 推导。
15. `WorkspaceConflictResolved` 双向。C→S request data：`workspaceId`、`clientId`、`operationId`、`conflictId`、`conflictRevision`、`choice:"current"|"incoming"|"merged"|"delete"`、`path`、`contentHash`、`metadata`；merged 要非空 hash，delete 要 null/size 0，current/incoming 必须逐字匹配 ConflictCreated side。S→C correlated success response 与 push data：`workspaceId`、`conflictId`、`conflictRevision`、`operationId`、`revision`、`choice`、`pathState`、`resolvedByClientId`；correlated response 带 requestId，其他订阅方的 push 不带。不相等的 conflictRevision 以同 action 的 `conflict_revision_stale` 失败 envelope 返回；stale 只表示 equality mismatch，不表示数值先后。response/push DTO 不带 `streamId`/`index`；成功 resolve 在提交前重验 source drift，rename 还重验 target drift，只推进一次 tree revision 并写一个 tagged `WorkspaceConflictResolved` revision item，绝不合成 mutation/accepted/event。

Merged conflict 的唯一缺 blob 序列：client 发送 `WorkspaceConflictResolved` resolve request（choice=merged，contentHash 非空但 server blob store 尚无该 hash）→ server 先持久化 pending resolve，再以同 `WorkspaceConflictResolved` action 返回 correlated failure envelope（回显 requestId，error.code=`blob_required`、retryable=false），使本次 request 明确终结 → server 随后发送 `WorkspaceBlobNeed(direction=upload)` push，operationId 等于 resolve operationId → client 完成 BlobBegin/binary/BlobEnd upload → client 以新的 requestId 重发与首次逐字段相同的 resolve data（同 operationId、conflictRevision、path、hash、metadata）→ server 原子 resolve。断线/重连后重发同 operation：blob 仍缺则再次返回同 correlated `blob_required` failure 并再次 push BlobNeed；blob 已存在则 resolve；conflict revision 已 stale 则 correlated `conflict_revision_stale` 并删除 pending。第一次与重复缺 blob 均不把 operationId 写成终态 result，因此不得报 `operation_reused`。pending TTL 固定 24 hours；超时只删除 pending，不修改 conflict 或 blob，client 可用新 operationId 重新发起。stale 后已上传但无人引用的 blob 是 orphan，由 Task 2 blob GC 按正常 grace period 回收，不在 resolve request 中同步删除。

### Stream ordering

- 增量：SnapshotBegin(mode=incremental) → 恰好 eventCount 个按 tree revision 严格升序的 ordered revision-item union（每项是 `WorkspaceEvent` 或 server-push `WorkspaceConflictResolved`）→ 恰好 conflictCount 个按 conflictId UTF-8 byte lexicographic 升序的 authoritative `WorkspaceConflictCreated` set → SnapshotEnd → client WorkspaceAck。`WorkspaceEvent.index` 只在 Event 子序列连续，union phase 另计全部 items。
- 全量：SnapshotBegin(mode=snapshot) → 恰好 entryCount 个按 path UTF-8 byte lexicographic 升序的 WorkspaceSnapshotEntry → 恰好 conflictCount 个按 conflictId UTF-8 byte lexicographic 升序的 authoritative `WorkspaceConflictCreated` set → SnapshotEnd → client WorkspaceAck。
- Begin 到 End 是同一 read snapshot；客户端只有在精确校验 phase、count、ordering 与 matching End 并持久化完整结果后，才以该 authoritative set 删除本地缺席 conflict。中断/失败保留旧 set。snapshot deliveredCount 是 checked `entryCount+conflictCount`，incremental 是 checked `eventCount+conflictCount`。
- 一个连接同时一个 subscription stream；旧 stream End 前新 Subscribe 返回 `invalid_request`。Begin/End 之间不得夹同 workspace 的 stream 外 notification；并发 live ConflictCreated/ConflictResolved 必须缓冲到 End 后且不计入 count。blob frame 可按 transferId 与控制流交错。
- Upload：server BlobNeed(direction=upload) push → client BlobBegin request → server BlobBegin success → binary chunks → client BlobEnd request → server BlobEnd success → client 重发原 Mutation → Accepted/Rejected。
- Download：client BlobNeed(direction=download) request → server BlobNeed correlated success → server BlobBegin push → binary chunks → server BlobEnd push → client validates → client BlobEnd acknowledgement request(new requestId, same direction/transfer/hash/size/count) → server BlobEnd correlated success。server BlobEnd push 本身不接收 response。
- Merged resolve missing blob：ConflictResolved client_request → persist pending → ConflictResolved correlated `blob_required` failure → BlobNeed upload server_push(same operationId) → BlobBegin/frames/BlobEnd upload → ConflictResolved client_request(new requestId, byte-equivalent data) → resolved response，或 correlated `conflict_revision_stale` + pending delete。每个 client request 都先得到 correlated terminal response；pending operationId 不进入终态幂等表，24h expiry 仅删 pending。
- 每个 transfer chunkIndex 从 0 连续，offset=chunkIndex*chunkSize；除末块外 payloadLength=chunkSize。相同 transfer 禁止乱序/重复，不同 transfer 可交错。final 只在末块；zero-byte 没 binary frame。

### Binary frame header and limits

固定 64-byte header，整数 big-endian：

| Bytes | Field | Rule |
|---|---|---|
| 0..3 | magic | ASCII `FNS2` |
| 4 | version | `0x02` |
| 5 | direction | `0x01` upload, `0x02` download |
| 6 | flags | bit 0 final; bits 1..7 zero |
| 7 | headerLength | `0x40` |
| 8..23 | transferId | UUID 16 raw bytes |
| 24..31 | chunkIndex | uint64 |
| 32..39 | offset | uint64 |
| 40..43 | payloadLength | uint32, actual frame 必须为 1..1048576 |
| 44..47 | reserved | all zero |
| 48..63 | chunkDigest | first 16 raw BLAKE3(payload) bytes |

frame 总长严格等于 `64+payloadLength`；`payloadLength:0` 的 constructed/received frame 非法，zero-byte blob 只通过 BlobBegin/BlobEnd 的 `chunkCount:0` 表示且不发送 binary frame。单 blob 最大 5,368,709,120 bytes；chunk 固定 1,048,576 bytes（仅末块更小）；每连接 4 transfers、每 workspace 16、每 user 32；60 seconds idle 过期，单 transfer 最长 30 minutes。上限返回 `blob_limit_exceeded`，不驱逐活跃 transfer。完整 blob hash/size/count 校验成功前不得公布临时内容。

## Wave 1 Integration Gate (After Both Worktrees Pass Review)

**Files:** Verify only。

- [ ] Format both arms with `gofmt -w pkg/app/websocket_v1_regression_test.go internal/routers/router_websocket_test.go internal/routers/websocket_router/ws_v1_regression_test.go internal/dto/workspace_v2_dto_ws.go internal/dto/workspace_v2_dto_ws_test.go internal/dto/workspace_v2_fixtures_test.go`.
- [ ] Run focused suites: `GOMAXPROCS=1 go test -p 1 ./pkg/app ./internal/routers ./internal/routers/websocket_router ./internal/dto -count=1`; expected PASS.
- [ ] Run this Mac/Rosetta full suite: `GOMAXPROCS=1 go test -p 1 ./...`; expected PASS.
- [ ] CI/native amd64 still runs `go test ./...`; expected PASS, and `-p 1` must not become a permanent project limit.
- [ ] Run `git diff --check`, inspect `git status --short`, and confirm no diff in protected v1 production files listed under Global Constraints.

## Completion Checklist

- [ ] Task 1/2 file sets are disjoint and independently reviewed before integration.
- [ ] Registry/docs contain exact 15 actions and every declared flow has one concrete factory consumed by fixtures.
- [ ] Present-aware required-null, download BlobEnd acknowledgement, merged pending resolve, BLAKE3 replay, v1 route/binary dispatch, revision/hash/path and error envelopes are covered by the named tests/fixtures.
- [ ] Local tests use `GOMAXPROCS=1 go test -p 1 ...`; CI/native full test remains `go test ./...`.
- [ ] No unrelated refactor/dependency or v1 behavior change is included.

## Task 1: v1 Regression Baseline

**Files:**
- Create: `pkg/app/websocket_v1_regression_test.go`
- Create: `internal/routers/router_websocket_test.go`
- Create: `internal/routers/websocket_router/ws_v1_regression_test.go`

**Produces:** Task 3 必须持续通过的 v1 compatibility gate。

- [ ] **Step 1: Add a test-only real-WebSocket harness**

在 `pkg/app/websocket_v1_regression_test.go` 使用 package `app`。实现只存在于 `_test.go` 的 `AppContainer` double：`SubmitTask`/`SubmitTaskAsync` 同步执行、nop logger、nil validator、`IsReturnSuccess=true`、固定 auth key。用 `httptest.NewServer` 挂 `GET /api/user/sync` → `wss.Run()`，以现有依赖 `gws.NewClient` 连接，text/close frames 进入 buffered channels，每次 receive 上限 5 seconds，避免现有无效鉴权延迟关闭产生竞态。

- [ ] **Step 2: Lock parser and authentication**

新增：

```go
func TestV1TextFrameSplitsOnFirstPipe(t *testing.T)
func TestV1BusinessActionBeforeAuthorizationIsRejected(t *testing.T)
func TestV1AuthorizationAcceptsValidJWT(t *testing.T)
func TestV1AuthorizationRejectsMalformedJWT(t *testing.T)
func TestV1Binary00DispatchesPayloadWithoutPrefix(t *testing.T)
```

第一个注册 test-only authenticated echo，发送 `Echo|{"value":"left|right"}` 并断言完整 JSON suffix。第二个断言未鉴权 business action code 307。valid test 用现有 `NewTokenManager` 生成 JWT，安装 `UseTokenVerify`/`UseUserVerify`，断言 `Authorization|{Res}` code 1/status true/version data。malformed test 断言 code 308 和 close；不锁当前 2-second sleep。binary test 先建立已认证 client、以 `UseBinary("00", handler)` 注册真实 dispatch，发送 binary frame `append([]byte("00"), payload...)`，断言 handler 只收到去掉 prefix 的原始 payload，且 text/protobuf handler 均未调用。

- [ ] **Step 3: Lock actual NoteSync/FileSync handlers**

在 router test 用 `app.NewTestApp`、`MockVaultService`、`MockNoteService`、`MockFileService`，只注册现有 NoteSync/FileSync handlers 和现有 interceptor。test-only setup action 可设置 `c.User`、`c.Scope="*"`、client identity 和 strategy，但不能替代上一文件的真实 auth tests。

新增：

```go
func TestV1NoteSyncSendsEndBeforeQueuedModify(t *testing.T)
func TestV1FileSyncSendsEndBeforeQueuedDelete(t *testing.T)
func TestV1SyncInvalidJSONKeepsErrorEnvelope(t *testing.T)
```

Note fixture：server list 一个 live note、client notes 空；严格断言 frames 是 NoteSyncEnd 后 NoteSyncModify，均为 v1 `Res`，End modify=1 其余 0，vault/context 原样。File fixture：server list 一个 `Action:"delete"` file、client 含相同 pathHash；严格断言 FileSyncEnd 后 FileSyncDelete，delete=1。invalid JSON 对两个 action 都断言 code 305，不能出现 v2 envelope。

- [ ] **Step 4: Lock the production v1 route table**

在 `internal/routers/router_websocket_test.go` 使用 production `registerAPIRoutes` 构建 Gin routes table，新增 `TestRegisterAPIRoutesKeepsV1WebSocketGET`；遍历 `engine.Routes()`，必须找到 `Method == http.MethodGet && Path == "/api/user/sync"`，且同一 method/path 恰好一次。测试不得复制 route 常量或建立 test-only router。

- [ ] **Step 5: Run baseline against the shared starting commit**

```bash
GOMAXPROCS=1 go test -p 1 ./pkg/app ./internal/routers ./internal/routers/websocket_router -run 'TestV1|TestRegisterAPIRoutesKeepsV1WebSocketGET' -count=1
```

Expected: PASS。它们是已有行为的 characterization TDD 例外，不要求先失败；失败表示 harness 未复现 live behavior，只修测试，不改 v1 production。Task 1B 可在另一 worktree 同时进行。

- [ ] **Step 6: Add a follow-up characterization commit**

若原 v1 baseline commit 已存在，保留其历史，不执行 `git commit --amend` 或 rebase；把 route/binary additions 作为 follow-up commit：

```bash
git add pkg/app/websocket_v1_regression_test.go internal/routers/router_websocket_test.go internal/routers/websocket_router/ws_v1_regression_test.go
git commit -m "test(sync): extend v1 websocket compatibility gate"
```

## Task 2: v2 Wire Contract

**Worktree:** B。此 Task 独占 Exact File Map 中全部 v2 文件；下列 Phase 在同一 worktree 内串行执行，均属于同一个可抽取 task brief。

### Phase 2.1: Add Strict v2 Primitives

**Files:**
- Create: `internal/dto/workspace_v2_dto_ws.go`
- Create: `internal/dto/workspace_v2_dto_ws_test.go`

**Produces for Tasks 2/3:** `WorkspaceRevision`、`WorkspaceContentHash`、`WorkspacePath`、`WorkspaceNullableHash`、`WorkspaceNullableUUID`、`WorkspaceNullableUint64`、`WorkspaceFileMetadata`、`WorkspaceEntryKind`、`WorkspaceMutationKind`、`WorkspaceValidationError`、`ParseWorkspaceRevision`、`ParseWorkspaceContentHash`、`ParseWorkspacePath`。

**Required production API and validator implementation:**

```go
type WorkspaceValidationError struct {
	Field  string
	Reason string
}

func (e *WorkspaceValidationError) Error() string {
	return e.Field + ": " + e.Reason
}

type WorkspaceUUID string
type WorkspaceRevision uint64
type WorkspaceContentHash string
type WorkspacePath string

type WorkspaceNullableHash struct {
	Present bool
	Value   *WorkspaceContentHash
}

type WorkspaceNullableUUID struct {
	Present bool
	Value   *WorkspaceUUID
}

type WorkspaceNullableUint64 struct {
	Present bool
	Value   *uint64
}

type WorkspaceFileMetadata struct {
	Size         uint64 `json:"size"`
	ModifiedAtMS int64  `json:"modifiedAtMs"`
	Executable   bool   `json:"executable"`
}

type WorkspaceEntryKind string
const (
	WorkspaceEntryFile      WorkspaceEntryKind = "file"
	WorkspaceEntryDirectory WorkspaceEntryKind = "directory"
	WorkspaceEntrySymlink   WorkspaceEntryKind = "symlink"
	WorkspaceEntryTombstone WorkspaceEntryKind = "tombstone"
)

type WorkspaceMutationKind string
const (
	WorkspaceMutationUpsertFile    WorkspaceMutationKind = "upsert_file"
	WorkspaceMutationMkdir         WorkspaceMutationKind = "mkdir"
	WorkspaceMutationUpsertSymlink WorkspaceMutationKind = "upsert_symlink"
	WorkspaceMutationDelete        WorkspaceMutationKind = "delete"
	WorkspaceMutationRename        WorkspaceMutationKind = "rename"
)

func ParseWorkspaceRevision(s string) (WorkspaceRevision, error) {
	if s == "" {
		return 0, &WorkspaceValidationError{Field: "revision", Reason: "empty"}
	}
	v, err := strconv.ParseUint(s, 10, 64)
	if err != nil || strconv.FormatUint(v, 10) != s {
		return 0, &WorkspaceValidationError{Field: "revision", Reason: "non_canonical_decimal"}
	}
	return WorkspaceRevision(v), nil
}

func (r WorkspaceRevision) MarshalJSON() ([]byte, error) {
	return json.Marshal(strconv.FormatUint(uint64(r), 10))
}

func (r *WorkspaceRevision) UnmarshalJSON(data []byte) error {
	var s string
	if err := json.Unmarshal(data, &s); err != nil {
		return &WorkspaceValidationError{Field: "revision", Reason: "must_be_string"}
	}
	v, err := ParseWorkspaceRevision(s)
	if err != nil {
		return err
	}
	*r = v
	return nil
}

func ParseWorkspaceContentHash(s string) (WorkspaceContentHash, error) {
	if len(s) != len("blake3:")+64 || !strings.HasPrefix(s, "blake3:") {
		return "", &WorkspaceValidationError{Field: "contentHash", Reason: "invalid_blake3"}
	}
	if _, err := hex.DecodeString(s[len("blake3:"):]); err != nil || strings.ToLower(s) != s {
		return "", &WorkspaceValidationError{Field: "contentHash", Reason: "invalid_blake3"}
	}
	return WorkspaceContentHash(s), nil
}

func ParseWorkspacePath(s string) (WorkspacePath, error) {
	fail := func(reason string) (WorkspacePath, error) {
		return "", &WorkspaceValidationError{Field: "path", Reason: reason}
	}
	if s == "" || len([]byte(s)) > 4096 || !utf8.ValidString(s) {
		return fail("invalid_length_or_utf8")
	}
	if norm.NFC.String(s) != s {
		return fail("not_nfc")
	}
	if strings.HasPrefix(s, "/") || strings.HasSuffix(s, "/") || strings.Contains(s, "//") || strings.ContainsRune(s, '\\') {
		return fail("not_relative_posix")
	}
	for _, segment := range strings.Split(s, "/") {
		if segment == "" || segment == "." || segment == ".." {
			return fail("invalid_segment")
		}
		if strings.HasSuffix(segment, ".") || strings.HasSuffix(segment, " ") {
			return fail("windows_unsafe_suffix")
		}
		for _, r := range segment {
			if r <= 0x1f || (r >= 0x7f && r <= 0x9f) || strings.ContainsRune(`<>:"|?*`, r) {
				return fail("unsafe_character")
			}
		}
		base := strings.ToUpper(strings.SplitN(segment, ".", 2)[0])
		if base == "CON" || base == "PRN" || base == "AUX" || base == "NUL" ||
			(len(base) == 4 && ((strings.HasPrefix(base, "COM") || strings.HasPrefix(base, "LPT")) && base[3] >= '1' && base[3] <= '9')) {
			return fail("windows_device_name")
		}
	}
	return WorkspacePath(s), nil
}

func ParseWorkspaceUUID(field, s string) (WorkspaceUUID, error)
func (h *WorkspaceNullableHash) UnmarshalJSON(data []byte) error
func (h WorkspaceNullableHash) MarshalJSON() ([]byte, error)
func (v *WorkspaceNullableUUID) UnmarshalJSON(data []byte) error {
	v.Present = true
	if bytes.Equal(data, []byte("null")) {
		v.Value = nil
		return nil
	}
	var raw string
	if err := json.Unmarshal(data, &raw); err != nil {
		return &WorkspaceValidationError{Field: "uuid", Reason: "must_be_uuid_or_null"}
	}
	parsed, err := ParseWorkspaceUUID("uuid", raw)
	if err != nil { return err }
	v.Value = &parsed
	return nil
}
func (v WorkspaceNullableUUID) MarshalJSON() ([]byte, error)

func (v *WorkspaceNullableUint64) UnmarshalJSON(data []byte) error {
	v.Present = true
	if bytes.Equal(data, []byte("null")) {
		v.Value = nil
		return nil
	}
	var parsed uint64
	if err := json.Unmarshal(data, &parsed); err != nil {
		return &WorkspaceValidationError{Field: "uint64", Reason: "must_be_uint64_or_null"}
	}
	v.Value = &parsed
	return nil
}
func (v WorkspaceNullableUint64) MarshalJSON() ([]byte, error)
func (m WorkspaceFileMetadata) Validate(kind WorkspaceEntryKind) error
```

For all three Present-aware nullable types, a missing object key leaves `Present=false`; an explicit JSON `null` invokes `UnmarshalJSON`, producing `Present=true, Value=nil`. Their `MarshalJSON` methods return `WorkspaceValidationError{Reason:"required_key_missing"}` when `Present=false`, so required-null keys cannot silently disappear during re-encode.

- [ ] **Step 1: RED tests**

覆盖上文全部规则，含 `"0"`、max uint64、64 lowercase hex、4096-byte NFC path 的有效边界，以及 number/负数/leading zero/overflow revision，算法/长度/大写/non-hex hash，absolute/traversal/backslash/control/NFD/Windows-unsafe path。错误断言 typed `Field`/`Reason`，不匹配 message 文本。

代表性完整测试先写入 `internal/dto/workspace_v2_dto_ws_test.go`：

```go
func TestWorkspaceRevisionJSON(t *testing.T) {
	t.Parallel()
	tests := []struct {
		name    string
		input   string
		want    WorkspaceRevision
		wantErr string
	}{
		{name: "zero", input: `"0"`, want: 0},
		{name: "max", input: `"18446744073709551615"`, want: WorkspaceRevision(math.MaxUint64)},
		{name: "number rejected", input: `1`, wantErr: "must_be_string"},
		{name: "leading zero", input: `"01"`, wantErr: "non_canonical_decimal"},
		{name: "overflow", input: `"18446744073709551616"`, wantErr: "non_canonical_decimal"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			var got WorkspaceRevision
			err := json.Unmarshal([]byte(tt.input), &got)
			if tt.wantErr == "" {
				require.NoError(t, err)
				require.Equal(t, tt.want, got)
				encoded, err := json.Marshal(got)
				require.NoError(t, err)
				require.Equal(t, tt.input, string(encoded))
				return
			}
			var validationErr *WorkspaceValidationError
			require.ErrorAs(t, err, &validationErr)
			require.Equal(t, tt.wantErr, validationErr.Reason)
		})
	}
}
```

- [ ] **Step 2: Verify RED**

```bash
GOMAXPROCS=1 go test -p 1 ./internal/dto -run 'TestWorkspace(Revision|ContentHash|Path)' -count=1
```

Expected: compilation FAIL，undefined primitives/parsers；失败原因只能是 contract 尚未实现。

- [ ] **Step 3: GREEN implementation**

自定义 revision/hash JSON marshal/unmarshal；用 `golang.org/x/text/unicode/norm` 做 NFC；路径按 POSIX segment 校验，不用 host `filepath`；UUID 用 parse 后与 `String()` 比较 canonical。strict decoder 先用 token walk 记录每层 object keys 并拒绝 duplicate key，再递归检查 required keys/nullability、用 `DisallowUnknownFields` 解 concrete struct，并验证单 object EOF。Null 只允许 Present-aware nullable、required non-`omitempty` pointer wire-null 和 `json.RawMessage` leaf；scalar/struct/slice/array/map 及 present `omitempty` pointer 一律拒绝 null。nullable hash 记录 key 是否 present，区分 omitted/null。

- [ ] **Step 4: Verify GREEN**

```bash
GOMAXPROCS=1 go test -p 1 ./internal/dto -run 'TestWorkspace(Revision|ContentHash|Path)' -count=1
```

Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add internal/dto/workspace_v2_dto_ws.go internal/dto/workspace_v2_dto_ws_test.go
git commit -m "feat(sync): add workspace v2 wire primitives"
```

### Phase 2.2: Define Envelopes, Errors, and Action Names

**Files:**
- Modify: `internal/dto/workspace_v2_dto_ws.go`
- Modify: `internal/dto/workspace_v2_dto_ws_test.go`

**Produces for later phases:** `WorkspaceV2Request[T]`、`WorkspaceV2Response[T]`、`WorkspaceV2Error`、`WorkspaceV2FieldError`、`WorkspaceV2ErrorCode`、`WorkspaceV2Action`、`WorkspaceV2Flow`、exact `WorkspaceV2Actions`、`DecodeWorkspaceV2Request`、`EncodeWorkspaceV2Response`、`EncodeWorkspaceV2UnknownActionFailure`；全部 DTO/registry 就绪后再增加 `DecodeWorkspaceV2Data`。本 Phase 不声明 factory/spec registry，也不引用任何 Phase 2.3–2.5 DTO。

**Required envelope, action-name, and flow code:**

```go
type WorkspaceV2Request[T any] struct {
	RequestID WorkspaceUUID `json:"requestId"`
	Data      T             `json:"data"`
}

type WorkspaceV2Response[T any] struct {
	RequestID *WorkspaceUUID  `json:"requestId,omitempty"`
	Status    bool            `json:"status"`
	Data      *T              `json:"data,omitempty"`
	Error     *WorkspaceV2Error `json:"error,omitempty"`
}

type WorkspaceV2FieldError struct {
	Field  string `json:"field"`
	Reason string `json:"reason"`
}

type WorkspaceV2Error struct {
	Code      WorkspaceV2ErrorCode   `json:"code"`
	Message   string                 `json:"message"`
	Retryable bool                   `json:"retryable"`
	Fields    []WorkspaceV2FieldError `json:"fields,omitempty"`
}

type WorkspaceV2ErrorCode string
func NewWorkspaceV2Error(code WorkspaceV2ErrorCode, fields ...WorkspaceV2FieldError) WorkspaceV2Error

type WorkspaceV2Action string
const (
	WorkspaceActionHello             WorkspaceV2Action = "WorkspaceHello"
	WorkspaceActionSubscribe         WorkspaceV2Action = "WorkspaceSubscribe"
	WorkspaceActionSnapshotBegin     WorkspaceV2Action = "WorkspaceSnapshotBegin"
	WorkspaceActionSnapshotEntry     WorkspaceV2Action = "WorkspaceSnapshotEntry"
	WorkspaceActionSnapshotEnd       WorkspaceV2Action = "WorkspaceSnapshotEnd"
	WorkspaceActionMutation          WorkspaceV2Action = "WorkspaceMutation"
	WorkspaceActionMutationAccepted  WorkspaceV2Action = "WorkspaceMutationAccepted"
	WorkspaceActionMutationRejected  WorkspaceV2Action = "WorkspaceMutationRejected"
	WorkspaceActionEvent             WorkspaceV2Action = "WorkspaceEvent"
	WorkspaceActionAck               WorkspaceV2Action = "WorkspaceAck"
	WorkspaceActionBlobNeed          WorkspaceV2Action = "WorkspaceBlobNeed"
	WorkspaceActionBlobBegin         WorkspaceV2Action = "WorkspaceBlobBegin"
	WorkspaceActionBlobEnd           WorkspaceV2Action = "WorkspaceBlobEnd"
	WorkspaceActionConflictCreated   WorkspaceV2Action = "WorkspaceConflictCreated"
	WorkspaceActionConflictResolved  WorkspaceV2Action = "WorkspaceConflictResolved"
)

type WorkspaceV2Flow string
const (
	WorkspaceFlowClientRequest  WorkspaceV2Flow = "client_request"
	WorkspaceFlowServerResponse WorkspaceV2Flow = "server_response"
	WorkspaceFlowServerPush     WorkspaceV2Flow = "server_push"
)

var WorkspaceV2Actions = []WorkspaceV2Action{
	WorkspaceActionHello, WorkspaceActionSubscribe,
	WorkspaceActionSnapshotBegin, WorkspaceActionSnapshotEntry, WorkspaceActionSnapshotEnd,
	WorkspaceActionMutation, WorkspaceActionMutationAccepted, WorkspaceActionMutationRejected,
	WorkspaceActionEvent, WorkspaceActionAck,
	WorkspaceActionBlobNeed, WorkspaceActionBlobBegin, WorkspaceActionBlobEnd,
	WorkspaceActionConflictCreated, WorkspaceActionConflictResolved,
}

func (r WorkspaceV2Response[T]) Validate() error
func DecodeWorkspaceV2Request[T any](frame []byte, dst *WorkspaceV2Request[T]) error
func DecodeWorkspaceV2Data(action WorkspaceV2Action, flow WorkspaceV2Flow, data []byte) (any, error)
func EncodeWorkspaceV2Response[T any](action WorkspaceV2Action, response WorkspaceV2Response[T]) ([]byte, error)
func EncodeWorkspaceV2UnknownActionFailure(receivedAction string, requestID *WorkspaceUUID) ([]byte, error)
```

Task 3 MUST use `DecodeWorkspaceV2Data` for registered concrete data. It calls `NewWorkspaceV2Data` without another action switch, strict-decodes duplicate/unknown/trailing/required-key/nullability rules, preserves typed `unknown_action`/`flow_not_allowed`, normalizes schema/JSON failures to `data:invalid_json`, and returns the concrete pointer without state-dependent `Validate` calls.

Task 3 MUST use `EncodeWorkspaceV2Response` for every registered outbound frame. Successful output infers `server_push` when `requestId` is nil and `server_response` otherwise, then requires both a declared registry flow and an exact concrete data pointer type match with that flow's factory. Every registered action may encode a same-action failure, including a server-only action illegally received from a client. Safely echoing an unregistered ASCII `[A-Za-z][A-Za-z0-9]{0,63}` token MUST use `EncodeWorkspaceV2UnknownActionFailure`; unsafe or registered tokens are rejected, and this API never adds an action.

- [ ] **Step 1: RED tests**

断言 exact JSON keys、requestId echo、success/error mutual exclusion、duplicate/unknown field rejection、完整 error set，以及 `WorkspaceV2Actions` 的 exact 15 names/order/count。`WorkspaceV2Flow` 只锁三个合法字符串。后续 registry DTO 齐备后，补齐 outbound encoder 的 success response/push flow、concrete pointer type、known-action failure 和 safe/unsafe/registered unknown-action tests，不新增 action switch。

代表性完整 action-name test：

```go
func TestWorkspaceV2ActionNames(t *testing.T) {
	t.Parallel()
	want := []WorkspaceV2Action{
		WorkspaceActionHello, WorkspaceActionSubscribe,
		WorkspaceActionSnapshotBegin, WorkspaceActionSnapshotEntry, WorkspaceActionSnapshotEnd,
		WorkspaceActionMutation, WorkspaceActionMutationAccepted, WorkspaceActionMutationRejected,
		WorkspaceActionEvent, WorkspaceActionAck,
		WorkspaceActionBlobNeed, WorkspaceActionBlobBegin, WorkspaceActionBlobEnd,
		WorkspaceActionConflictCreated, WorkspaceActionConflictResolved,
	}
	require.Equal(t, want, WorkspaceV2Actions)
	require.Len(t, WorkspaceV2Actions, 15)
	require.Equal(t, WorkspaceV2Flow("client_request"), WorkspaceFlowClientRequest)
	require.Equal(t, WorkspaceV2Flow("server_response"), WorkspaceFlowServerResponse)
	require.Equal(t, WorkspaceV2Flow("server_push"), WorkspaceFlowServerPush)
}
```

- [ ] **Step 2: Verify RED**

```bash
GOMAXPROCS=1 go test -p 1 ./internal/dto -run 'TestWorkspaceV2(Envelope|ErrorCode|ActionNames)' -count=1
```

Expected: compilation FAIL，undefined envelope/error/action-name/flow types；不得因未来 DTO 或 factory 缺失失败。

- [ ] **Step 3: GREEN implementation**

只实现 envelopes、stable errors、15 action constants、三个 flow constants 和独立 `WorkspaceV2Actions` list；不声明 factory/spec registry。失败 envelope 不复用/修改 v1 `pkgapp.Res`。

- [ ] **Step 4: Verify GREEN in Worktree B**

```bash
GOMAXPROCS=1 go test -p 1 ./internal/dto -run 'TestWorkspaceV2(Envelope|ErrorCode|ActionNames)' -count=1
```

Expected: PASS。Task 1A 的 v1 baseline 在另一 worktree 独立运行，两个 arm 合入后才共同验证。

- [ ] **Step 5: Commit**

```bash
git add internal/dto/workspace_v2_dto_ws.go internal/dto/workspace_v2_dto_ws_test.go
git commit -m "feat(sync): define workspace v2 control envelope"
```

### Phase 2.3: Add Stream, Mutation, and Event DTOs

**Files:**
- Modify: `internal/dto/workspace_v2_dto_ws.go`
- Modify: `internal/dto/workspace_v2_dto_ws_test.go`

**Produces for Task 2:** `WorkspaceHelloRequest`、`WorkspaceHelloResponse`、`WorkspaceSubscribeRequest`、`WorkspaceSnapshotBeginMessage`、`WorkspaceSnapshotEntryMessage`、`WorkspaceSnapshotEndMessage`、`WorkspacePathState`、`WorkspaceMutation`、`WorkspaceMutationAcceptedMessage`、`WorkspaceMutationRejectedMessage`、`WorkspaceEventMessage`、`WorkspaceAckRequest`。

**Required public wire DTOs:**

```go
type WorkspaceHelloRequest struct {
	ProtocolVersion string        `json:"protocolVersion"`
	ClientID        WorkspaceUUID `json:"clientId"`
	ClientVersion   string        `json:"clientVersion"`
	Capabilities    []string      `json:"capabilities"`
}

type WorkspaceHelloResponse struct {
	ProtocolVersion           string `json:"protocolVersion"`
	ServerVersion             string `json:"serverVersion"`
	MaxControlFrameBytes      uint32 `json:"maxControlFrameBytes"`
	MaxBinaryChunkBytes       uint32 `json:"maxBinaryChunkBytes"`
	MaxBlobBytes              uint64 `json:"maxBlobBytes"`
	MaxTransfersPerConnection uint32 `json:"maxTransfersPerConnection"`
	HeartbeatSeconds          uint32 `json:"heartbeatSeconds"`
}

type WorkspaceSubscribeRequest struct {
	WorkspaceID     WorkspaceUUID     `json:"workspaceId"`
	ClientID        WorkspaceUUID     `json:"clientId"`
	LastAckRevision WorkspaceRevision `json:"lastAckRevision"`
}

type WorkspacePathState struct {
	Path         WorkspacePath         `json:"path"`
	PathRevision WorkspaceRevision     `json:"pathRevision"`
	Kind         WorkspaceEntryKind    `json:"kind"`
	ContentHash  WorkspaceNullableHash `json:"contentHash"`
	Metadata     WorkspaceFileMetadata `json:"metadata"`
	Tombstone    bool                  `json:"tombstone"`
}

type WorkspaceSnapshotMode string
const (
	WorkspaceSnapshotFull        WorkspaceSnapshotMode = "snapshot"
	WorkspaceSnapshotIncremental WorkspaceSnapshotMode = "incremental"
)

type WorkspaceSnapshotBeginMessage struct {
	WorkspaceID   WorkspaceUUID         `json:"workspaceId"`
	StreamID      WorkspaceUUID         `json:"streamId"`
	Mode          WorkspaceSnapshotMode `json:"mode"`
	FromRevision  WorkspaceRevision     `json:"fromRevision"`
	FinalRevision WorkspaceRevision     `json:"finalRevision"`
	EntryCount    uint32                `json:"entryCount"`
	EventCount    uint32                `json:"eventCount"`
	ConflictCount uint32                `json:"conflictCount"`
}

type WorkspaceSnapshotEntryMessage struct {
	WorkspaceID WorkspaceUUID      `json:"workspaceId"`
	StreamID    WorkspaceUUID      `json:"streamId"`
	Index       uint32             `json:"index"`
	Entry       WorkspacePathState `json:"entry"`
}

type WorkspaceSnapshotEndMessage struct {
	WorkspaceID    WorkspaceUUID         `json:"workspaceId"`
	StreamID       WorkspaceUUID         `json:"streamId"`
	Mode           WorkspaceSnapshotMode `json:"mode"`
	DeliveredCount uint32                `json:"deliveredCount"`
	FinalRevision  WorkspaceRevision     `json:"finalRevision"`
}

type WorkspaceMutation struct {
	WorkspaceID            WorkspaceUUID         `json:"workspaceId"`
	ClientID               WorkspaceUUID         `json:"clientId"`
	OperationID            WorkspaceUUID         `json:"operationId"`
	Path                   WorkspacePath         `json:"path"`
	BasePathRevision       WorkspaceRevision     `json:"basePathRevision"`
	Kind                   WorkspaceMutationKind `json:"kind"`
	ContentHash            WorkspaceNullableHash `json:"contentHash"`
	Metadata               WorkspaceFileMetadata `json:"metadata"`
	NewPath                *WorkspacePath        `json:"newPath,omitempty"`
	TargetBasePathRevision *WorkspaceRevision    `json:"targetBasePathRevision,omitempty"`
}

type WorkspaceMutationAcceptedMessage struct {
	WorkspaceID  WorkspaceUUID       `json:"workspaceId"`
	ClientID     WorkspaceUUID       `json:"clientId"`
	OperationID  WorkspaceUUID       `json:"operationId"`
	Revision     WorkspaceRevision   `json:"revision"`
	PathState    WorkspacePathState  `json:"pathState"`
	OldPathState *WorkspacePathState `json:"oldPathState,omitempty"`
	NewPathState *WorkspacePathState `json:"newPathState,omitempty"`
}

type WorkspaceMutationRejectedMessage struct {
	WorkspaceID      WorkspaceUUID        `json:"workspaceId"`
	ClientID         WorkspaceUUID        `json:"clientId"`
	OperationID      WorkspaceUUID        `json:"operationId"`
	Reason           string               `json:"reason"`
	CurrentPathState *WorkspacePathState  `json:"currentPathState"`
	ConflictID       *WorkspaceUUID       `json:"conflictId"`
	RequiredHash     *WorkspaceContentHash `json:"requiredHash"`
}

type WorkspaceEventMessage struct {
	WorkspaceID    WorkspaceUUID       `json:"workspaceId"`
	StreamID       WorkspaceUUID       `json:"streamId"`
	Index          uint32              `json:"index"`
	Revision       WorkspaceRevision   `json:"revision"`
	OperationID    WorkspaceUUID       `json:"operationId"`
	OriginClientID WorkspaceUUID       `json:"originClientId"`
	Mutation       WorkspaceMutation     `json:"mutation"`
	PathState      WorkspacePathState  `json:"pathState"`
	OldPathState   *WorkspacePathState `json:"oldPathState,omitempty"`
	NewPathState   *WorkspacePathState `json:"newPathState,omitempty"`
}

type WorkspaceAckRequest struct {
	WorkspaceID WorkspaceUUID     `json:"workspaceId"`
	ClientID    WorkspaceUUID     `json:"clientId"`
	Revision    WorkspaceRevision `json:"revision"`
}

func (m WorkspacePathState) Validate() error
func (m WorkspaceSnapshotBeginMessage) Validate() error
func (m WorkspaceMutation) Validate() error
func (m WorkspaceMutationAcceptedMessage) Validate() error
func (m WorkspaceMutationRejectedMessage) Validate() error
func (m WorkspaceEventMessage) Validate(previousIndex uint32, previousRevision WorkspaceRevision) error
func (m WorkspaceAckRequest) Validate(previousAck, lastDelivered WorkspaceRevision) error
```

Action constants use the `WorkspaceAction*` prefix so the public payload type remains the Task 2 contract name `WorkspaceMutation` without a Go identifier collision.

- [ ] **Step 1: RED tests for actions 1–10**

每类型一个 canonical round-trip，并覆盖 required `conflictCount`、snapshot/incremental checked delivered-count arithmetic、count/mode mismatch、stream index gap、revision order、hash-kind mismatch、rename missing/forbidden fields、directory-into-child、Ack regression/overshoot、operation reuse shape。service-state validation 通过显式参数（如 lastSentRevision）表达，不引数据库。

- [ ] **Step 2: Verify RED**

```bash
GOMAXPROCS=1 go test -p 1 ./internal/dto -run 'TestWorkspace(V2(Hello|Subscribe|Snapshot|Mutation|Event|Ack)|Snapshot)' -count=1
```

Expected: compilation FAIL at first undefined action DTO。

- [ ] **Step 3: GREEN implementation**

只用 named structs，不用 `map[string]any`。仅在 wire null 有语义时用 pointer/nullable type。每 action 提供 `Validate()`；service persistence 不进入本 Task。

- [ ] **Step 4: Verify GREEN**

```bash
GOMAXPROCS=1 go test -p 1 ./internal/dto -run 'TestWorkspace(V2(Hello|Subscribe|Snapshot|Mutation|Event|Ack)|Snapshot)' -count=1
```

Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add internal/dto/workspace_v2_dto_ws.go internal/dto/workspace_v2_dto_ws_test.go
git commit -m "feat(sync): define workspace mutation and stream messages"
```

### Phase 2.4: Add Blob DTOs and Binary Header

**Files:**
- Modify: `go.mod`
- Modify: `go.sum`
- Modify: `internal/dto/workspace_v2_dto_ws.go`
- Modify: `internal/dto/workspace_v2_dto_ws_test.go`

**Produces for Tasks 2/3:** `WorkspaceBlobNeedUploadPush`、`WorkspaceBlobNeedDownloadRequest`、`WorkspaceBlobNeedDownloadResponse`、`WorkspaceBlobBeginMessage`、`WorkspaceBlobEndMessage`、`WorkspaceBlobDirection`、`WorkspaceBlobHeader`、`MarshalWorkspaceBlobHeader`、`UnmarshalWorkspaceBlobHeader`，以及上文 limits 的 exported constants。

**Required blob DTO and binary header code:**

```go
const (
	WorkspaceBlobHeaderSize       = 64
	WorkspaceMaxControlFrameBytes = 65_536
	WorkspaceBlobChunkSize        = 1_048_576
	WorkspaceMaxBlobBytes  uint64 = 5_368_709_120
)

type WorkspaceBlobDirection string
const (
	WorkspaceBlobUpload   WorkspaceBlobDirection = "upload"
	WorkspaceBlobDownload WorkspaceBlobDirection = "download"
)

type WorkspaceBlobNeedUploadPush struct {
	WorkspaceID WorkspaceUUID        `json:"workspaceId"`
	Direction   WorkspaceBlobDirection `json:"direction"`
	OperationID WorkspaceUUID        `json:"operationId"`
	ContentHash WorkspaceContentHash `json:"contentHash"`
	Size        uint64               `json:"size"`
}

type WorkspaceBlobNeedDownloadRequest struct {
	WorkspaceID WorkspaceUUID          `json:"workspaceId"`
	Direction   WorkspaceBlobDirection `json:"direction"`
	OperationID WorkspaceNullableUUID  `json:"operationId"`
	ContentHash WorkspaceContentHash   `json:"contentHash"`
	Size        WorkspaceNullableUint64 `json:"size"`
}

type WorkspaceBlobNeedDownloadResponse struct {
	WorkspaceID WorkspaceUUID          `json:"workspaceId"`
	Direction   WorkspaceBlobDirection `json:"direction"`
	OperationID WorkspaceNullableUUID  `json:"operationId"`
	ContentHash WorkspaceContentHash   `json:"contentHash"`
	Size        uint64                 `json:"size"`
}

type WorkspaceBlobBeginMessage struct {
	WorkspaceID WorkspaceUUID         `json:"workspaceId"`
	TransferID  WorkspaceUUID         `json:"transferId"`
	Direction   WorkspaceBlobDirection `json:"direction"`
	ContentHash WorkspaceContentHash  `json:"contentHash"`
	Size        uint64                `json:"size"`
	ChunkSize   uint32                `json:"chunkSize"`
	ChunkCount  uint64                `json:"chunkCount"`
}

type WorkspaceBlobEndMessage struct {
	WorkspaceID WorkspaceUUID         `json:"workspaceId"`
	TransferID  WorkspaceUUID         `json:"transferId"`
	Direction   WorkspaceBlobDirection `json:"direction"`
	ContentHash WorkspaceContentHash  `json:"contentHash"`
	Size        uint64                `json:"size"`
	ChunkCount  uint64                `json:"chunkCount"`
}

type WorkspaceBlobHeader struct {
	Direction   WorkspaceBlobDirection
	Final       bool
	TransferID  uuid.UUID
	ChunkIndex  uint64
	Offset      uint64
	PayloadLen  uint32
	ChunkDigest [16]byte
}

func MarshalWorkspaceBlobHeader(h WorkspaceBlobHeader) ([WorkspaceBlobHeaderSize]byte, error) {
	var out [WorkspaceBlobHeaderSize]byte
	copy(out[0:4], "FNS2")
	out[4] = 2
	switch h.Direction {
	case WorkspaceBlobUpload:
		out[5] = 1
	case WorkspaceBlobDownload:
		out[5] = 2
	default:
		return out, &WorkspaceValidationError{Field: "direction", Reason: "invalid_enum"}
	}
	if h.Final { out[6] = 1 }
	out[7] = WorkspaceBlobHeaderSize
	copy(out[8:24], h.TransferID[:])
	binary.BigEndian.PutUint64(out[24:32], h.ChunkIndex)
	binary.BigEndian.PutUint64(out[32:40], h.Offset)
	binary.BigEndian.PutUint32(out[40:44], h.PayloadLen)
	copy(out[48:64], h.ChunkDigest[:])
	return out, nil
}

func UnmarshalWorkspaceBlobHeader(data []byte, actualPayloadLen uint32, expectedDigest [16]byte) (WorkspaceBlobHeader, error)
func (h WorkspaceBlobHeader) ValidateSequence(expectedIndex, expectedOffset uint64, isLast bool) error
func (m WorkspaceBlobNeedUploadPush) Validate() error
func (m WorkspaceBlobNeedDownloadRequest) Validate() error
func (m WorkspaceBlobNeedDownloadResponse) Validate() error
func (m WorkspaceBlobBeginMessage) Validate() error
func (m WorkspaceBlobEndMessage) Validate() error

func ComputeWorkspaceBlobDigest(payload []byte) (full [32]byte, first16 [16]byte) {
	full = blake3.Sum256(payload)
	copy(first16[:], full[:16])
	return full, first16
}
```

`WorkspaceBlobNeedDownloadRequest.Validate` and `WorkspaceBlobNeedDownloadResponse.Validate` must require `OperationID.Present && OperationID.Value == nil`; the request additionally requires `Size.Present && Size.Value == nil`. Missing keys and non-null values are both invalid, with distinct reasons `required_key_missing` and `must_be_null`.

- [ ] **Step 0: Add the protocol hash dependency before RED**

```bash
go get github.com/zeebo/blake3@v0.2.4
GOMAXPROCS=1 go test -p 1 ./internal/dto -count=1
git add go.mod go.sum
git commit -m "build(sync): add blake3 protocol dependency"
```

Expected: the existing dto suite PASS before any new blob/digest test or production implementation is written. This setup commit contains only `go.mod`/`go.sum`.

- [ ] **Step 1: RED tests for actions 11–13/header**

先在 test import `github.com/zeebo/blake3`，覆盖 begin/end arithmetic、zero-byte `chunkCount:0` 且无 binary frame、direction、limits、exact offsets/endianness、reserved bits、magic/version/header length、`payloadLength:0` 拒绝、从 payload 计算 full BLAKE3 与 header first16（含 empty slice digest unit case）、digest mismatch、truncated/oversized frame、duplicate/out-of-order index、wrong offset、final flag。

代表性完整 header layout test：

```go
func TestWorkspaceBlobHeaderLayout(t *testing.T) {
	t.Parallel()
	id := uuid.MustParse("10000000-0000-4000-8000-000000000001")
	digest := [16]byte{0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15}
	raw, err := MarshalWorkspaceBlobHeader(WorkspaceBlobHeader{
		Direction: WorkspaceBlobUpload, Final: true, TransferID: id,
		ChunkIndex: 2, Offset: 2 * WorkspaceBlobChunkSize, PayloadLen: 7,
		ChunkDigest: digest,
	})
	require.NoError(t, err)
	require.Equal(t, []byte("FNS2"), raw[0:4])
	require.Equal(t, byte(2), raw[4])
	require.Equal(t, byte(1), raw[5])
	require.Equal(t, byte(1), raw[6])
	require.Equal(t, byte(64), raw[7])
	require.Equal(t, uint64(2), binary.BigEndian.Uint64(raw[24:32]))
	require.Equal(t, uint64(2*WorkspaceBlobChunkSize), binary.BigEndian.Uint64(raw[32:40]))
	require.Equal(t, uint32(7), binary.BigEndian.Uint32(raw[40:44]))
	require.Equal(t, []byte{0, 0, 0, 0}, raw[44:48])
	require.Equal(t, digest[:], raw[48:64])
}
```

代表性 required-null test：

```go
func TestWorkspaceBlobNeedDownloadRequiresExplicitNull(t *testing.T) {
	t.Parallel()
	base := `{"workspaceId":"10000000-0000-4000-8000-000000000001","direction":"download","contentHash":"blake3:` + strings.Repeat("0", 64) + `"}`
	var missing WorkspaceBlobNeedDownloadRequest
	require.NoError(t, json.Unmarshal([]byte(base), &missing))
	err := missing.Validate()
	var validationErr *WorkspaceValidationError
	require.ErrorAs(t, err, &validationErr)
	require.Equal(t, "required_key_missing", validationErr.Reason)

	explicit := strings.TrimSuffix(base, "}") + `,"operationId":null,"size":null}`
	var valid WorkspaceBlobNeedDownloadRequest
	require.NoError(t, json.Unmarshal([]byte(explicit), &valid))
	require.NoError(t, valid.Validate())
	require.True(t, valid.OperationID.Present)
	require.Nil(t, valid.OperationID.Value)
	require.True(t, valid.Size.Present)
	require.Nil(t, valid.Size.Value)

	nonNull := strings.TrimSuffix(base, "}") + `,"operationId":"10000000-0000-4000-8000-000000000002","size":1}`
	var invalid WorkspaceBlobNeedDownloadRequest
	require.NoError(t, json.Unmarshal([]byte(nonNull), &invalid))
	err = invalid.Validate()
	require.ErrorAs(t, err, &validationErr)
	require.Equal(t, "must_be_null", validationErr.Reason)
}
```

- [ ] **Step 2: Verify RED**

```bash
GOMAXPROCS=1 go test -p 1 ./internal/dto -run 'TestWorkspace(V2Blob|BlobHeader|BlobNeedDownload)' -count=1
```

Expected: FAIL because `ComputeWorkspaceBlobDigest`, blob/header DTOs, required-null validation, or the newly asserted behavior is not implemented. The dependency is already present, so missing-module/setup failure is not an acceptable RED.

- [ ] **Step 3: GREEN implementation**

只实现使 RED 变绿的 production DTO、validators 和 digest/header codec；此步不再修改 dependency files 或 tests。marshal 固定 64 bytes；unmarshal 接收 header、实际 payload length 与由 `ComputeWorkspaceBlobDigest` 计算的 expected first16，验证全部固定字段并返回 typed validation error。Task 2 blob store 直接消费相同 module 做 streaming chunk/full-blob 计算，不引入第二套 BLAKE3 实现。

- [ ] **Step 4: Verify GREEN**

```bash
GOMAXPROCS=1 go test -p 1 ./internal/dto -run 'TestWorkspace(V2Blob|BlobHeader|BlobNeedDownload)' -count=1
```

Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add internal/dto/workspace_v2_dto_ws.go internal/dto/workspace_v2_dto_ws_test.go
git commit -m "feat(sync): define workspace blob framing"
```

### Phase 2.5: Add Conflict DTOs

**Files:**
- Modify: `internal/dto/workspace_v2_dto_ws.go`
- Modify: `internal/dto/workspace_v2_dto_ws_test.go`

**Produces for Task 2:** distinct opaque `WorkspaceConflictRevision`、`WorkspaceConflictSide`、`WorkspaceConflictKind`、`WorkspaceConflictCreatedMessage`、`WorkspaceConflictResolvedRequest`、`WorkspaceConflictChoice`、`WorkspaceConflictResolvedMessage`。同 action 的 request/response/push 由 envelope direction 选择后两个 concrete DTO。

**Required conflict public types:**

```go
type WorkspaceConflictRevision struct {
	value uint64
}

func ParseWorkspaceConflictRevision(s string) (WorkspaceConflictRevision, error)
func (r WorkspaceConflictRevision) MarshalJSON() ([]byte, error)
func (r *WorkspaceConflictRevision) UnmarshalJSON(data []byte) error

type WorkspaceConflictKind string
type WorkspaceConflictChoice string

const (
	WorkspaceConflictContent      WorkspaceConflictKind = "content"
	WorkspaceConflictDeleteModify WorkspaceConflictKind = "delete_modify"
	WorkspaceConflictRename       WorkspaceConflictKind = "rename"
	WorkspaceConflictBinary       WorkspaceConflictKind = "binary"
	WorkspaceConflictKeepCurrent  WorkspaceConflictChoice = "current"
	WorkspaceConflictUseIncoming  WorkspaceConflictChoice = "incoming"
	WorkspaceConflictUseMerged    WorkspaceConflictChoice = "merged"
	WorkspaceConflictDelete       WorkspaceConflictChoice = "delete"
)

type WorkspaceConflictSide struct {
	Path         *WorkspacePath        `json:"path"`
	PathRevision WorkspaceRevision     `json:"pathRevision"`
	ContentHash  WorkspaceNullableHash `json:"contentHash"`
	Metadata     WorkspaceFileMetadata `json:"metadata"`
	Tombstone    bool                  `json:"tombstone"`
}

type WorkspaceConflictCreatedMessage struct {
	WorkspaceID         WorkspaceUUID        `json:"workspaceId"`
	ConflictID          WorkspaceUUID        `json:"conflictId"`
	ConflictRevision    WorkspaceConflictRevision `json:"conflictRevision"`
	Path                WorkspacePath        `json:"path"`
	Kind                WorkspaceConflictKind `json:"kind"`
	Ancestor            WorkspaceConflictSide `json:"ancestor"`
	Current             WorkspaceConflictSide `json:"current"`
	Incoming            WorkspaceConflictSide `json:"incoming"`
	CreatedByOperationID WorkspaceUUID        `json:"createdByOperationId"`
}

type WorkspaceConflictResolvedRequest struct {
	WorkspaceID      WorkspaceUUID         `json:"workspaceId"`
	ClientID         WorkspaceUUID         `json:"clientId"`
	OperationID      WorkspaceUUID         `json:"operationId"`
	ConflictID       WorkspaceUUID         `json:"conflictId"`
	ConflictRevision WorkspaceConflictRevision `json:"conflictRevision"`
	Choice           WorkspaceConflictChoice `json:"choice"`
	Path             WorkspacePath         `json:"path"`
	ContentHash      WorkspaceNullableHash `json:"contentHash"`
	Metadata         WorkspaceFileMetadata `json:"metadata"`
}

type WorkspaceConflictResolvedMessage struct {
	WorkspaceID        WorkspaceUUID         `json:"workspaceId"`
	ConflictID         WorkspaceUUID         `json:"conflictId"`
	ConflictRevision   WorkspaceConflictRevision `json:"conflictRevision"`
	OperationID        WorkspaceUUID         `json:"operationId"`
	Revision           WorkspaceRevision     `json:"revision"`
	Choice             WorkspaceConflictChoice `json:"choice"`
	PathState          WorkspacePathState    `json:"pathState"`
	ResolvedByClientID WorkspaceUUID         `json:"resolvedByClientId"`
}

func (m WorkspaceConflictCreatedMessage) Validate() error
func (m WorkspaceConflictResolvedRequest) ValidateAgainst(created WorkspaceConflictCreatedMessage) error
func (m WorkspaceConflictResolvedMessage) Validate() error
```

- [ ] **Step 1: RED tests for actions 14–15**

先用 reflection 锁定 `WorkspaceConflictRevision` 是与 `WorkspaceRevision` 不同的 comparable struct 且 storage unexported，再覆盖 canonical positive string JSON、zero/number/non-canonical rejection 和三处 DTO 字段 exact type。随后覆盖 content/binary/delete_modify/rename、null tombstone side、choice-specific hash/metadata、current/incoming side exact replay、merged、delete、显式不同的 stale conflict guard、resolved round-trip。path/tree revision fixture value必须显式给出，不得由 conflictRevision 加减推导。保留 DTO-level `TestWorkspaceV2ConflictMergedPayload`，只验证 merged request 必须带 hash、同 body 可换 requestId 重封装且 operationId 不变；跨 action sequence fixture test 归 Phase 2.6。

- [ ] **Step 2: Verify RED**

```bash
GOMAXPROCS=1 go test -p 1 ./internal/dto -run 'TestWorkspace(V2Conflict|ConflictRevision)' -count=1
```

Expected: compilation FAIL，conflict DTOs undefined。

- [ ] **Step 3: GREEN implementation**

不实现 merge 或 persistence。`WorkspaceConflictRevision` 只提供 canonical string parse/JSON 与 value equality；不提供 numeric accessor、ordering 或 arithmetic API，zero value/`"0"` 非法。`WorkspaceConflictResolvedRequest` validation 接收 caller 提供的 ConflictCreated；guard 不等先返回 `conflict_revision_stale`，相同时再检查 choice payload。文档接口明确 Task 2 的 pending resolve record 键为 `(clientId,operationId)`，包含 canonical resolve data digest、conflictRevision、required contentHash、createdAt 和 expiresAt=createdAt+24h；blob 缺失时先写 pending，再返回 correlated `blob_required` failure，随后 push BlobNeed，不写终态 operation result。stale 删除 pending；TTL expiry 只删 pending，不改 conflict/blob。

- [ ] **Step 4: Verify GREEN**

```bash
GOMAXPROCS=1 go test -p 1 ./internal/dto -run 'TestWorkspace(V2Conflict|ConflictRevision)' -count=1
```

Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add internal/dto/workspace_v2_dto_ws.go internal/dto/workspace_v2_dto_ws_test.go
git commit -m "feat(sync): define workspace conflict messages"
```

### Phase 2.6: Publish Shared Go/Rust Fixtures

**Files:**
- Modify: `internal/dto/workspace_v2_dto_ws.go`
- Modify: `internal/dto/workspace_v2_dto_ws_test.go`
- Create: `internal/dto/workspace_v2_fixtures_test.go`
- Create: `testdata/workspace-sync-v2/manifest.json`
- Create: `testdata/workspace-sync-v2/valid/control-frames.jsonl`
- Create: `testdata/workspace-sync-v2/valid/error-envelopes.jsonl`
- Create: `testdata/workspace-sync-v2/invalid/revisions.jsonl`
- Create: `testdata/workspace-sync-v2/invalid/hashes.jsonl`
- Create: `testdata/workspace-sync-v2/invalid/paths.jsonl`
- Create: `testdata/workspace-sync-v2/binary/header-vectors.json`

**Produces for Task 4:**
- Canonical source: `fast-note-sync-service/testdata/workspace-sync-v2/`
- Exact Rust copy destination: `<rust-workspace>/crates/fns-protocol/tests/fixtures/workspace-sync-v2/`
- Exact Rust tests: `<rust-workspace>/crates/fns-protocol/tests/workspace_v2_fixtures.rs` and `workspace_v2_binary_header.rs`

**Produces for Task 3 before fixtures:** `WorkspaceV2DataFactory`、`WorkspaceV2ActionSpec`、`WorkspaceV2ActionSpecs`、`NewWorkspaceV2Data`。这些 API 只能在 Phase 2.3–2.5 DTO 全部存在后加入。

#### Registry subcycle

- [ ] **Step R1: Write the failing full-flow registry test**

在 `internal/dto/workspace_v2_dto_ws_test.go` 新增 `TestWorkspaceV2ActionRegistryFlows`。测试必须覆盖 `WorkspaceV2Actions` 中全部 15 actions 的所有合法 flow，调用 `NewWorkspaceV2Data` 并断言 concrete pointer type；还要断言未声明 flow 返回 `flow_not_allowed`，未知 action 返回 `unknown_action`。尤其完整覆盖 BlobNeed/BlobBegin/BlobEnd/ConflictResolved 的三种 flow。

- [ ] **Step R2: Run registry RED after all DTOs exist**

```bash
GOMAXPROCS=1 go test -p 1 ./internal/dto -run 'TestWorkspaceV2ActionRegistryFlows' -count=1
```

Expected: compilation FAIL only because `WorkspaceV2DataFactory`、`WorkspaceV2ActionSpec`、`WorkspaceV2ActionSpecs`、`NewWorkspaceV2Data` are undefined. All concrete DTO names referenced by the test already compile from Phases 2.3–2.5.

- [ ] **Step R3: Implement the complete factory registry**

```go
type WorkspaceV2DataFactory func() any
type WorkspaceV2ActionSpec struct {
	Flows map[WorkspaceV2Flow]WorkspaceV2DataFactory
}

func workspaceV2Factory[T any]() WorkspaceV2DataFactory {
	return func() any { return new(T) }
}

var WorkspaceV2ActionSpecs = map[WorkspaceV2Action]WorkspaceV2ActionSpec{
	WorkspaceActionHello: {Flows: map[WorkspaceV2Flow]WorkspaceV2DataFactory{WorkspaceFlowClientRequest: workspaceV2Factory[WorkspaceHelloRequest](), WorkspaceFlowServerResponse: workspaceV2Factory[WorkspaceHelloResponse]()}},
	WorkspaceActionSubscribe: {Flows: map[WorkspaceV2Flow]WorkspaceV2DataFactory{WorkspaceFlowClientRequest: workspaceV2Factory[WorkspaceSubscribeRequest]()}},
	WorkspaceActionSnapshotBegin: {Flows: map[WorkspaceV2Flow]WorkspaceV2DataFactory{WorkspaceFlowServerPush: workspaceV2Factory[WorkspaceSnapshotBeginMessage]()}},
	WorkspaceActionSnapshotEntry: {Flows: map[WorkspaceV2Flow]WorkspaceV2DataFactory{WorkspaceFlowServerPush: workspaceV2Factory[WorkspaceSnapshotEntryMessage]()}},
	WorkspaceActionSnapshotEnd: {Flows: map[WorkspaceV2Flow]WorkspaceV2DataFactory{WorkspaceFlowServerPush: workspaceV2Factory[WorkspaceSnapshotEndMessage]()}},
	WorkspaceActionMutation: {Flows: map[WorkspaceV2Flow]WorkspaceV2DataFactory{WorkspaceFlowClientRequest: workspaceV2Factory[WorkspaceMutation]()}},
	WorkspaceActionMutationAccepted: {Flows: map[WorkspaceV2Flow]WorkspaceV2DataFactory{WorkspaceFlowServerResponse: workspaceV2Factory[WorkspaceMutationAcceptedMessage]()}},
	WorkspaceActionMutationRejected: {Flows: map[WorkspaceV2Flow]WorkspaceV2DataFactory{WorkspaceFlowServerResponse: workspaceV2Factory[WorkspaceMutationRejectedMessage]()}},
	WorkspaceActionEvent: {Flows: map[WorkspaceV2Flow]WorkspaceV2DataFactory{WorkspaceFlowServerPush: workspaceV2Factory[WorkspaceEventMessage]()}},
	WorkspaceActionAck: {Flows: map[WorkspaceV2Flow]WorkspaceV2DataFactory{WorkspaceFlowClientRequest: workspaceV2Factory[WorkspaceAckRequest](), WorkspaceFlowServerResponse: workspaceV2Factory[WorkspaceAckRequest]()}},
	WorkspaceActionBlobNeed: {Flows: map[WorkspaceV2Flow]WorkspaceV2DataFactory{WorkspaceFlowClientRequest: workspaceV2Factory[WorkspaceBlobNeedDownloadRequest](), WorkspaceFlowServerResponse: workspaceV2Factory[WorkspaceBlobNeedDownloadResponse](), WorkspaceFlowServerPush: workspaceV2Factory[WorkspaceBlobNeedUploadPush]()}},
	WorkspaceActionBlobBegin: {Flows: map[WorkspaceV2Flow]WorkspaceV2DataFactory{WorkspaceFlowClientRequest: workspaceV2Factory[WorkspaceBlobBeginMessage](), WorkspaceFlowServerResponse: workspaceV2Factory[WorkspaceBlobBeginMessage](), WorkspaceFlowServerPush: workspaceV2Factory[WorkspaceBlobBeginMessage]()}},
	WorkspaceActionBlobEnd: {Flows: map[WorkspaceV2Flow]WorkspaceV2DataFactory{WorkspaceFlowClientRequest: workspaceV2Factory[WorkspaceBlobEndMessage](), WorkspaceFlowServerResponse: workspaceV2Factory[WorkspaceBlobEndMessage](), WorkspaceFlowServerPush: workspaceV2Factory[WorkspaceBlobEndMessage]()}},
	WorkspaceActionConflictCreated: {Flows: map[WorkspaceV2Flow]WorkspaceV2DataFactory{WorkspaceFlowServerPush: workspaceV2Factory[WorkspaceConflictCreatedMessage]()}},
	WorkspaceActionConflictResolved: {Flows: map[WorkspaceV2Flow]WorkspaceV2DataFactory{WorkspaceFlowClientRequest: workspaceV2Factory[WorkspaceConflictResolvedRequest](), WorkspaceFlowServerResponse: workspaceV2Factory[WorkspaceConflictResolvedMessage](), WorkspaceFlowServerPush: workspaceV2Factory[WorkspaceConflictResolvedMessage]()}},
}

func NewWorkspaceV2Data(action WorkspaceV2Action, flow WorkspaceV2Flow) (any, error) {
	spec, ok := WorkspaceV2ActionSpecs[action]
	if !ok { return nil, &WorkspaceValidationError{Field: "action", Reason: "unknown_action"} }
	factory, ok := spec.Flows[flow]
	if !ok { return nil, &WorkspaceValidationError{Field: "flow", Reason: "flow_not_allowed"} }
	return factory(), nil
}
```

Implementation must iterate `WorkspaceV2Actions` in a completeness assertion/test so the independent action list and registry cannot drift; router and fixture code must not define another switch.

- [ ] **Step R4: Verify registry GREEN**

```bash
GOMAXPROCS=1 go test -p 1 ./internal/dto -run 'TestWorkspaceV2ActionRegistryFlows' -count=1
```

Expected: PASS for exact action/flow/concrete-type coverage.

- [ ] **Step R5: Commit registry separately**

```bash
git add internal/dto/workspace_v2_dto_ws.go internal/dto/workspace_v2_dto_ws_test.go
git commit -m "feat(sync): register workspace v2 action data flows"
```

#### Fixture subcycle

**Required test-only fixture row structs:**

```go
type workspaceFixtureManifest struct {
	SchemaVersion string                    `json:"schemaVersion"`
	Actions       []WorkspaceV2Action       `json:"actions"`
	Files         map[string]string         `json:"files"`
}

type workspaceControlFixtureRow struct {
	Case     string            `json:"case"`
	Sequence string            `json:"sequence,omitempty"`
	Step     uint32            `json:"step,omitempty"`
	Action   WorkspaceV2Action `json:"action"`
	Flow     WorkspaceV2Flow   `json:"flow"`
	Frame    string            `json:"frame"`
}

type workspaceErrorFixtureRow struct {
	Case   string            `json:"case"`
	Action WorkspaceV2Action `json:"action"`
	Frame  string            `json:"frame"`
}

type workspaceInvalidFixtureRow struct {
	Case   string          `json:"case"`
	Value  json.RawMessage `json:"value"`
	Field  string          `json:"field"`
	Reason string          `json:"reason"`
}

type workspaceBinaryHeaderVector struct {
	Case       string                 `json:"case"`
	Direction WorkspaceBlobDirection `json:"direction"`
	Final      bool                   `json:"final"`
	TransferID WorkspaceUUID          `json:"transferId"`
	ChunkIndex uint64                 `json:"chunkIndex"`
	Offset     uint64                 `json:"offset"`
	PayloadHex string                 `json:"payloadHex"`
	DigestHex  string                 `json:"digestHex"`
	HeaderHex  string                 `json:"headerHex"`
	Valid      bool                   `json:"valid"`
	Reason     string                 `json:"reason,omitempty"`
}
```

- [ ] **Step 1: RED loader tests before fixtures**

Manifest 必须有 `schemaVersion:"workspace-sync-v2-fixtures/1"`、六个 data-file lowercase SHA-256、主计划 exact 15-action list。valid loader 对每 row 调 `DecodeWorkspaceV2Data(row.Action,row.Flow,data)` 得到 concrete destination，再做需要的 Validate、re-encode、比 canonical JSON；不得从 Task 3 复制 strict decoder 或 action switch。invalid row 必须含 `case,value,field,reason` 并返回对应 typed reason。新增 `TestWorkspaceMergedConflictUploadSequenceFixtures`，验证每个 client request 紧跟 correlated response、首次/重连缺 blob 均先收到 `blob_required` failure 后收到 BlobNeed push、retry requestId 改变但 resolve data 与 operationId 不变、stale response 后序列终止。

代表性完整 manifest test：

```go
func TestWorkspaceV2FixtureManifest(t *testing.T) {
	t.Parallel()
	raw, err := os.ReadFile(filepath.Join("..", "..", "testdata", "workspace-sync-v2", "manifest.json"))
	require.NoError(t, err)
	var manifest workspaceFixtureManifest
	require.NoError(t, json.Unmarshal(raw, &manifest))
	require.Equal(t, "workspace-sync-v2-fixtures/1", manifest.SchemaVersion)
	require.Len(t, manifest.Actions, 15)
	require.Len(t, manifest.Files, 6)
	for name, digest := range manifest.Files {
		require.NotEmpty(t, name)
		require.Regexp(t, `^[0-9a-f]{64}$`, digest)
	}
	for action := range WorkspaceV2ActionSpecs {
		require.Contains(t, manifest.Actions, action)
	}
}
```

代表性 binary replay test（对 JSON array 中每个 row 执行）：

```go
func testWorkspaceBinaryHeaderVector(t *testing.T, row workspaceBinaryHeaderVector) {
	t.Helper()
	payload, err := hex.DecodeString(row.PayloadHex)
	require.NoError(t, err)
	wantFull, err := hex.DecodeString(row.DigestHex)
	require.NoError(t, err)
	require.Len(t, wantFull, 32)
	computed := blake3.Sum256(payload)

	headerBytes, err := hex.DecodeString(row.HeaderHex)
	require.NoError(t, err)
	require.Len(t, headerBytes, WorkspaceBlobHeaderSize)
	_, parseErr := UnmarshalWorkspaceBlobHeader(headerBytes, uint32(len(payload)), [16]byte(computed[:16]))
	if !row.Valid {
		require.NotEmpty(t, row.Reason)
		require.True(t, parseErr != nil || !bytes.Equal(wantFull, computed[:]), "invalid vector must fail header or full digest verification")
		return
	}
	require.NoError(t, parseErr)
	require.Equal(t, wantFull, computed[:], "full BLAKE3 must be independently replayable")
	require.Equal(t, computed[:16], headerBytes[48:64], "header stores first 16 digest bytes")
}
```

`header-vectors.json` 必须包含 valid upload/download/final/partial rows、invalid zero-payload row，以及 digestHex 正确但 header first16 被篡改、payload 被篡改、header magic/flags/reserved/length 非法的 mismatch rows；不得包含 valid zero-payload row。Empty payload 的 BLAKE3 只由 digest unit test 与 `blake3.Sum256(nil)` 对照覆盖。Go 不依赖预计算结果本身判定 hash：始终从 `payloadHex` 重算；Rust Task 4 从相同 payload 独立重算，因此 fixture 可跨仓独立复放。

- [ ] **Step 2: Verify RED**

```bash
GOMAXPROCS=1 go test -p 1 ./internal/dto -run 'TestWorkspace(V2Fixture|MergedConflict|SnapshotConflict)' -count=1
```

Expected: FAIL opening `../../testdata/workspace-sync-v2/manifest.json`，证明 fixture 是测试输入。

- [ ] **Step 3: Add complete vectors**

`control-frames.jsonl` 覆盖 exact 15 names 和 registry 声明的 exact 25 flows；普通 row 有 `case,action,flow,frame`，序列 row 另有相同 `sequence` 与从 1 连续的 `step`。`WorkspaceBlobNeed` 必须含 upload server_push、download client_request、download server_response；`WorkspaceBlobEnd` 必须含 download server_push、client acknowledgement request（新 requestId）和 correlated server_response；`WorkspaceConflictResolved` 必须含 resolve client_request、correlated server_response、subscriber server_push。增加 counted-conflict stream sequences：full 是 Begin(required conflictCount) → 全部 ordered entries → authoritative ordered ConflictCreated set → checked End；incremental 是 Begin → `WorkspaceEvent -> WorkspaceConflictResolved -> WorkspaceEvent` ordered revision-item union → authoritative ordered ConflictCreated set → checked End，eventCount 计 union 三项而两个 Event index 仍相邻。fixture test 必须直接证明 ConflictCreated raw data 没有 `streamId`/`index`，且注入任一字段被 strict decode 拒绝。另有三个 merged sequences：`merged-conflict-upload`（resolve request → correlated blob_required failure → BlobNeed push → upload → new-requestId exact-data retry → resolved response）、`merged-conflict-reconnect-missing`（same-operation retry → correlated blob_required failure → repeated BlobNeed push）、`merged-conflict-stale`（retry → correlated conflict_revision_stale，代表 pending delete）。24h pending expiry 没有额外 wire frame，作为 docs/service contract fixture metadata 说明，不伪造 action。使用固定 `10000000-...` IDs、`"0"`/`"1"`/max tree/path revision、独立 positive conflictRevision、NFC Unicode path、rename、四类 conflict、四种 resolution；任何 path/tree revision helper 都不得从 conflictRevision 做算术推导。error fixtures 必须覆盖每 code，并为 `blob_required` 固定 `retryable:false`；`invalid/paths.jsonl` 必须对每个锁定 path category 至少提供一条跨语言代表 row，其他 invalid files 也覆盖本计划每类非法值。

`binary/header-vectors.json` 使用上述完整 row schema；每个 digestHex 是 `github.com/zeebo/blake3 v0.2.4` 对 payloadHex 解码 bytes 的 32-byte digest，headerHex 的 bytes 48..63 是其前 16 bytes。fixture 中不得只存 header 而省略 payload/full digest，也不得用 Go encoder 生成后只自我比对同一结果。

- [ ] **Step 4: Record fixture SHA-256**

```bash
shasum -a 256 testdata/workspace-sync-v2/valid/control-frames.jsonl testdata/workspace-sync-v2/valid/error-envelopes.jsonl testdata/workspace-sync-v2/invalid/revisions.jsonl testdata/workspace-sync-v2/invalid/hashes.jsonl testdata/workspace-sync-v2/invalid/paths.jsonl testdata/workspace-sync-v2/binary/header-vectors.json
```

将六个 digest 原样写入 manifest；manifest 不 hash 自己。

- [ ] **Step 5: Verify GREEN**

```bash
GOMAXPROCS=1 go test -p 1 ./internal/dto -run 'TestWorkspace(V2Fixture|MergedConflict|SnapshotConflict)' -count=1
```

Expected: PASS，包含 action count/digests。

- [ ] **Step 6: Lock Task 4 consumption**

Task 4 byte-for-byte copy canonical directory到 Rust destination，serde 类型使用同 JSON keys，两个 Rust tests 遍历全部 rows，并写 `SOURCE_MANIFEST_SHA256` 保存 Go manifest SHA-256。Rust 不重排/normalize fixture JSON，不定义第二套 wire names。

- [ ] **Step 7: Commit**

```bash
git add internal/dto/workspace_v2_fixtures_test.go testdata/workspace-sync-v2
git commit -m "test(sync): publish workspace v2 wire fixtures"
```

### Phase 2.7: Write Protocol Authority

**Files:**
- Create: `docs/workspace-sync-v2.md`

**Consumes:** locked contract and green fixtures。

**Produces:** Task 2 state/blob boundary、Task 3 route/transport boundary、Task 4 Rust contract。

- [ ] **Step 1: Write normative sections**

必须包含 scope/non-goals、endpoint/Bearer upgrade auth、first-pipe framing、request/response/push/error envelopes、primitives、15 full action tables and JSON examples、exact 25-flow factory registry、BlobNeed required-null、download BlobEnd new-requestId acknowledgement、frozen counted-conflict snapshot/incremental grammar、mutation/pending-resolve idempotency、merged-conflict missing-blob 唯一序列、conflicts、BLAKE3 dependency、fixed 64-byte binary header、upload/download ordering、numeric limits/timeouts、close/error behavior、v1 isolation、fixture governance。stream 规范必须写 required conflictCount、full entries 后 authoritative ConflictCreated set、incremental ordered `WorkspaceEvent|WorkspaceConflictResolved` revision-item union 后 authoritative set、checked deliveredCount、Event-only index，以及两个 conflict action 无 stream fields。conflict 规范必须把 `WorkspaceConflictRevision` 定为 positive opaque equality-only guard，禁止 ordering/Ack/retention/hub-tree keys，并写明 create 不推进任何 tree/path/global revision 且无 revision item，resolve 重验 source/rename-target drift、只推进一次 tree revision、写 tagged resolved item、绝不合成 mutation/event。merged section 必须逐步写出 persist pending → correlated blob_required failure → BlobNeed push，断线重连的 missing/existing/stale 三分支，以及 pending 24h TTL（expiry 只删 pending，不改 conflict/blob，允许新 operationId 重试）。

- [ ] **Step 2: Name Task 2 exact consumes**

Task 2 消费：`github.com/zeebo/blake3 v0.2.4`、`WorkspaceRevision`、独立 opaque `WorkspaceConflictRevision`、`WorkspaceContentHash`、`WorkspacePath`、Present-aware nullable types、`WorkspaceFileMetadata`、`WorkspacePathState`、`WorkspaceMutation`、Accepted/Rejected/Event、三个 BlobNeed direction-specific DTO、BlobBegin/BlobEnd/header、全部 conflict messages。Conflict create 不分配 tree/path/global revision、不写 revision log；resolve 原子重验 source 与 rename target drift，只分配一次 tree revision并写 tagged `WorkspaceConflictResolved` revision item，不造 synthetic mutation/event。重复终态 `(clientId,operationId)` 必须返回原 result，不分配第二 revision；merged blob 缺失先持久化 pending 并终结当前 request（correlated `blob_required`, retryable=false），精确重发继续 pending。blob existing 时 resolve；stale 时 correlated failure + pending delete + orphan GC；24h expiry 仅删 pending，client 用新 operationId 重试。

Task 3 消费：Phase 2.2 的 envelopes/errors/`WorkspaceV2Actions`，以及 Phase 2.6 registry subcycle 在全部 DTO 就绪后产出的 `WorkspaceV2DataFactory`、`WorkspaceV2ActionSpec`、`WorkspaceV2ActionSpecs`、`NewWorkspaceV2Data`、`DecodeWorkspaceV2Data`。router 对 concrete data 只能调用 `DecodeWorkspaceV2Data`，不得直接调用 internal strict decoder 或复制 action/flow switch；拿到 concrete pointer 后再由 Task 3/service 在具备 previous revision/conflict state 时调用相应验证。

- [ ] **Step 3: Name Task 4 exact consumes**

Canonical source 和 Rust destination/test paths 使用 Phase 2.6 exact paths；要求 byte parity、serde decode/re-encode parity。tree/path revision 是 string-backed Rust newtype，conflict revision 是不同的 positive equality-only newtype；hash/path 是 validated newtypes；required-null 必须区分 missing/null；Rust 从 payloadHex 独立重算 full BLAKE3 并验证 header first16，binary offsets 与 Go 完全一致。

- [ ] **Step 4: Verify**

```bash
GOMAXPROCS=1 go test -p 1 ./internal/dto -count=1
```

Expected: PASS。此处只验证 Worktree B 自己拥有的 DTO、fixtures 与文档契约。

- [ ] **Step 5: Commit**

```bash
git add docs/workspace-sync-v2.md
git commit -m "docs(sync): specify workspace sync v2 protocol"
```
