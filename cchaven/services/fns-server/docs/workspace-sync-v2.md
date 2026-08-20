# Workspace Sync v2 Wire Protocol

This document is the normative wire authority for workspace sync v2. The words **MUST**, **MUST NOT**, **REQUIRED**, **SHOULD**, and **MAY** are normative.

## Scope and non-goals

Workspace sync v2 synchronizes a workspace path tree, revisioned mutations, content-addressed blobs, and explicit conflicts between one authenticated server and desktop clients. It defines the WebSocket wire contract shared by the Go server and Rust client.

This contract does not define database tables, merge algorithms, secure filesystem joining, blob garbage-collection implementation, UI behavior, or route implementation. DTO validation is lexical; the service MUST still secure-join paths and reject symlinks escaping the workspace root.

The existing v1 endpoint and behavior are isolated. Implementations MUST NOT change `GET /api/user/sync`, its `Authorization|<JWT>` action, v1 DTOs, v1 binary frames, or `docs/ws_api.md`. V2 has no `Authorization` or `ClientInfo` action.

## Endpoint and upgrade authentication

- Endpoint: `GET /api/user/workspace-sync/v2`.
- The HTTP upgrade request MUST contain `Authorization: Bearer <token>`.
- The server MUST apply the existing token scope, client, IP, and user-agent checks before upgrading.
- Missing or invalid authentication fails before upgrade with HTTP 401; authenticated but unauthorized access fails with HTTP 403.
- Tokens, file contents, and absolute paths outside the workspace MUST NOT appear in wire errors or logs.

## Text framing

A control frame is UTF-8 text in this form:

```text
Action|JSON
```

The receiver splits on the first ASCII `|` only. The action is one of the exact 15 names below. A complete control frame, including action, separator, and JSON, is at most 65,536 bytes. Registered action names are at most 64 bytes.

JSON MUST be one object and MUST reject trailing data, duplicate keys at any nesting depth, unknown fields, NaN/Infinity, numeric revisions, and every omitted struct key not tagged `omitempty`. Required-key and nullability checks recurse through envelopes, concrete data, nested structs, and struct elements in arrays/slices. JSON null is legal only for Present-aware nullable values, required non-`omitempty` pointer fields that model required-null wire values, and `json.RawMessage` leaves. Scalars, structs, slices/arrays, maps, and a present `omitempty` pointer are non-null; optional means the key may be omitted, not explicitly null. An unknown action token is safe to echo once only when it matches ASCII `[A-Za-z][A-Za-z0-9]{0,63}`; echoing it in an error does not register a sixteenth action.

Missing separators, invalid/non-UTF-8 action framing, and unsafe action tokens close with WebSocket 1002. Oversized frames close with 1009. A request-level failure for a safely parsed action uses the received action token and a failure envelope.

## Envelopes and flows

The registry exposes exactly three flows:

| Flow | Sender | Envelope |
|---|---|---|
| `client_request` | client | request with a fresh `requestId` |
| `server_response` | server | correlated response echoing that `requestId` |
| `server_push` | server | success push without `requestId` |

Request:

```json
{"requestId":"10000000-0000-4000-8000-000000000001","data":{}}
```

Correlated success:

```json
{"requestId":"10000000-0000-4000-8000-000000000001","status":true,"data":{}}
```

Push:

```json
{"status":true,"data":{}}
```

Failure:

```json
{"requestId":"10000000-0000-4000-8000-000000000001","status":false,"error":{"code":"invalid_path","message":"path must be a canonical workspace-relative POSIX path","retryable":false,"fields":[{"field":"data.path","reason":"invalid_segment"}]}}
```

When `status=true`, `data` is REQUIRED and `error` is forbidden. When `status=false`, `error` is REQUIRED and `data` is forbidden. Pushes are always successful and never carry failure envelopes. Only a failure that cannot decode a request ID may omit it.

Stable error codes are:

```text
invalid_frame invalid_json unknown_action unauthenticated forbidden
invalid_request invalid_revision invalid_hash invalid_path
workspace_not_found workspace_limit_exceeded client_not_registered
stale_base_revision operation_reused blob_required blob_not_found
blob_hash_mismatch blob_size_mismatch blob_transfer_out_of_order
blob_limit_exceeded conflict_not_found conflict_revision_stale
server_busy internal
```

Only `server_busy` and `internal` are retryable. `blob_required` is explicitly `retryable:false`; the client advances through the upload state machine instead of blindly retrying.

## Primitive values

| Type | Wire representation and validation |
|---|---|
| `WorkspaceRevision` | JSON string; canonical decimal `0|[1-9][0-9]{0,19}`, at most `18446744073709551615` |
| `WorkspaceConflictRevision` | distinct opaque equality guard; JSON string; canonical positive decimal `[1-9][0-9]{0,19}`, at most `18446744073709551615` |
| workspace/client/operation/request/stream/transfer/conflict ID | lowercase canonical UUID string |
| `WorkspaceContentHash` | `blake3:` followed by exactly 64 lowercase hexadecimal characters |
| `WorkspacePath` | NFC UTF-8, 1–4096 bytes, workspace-relative POSIX path |
| `WorkspaceFileMetadata` | exact object `size`, `modifiedAtMs`, `executable` |

Paths forbid leading/trailing `/`, `//`, empty/`.`/`..` segments, backslash, NUL, C0/C1 controls, Windows drive/UNC forms, `< > : " | ? *`, segment suffix space/dot, and case-insensitive Windows device basenames `CON PRN AUX NUL COM1..COM9 LPT1..LPT9`, including before an extension.

Metadata requires `size<=5368709120` and `modifiedAtMs` in `0..253402300799999`. Directory, delete, and tombstone metadata use size 0 and `executable:false`.

`WorkspaceNullableHash`, `WorkspaceNullableUUID`, and `WorkspaceNullableUint64` distinguish an omitted key (`Present=false`) from explicit JSON null (`Present=true, Value=nil`). A REQUIRED-null key MUST be present and null; missing and non-null values are separate validation errors.

Entry kinds are `file|directory|symlink|tombstone`. Mutation kinds are `upsert_file|mkdir|upsert_symlink|delete|rename`. File/symlink/merged content hashes are non-null; directory/delete/tombstone hashes are null while the key remains present.

## Action and factory registry

`WorkspaceV2Actions` and `WorkspaceV2ActionSpecs` are the only Go action/flow registries. Transport and fixture code MUST call `NewWorkspaceV2Data(action, flow)` and MUST NOT duplicate a switch or define aliases.

`DecodeWorkspaceV2Data(action,flow,data)` calls `NewWorkspaceV2Data` and strict-decodes into that one registered concrete pointer. Registry failures remain typed `action:unknown_action` or `flow:flow_not_allowed`; duplicate/unknown/trailing/required-key/nullability failures normalize to typed `data:invalid_json`. It returns the concrete pointer without state-dependent validation.

`EncodeWorkspaceV2Response(action,response)` infers `server_push` for a successful response without `requestId` and `server_response` for one with `requestId`; the registry MUST allow that flow and the concrete `data` pointer type MUST equal its factory type. A failure is encodable for every registered action so an illegally received server-only action can still get a stable same-action failure envelope. `EncodeWorkspaceV2UnknownActionFailure(receivedAction,requestID)` is the only unknown-action echo path: it accepts only the safe ASCII grammar above, rejects registered actions, emits one `unknown_action` failure envelope, and never mutates either registry.

| # | Action | Legal flows | Concrete data type |
|---:|---|---|---|
| 1 | `WorkspaceHello` | client request, server response | `WorkspaceHelloRequest`, `WorkspaceHelloResponse` |
| 2 | `WorkspaceSubscribe` | client request | `WorkspaceSubscribeRequest` |
| 3 | `WorkspaceSnapshotBegin` | server push | `WorkspaceSnapshotBeginMessage` |
| 4 | `WorkspaceSnapshotEntry` | server push | `WorkspaceSnapshotEntryMessage` |
| 5 | `WorkspaceSnapshotEnd` | server push | `WorkspaceSnapshotEndMessage` |
| 6 | `WorkspaceMutation` | client request | `WorkspaceMutation` |
| 7 | `WorkspaceMutationAccepted` | server response | `WorkspaceMutationAcceptedMessage` |
| 8 | `WorkspaceMutationRejected` | server response | `WorkspaceMutationRejectedMessage` |
| 9 | `WorkspaceEvent` | server push | `WorkspaceEventMessage` |
| 10 | `WorkspaceAck` | client request, server response | `WorkspaceAckRequest` |
| 11 | `WorkspaceBlobNeed` | all three | upload push / download request / download response DTO |
| 12 | `WorkspaceBlobBegin` | all three | `WorkspaceBlobBeginMessage` |
| 13 | `WorkspaceBlobEnd` | all three | `WorkspaceBlobEndMessage` |
| 14 | `WorkspaceConflictCreated` | server push | `WorkspaceConflictCreatedMessage` |
| 15 | `WorkspaceConflictResolved` | all three | resolve request / resolved message DTO |

No other action token is part of v2.

The frozen registry therefore remains exactly 15 actions and 25 legal action/flow mappings. Counted conflict phases reuse actions 14 and 15; they add no action or flow.

## Action schemas

### 1. WorkspaceHello

| Flow | Required data fields | Cross-field rules |
|---|---|---|
| client request | `protocolVersion`, `clientId`, `clientVersion`, `capabilities` | version is `"2"`; capabilities are `binary_chunks`, `conflicts`, `snapshot_v1` |
| server response | `protocolVersion`, `serverVersion`, all negotiated numeric limits | values match the fixed limits below |

```json
{"requestId":"10000000-0000-4000-8000-000000000001","data":{"protocolVersion":"2","clientId":"10000000-0000-4000-8000-000000000002","clientVersion":"1.0.0","capabilities":["binary_chunks","conflicts","snapshot_v1"]}}
```

### 2. WorkspaceSubscribe

| Flow | Required data fields | Cross-field rules |
|---|---|---|
| client request | `workspaceId`, `clientId`, `lastAckRevision` | success begins a snapshot stream; no subscribe-ack action exists |

```json
{"requestId":"10000000-0000-4000-8000-000000000001","data":{"workspaceId":"10000000-0000-4000-8000-000000000002","clientId":"10000000-0000-4000-8000-000000000003","lastAckRevision":"0"}}
```

Only one subscription stream may be active on a connection. Subscribing before the current stream ends fails with `invalid_request`.

### 3. WorkspaceSnapshotBegin

| Flow | Required data fields | Cross-field rules |
|---|---|---|
| server push | `workspaceId`, `streamId`, `mode`, `fromRevision`, `finalRevision`, `entryCount`, `eventCount`, `conflictCount` | all three counts are required `uint32`; snapshot has `eventCount=0`; incremental has `entryCount=0`; final is not before from |

```json
{"status":true,"data":{"workspaceId":"10000000-0000-4000-8000-000000000002","streamId":"10000000-0000-4000-8000-000000000003","mode":"snapshot","fromRevision":"0","finalRevision":"7","entryCount":1,"eventCount":0,"conflictCount":1}}
```

### 4. WorkspaceSnapshotEntry

| Flow | Required data fields | Cross-field rules |
|---|---|---|
| server push | `workspaceId`, `streamId`, `index`, `entry` | index starts at 0 and is contiguous; entry has exact `path,pathRevision,kind,contentHash,metadata,tombstone` |

```json
{"status":true,"data":{"workspaceId":"10000000-0000-4000-8000-000000000002","streamId":"10000000-0000-4000-8000-000000000003","index":0,"entry":{"path":"notes/café.md","pathRevision":"1","kind":"file","contentHash":"blake3:abababababababababababababababababababababababababababababababab","metadata":{"size":3,"modifiedAtMs":1,"executable":false},"tombstone":false}}}
```

`kind=tombstone` if and only if `tombstone=true`.

### 5. WorkspaceSnapshotEnd

| Flow | Required data fields | Cross-field rules |
|---|---|---|
| server push | `workspaceId`, `streamId`, `mode`, `deliveredCount`, `finalRevision` | identity/mode/final match Begin; delivered count equals the selected entry/event count plus `conflictCount` without `uint32` overflow |

```json
{"status":true,"data":{"workspaceId":"10000000-0000-4000-8000-000000000002","streamId":"10000000-0000-4000-8000-000000000003","mode":"snapshot","deliveredCount":2,"finalRevision":"7"}}
```

The client may acknowledge `finalRevision` only after receiving End.

### 6. WorkspaceMutation

| Flow | Required data fields | Nullable/conditional fields |
|---|---|---|
| client request | `workspaceId`, `clientId`, `operationId`, `path`, `basePathRevision`, `kind`, `contentHash`, `metadata` | rename additionally requires `newPath,targetBasePathRevision`; other kinds forbid them |

```json
{"requestId":"10000000-0000-4000-8000-000000000001","data":{"workspaceId":"10000000-0000-4000-8000-000000000002","clientId":"10000000-0000-4000-8000-000000000003","operationId":"10000000-0000-4000-8000-000000000004","path":"notes/a.md","basePathRevision":"1","kind":"rename","contentHash":"blake3:abababababababababababababababababababababababababababababababab","metadata":{"size":3,"modifiedAtMs":1,"executable":false},"newPath":"archive/a.md","targetBasePathRevision":"0"}}
```

Source and target differ. A directory cannot move into its own descendant. Upsert file/symlink requires a hash; mkdir/delete requires null.

### 7. WorkspaceMutationAccepted

| Flow | Required data fields | Conditional fields |
|---|---|---|
| server response | `workspaceId`, `clientId`, `operationId`, `revision`, `pathState` | rename carries both `oldPathState` and `newPathState` |

```json
{"requestId":"10000000-0000-4000-8000-000000000001","status":true,"data":{"workspaceId":"10000000-0000-4000-8000-000000000002","clientId":"10000000-0000-4000-8000-000000000003","operationId":"10000000-0000-4000-8000-000000000004","revision":"2","pathState":{"path":"notes/a.md","pathRevision":"2","kind":"file","contentHash":"blake3:abababababababababababababababababababababababababababababababab","metadata":{"size":3,"modifiedAtMs":1,"executable":false},"tombstone":false}}}
```

### 8. WorkspaceMutationRejected

| Flow | Required data fields | Nullable rules |
|---|---|---|
| server response | `workspaceId`, `clientId`, `operationId`, `reason`, `currentPathState`, `conflictId`, `requiredHash` | reason is `stale_base_revision|operation_reused|blob_required|conflict_created` |

```json
{"requestId":"10000000-0000-4000-8000-000000000001","status":true,"data":{"workspaceId":"10000000-0000-4000-8000-000000000002","clientId":"10000000-0000-4000-8000-000000000003","operationId":"10000000-0000-4000-8000-000000000004","reason":"blob_required","currentPathState":null,"conflictId":null,"requiredHash":"blake3:abababababababababababababababababababababababababababababababab"}}
```

Only `conflict_created` has a non-null conflict ID. Only `blob_required` has a non-null required hash. This action is a successful business result; malformed frames use a failure envelope.

### 9. WorkspaceEvent

| Flow | Required data fields | Cross-field rules |
|---|---|---|
| server push | `workspaceId`, `streamId`, `index`, `revision`, `operationId`, `originClientId`, `mutation`, `pathState` | index contiguous within the Event subsequence; tree revision strictly increases across all revision items; nested identities match; rename carries old/new pair |

```json
{"status":true,"data":{"workspaceId":"10000000-0000-4000-8000-000000000002","streamId":"10000000-0000-4000-8000-000000000003","index":1,"revision":"2","operationId":"10000000-0000-4000-8000-000000000004","originClientId":"10000000-0000-4000-8000-000000000003","mutation":{"workspaceId":"10000000-0000-4000-8000-000000000002","clientId":"10000000-0000-4000-8000-000000000003","operationId":"10000000-0000-4000-8000-000000000004","path":"notes/a.md","basePathRevision":"1","kind":"upsert_file","contentHash":"blake3:abababababababababababababababababababababababababababababababab","metadata":{"size":3,"modifiedAtMs":1,"executable":false}},"pathState":{"path":"notes/a.md","pathRevision":"2","kind":"file","contentHash":"blake3:abababababababababababababababababababababababababababababababab","metadata":{"size":3,"modifiedAtMs":1,"executable":false},"tombstone":false}}}
```

### 10. WorkspaceAck

| Flow | Required data fields | Cross-field rules |
|---|---|---|
| client request and correlated response | `workspaceId`, `clientId`, `revision` | revision moves forward and cannot exceed the latest fully delivered End revision |

```json
{"requestId":"10000000-0000-4000-8000-000000000001","data":{"workspaceId":"10000000-0000-4000-8000-000000000002","clientId":"10000000-0000-4000-8000-000000000003","revision":"2"}}
```

### 11. WorkspaceBlobNeed

| Flow | Concrete DTO and exact fields | Nullability |
|---|---|---|
| server push | `WorkspaceBlobNeedUploadPush`: `workspaceId,direction,operationId,contentHash,size` | direction is `upload`; operation ID is non-null |
| client request | `WorkspaceBlobNeedDownloadRequest`: same five keys | direction is `download`; `operationId:null` and `size:null` are REQUIRED keys |
| server response | `WorkspaceBlobNeedDownloadResponse`: same five keys | echoes `operationId:null`, supplies actual size |

```json
{"requestId":"10000000-0000-4000-8000-000000000001","data":{"workspaceId":"10000000-0000-4000-8000-000000000002","direction":"download","operationId":null,"contentHash":"blake3:abababababababababababababababababababababababababababababababab","size":null}}
```

Missing REQUIRED-null keys fail with `required_key_missing`; non-null values fail with `must_be_null`. A missing download blob returns a correlated `blob_not_found` failure before any BlobBegin.

### 12. WorkspaceBlobBegin

| Flow | Required data fields | Cross-field rules |
|---|---|---|
| client request, server response, or server push | `workspaceId`, `transferId`, `direction`, `contentHash`, `size`, `chunkSize`, `chunkCount` | direction is upload/download; chunk size is 1,048,576; count is ceiling(size/chunkSize), or 0 for zero bytes |

```json
{"requestId":"10000000-0000-4000-8000-000000000001","data":{"workspaceId":"10000000-0000-4000-8000-000000000002","transferId":"10000000-0000-4000-8000-000000000005","direction":"upload","contentHash":"blake3:abababababababababababababababababababababababababababababababab","size":8,"chunkSize":1048576,"chunkCount":1}}
```

Upload binary data is forbidden until the server's correlated BlobBegin success.

### 13. WorkspaceBlobEnd

| Flow | Required data fields | Direction protocol |
|---|---|---|
| all three | `workspaceId`, `transferId`, `direction`, `contentHash`, `size`, `chunkCount` | transfer/hash/size/count are identical throughout confirmation |

```json
{"status":true,"data":{"workspaceId":"10000000-0000-4000-8000-000000000002","transferId":"10000000-0000-4000-8000-000000000005","direction":"download","contentHash":"blake3:abababababababababababababababababababababababababababababababab","size":7,"chunkCount":1}}
```

Upload uses client request then correlated server response. Download uses server push, then the client validates the complete blob and sends a **new client request with a new request ID** containing byte-equivalent data and `direction:"download"`; the server then sends its correlated success. The client MUST NOT treat the server push as a request requiring a direct response.

### 14. WorkspaceConflictCreated

| Flow | Required data fields | Side schema |
|---|---|---|
| server push | `workspaceId`, `conflictId`, `conflictRevision`, `path`, `kind`, `ancestor`, `current`, `incoming`, `createdByOperationId` | each side has `path|null,pathRevision,contentHash|null,metadata,tombstone` |

```json
{"status":true,"data":{"workspaceId":"10000000-0000-4000-8000-000000000002","conflictId":"10000000-0000-4000-8000-000000000005","conflictRevision":"42","path":"notes/a.md","kind":"delete_modify","ancestor":{"path":"notes/a.md","pathRevision":"3","contentHash":"blake3:abababababababababababababababababababababababababababababababab","metadata":{"size":3,"modifiedAtMs":1,"executable":false},"tombstone":false},"current":{"path":"notes/a.md","pathRevision":"6","contentHash":"blake3:abababababababababababababababababababababababababababababababab","metadata":{"size":3,"modifiedAtMs":1,"executable":false},"tombstone":false},"incoming":{"path":null,"pathRevision":"5","contentHash":null,"metadata":{"size":0,"modifiedAtMs":0,"executable":false},"tombstone":true},"createdByOperationId":"10000000-0000-4000-8000-000000000004"}}
```

Kinds are `content|delete_modify|rename|binary`. Content/binary sides are live files at the conflict path; delete/modify has one tombstone; rename has distinct live paths.

`conflictRevision` is a positive, opaque, equality-only conflict-generation guard. It is not a `WorkspaceRevision` and MUST NOT be ordered, incremented arithmetically by protocol consumers, acknowledged, used as a retention floor, or used as a hub/tree revision key. The three conflict DTOs use the distinct public `WorkspaceConflictRevision` type so Go consumers cannot accidentally apply numeric revision operations.

Creating or refreshing a conflict advances no global, path, or tree revision and writes no revision item. The conflict record stores explicit path revisions independently of `conflictRevision`; coincident decimal spellings have no semantic relationship.

`WorkspaceConflictCreated` has no `streamId` or `index`. During the counted conflict phase its position is established by the active stream and `conflictCount`; outside that phase the same DTO/action remains an ordinary live push.

### 15. WorkspaceConflictResolved

| Flow | Required data fields | Choice rules |
|---|---|---|
| client request | `workspaceId`, `clientId`, `operationId`, `conflictId`, `conflictRevision`, `choice`, `path`, `contentHash`, `metadata` | current/incoming replay that exact side; merged requires hash; delete requires null hash and zero metadata |
| server response/push | `workspaceId`, `conflictId`, `conflictRevision`, `operationId`, `revision`, `choice`, `pathState`, `resolvedByClientId` | response is correlated; subscriber push has no request ID |

```json
{"requestId":"10000000-0000-4000-8000-000000000001","data":{"workspaceId":"10000000-0000-4000-8000-000000000002","clientId":"10000000-0000-4000-8000-000000000003","operationId":"10000000-0000-4000-8000-000000000004","conflictId":"10000000-0000-4000-8000-000000000005","conflictRevision":"42","choice":"merged","path":"notes/a.md","contentHash":"blake3:abababababababababababababababababababababababababababababababab","metadata":{"size":8,"modifiedAtMs":2,"executable":false}}}
```

An unequal conflict-revision guard returns a correlated `conflict_revision_stale` failure before choice validation. Here “stale” means equality mismatch only, never numeric older/newer ordering.

Immediately before committing a resolution, the service MUST revalidate the conflict source state and, for a rename conflict, the recorded rename-target state; source or target drift invalidates that conflict generation and MUST NOT be applied. A successful resolution advances the tree revision exactly once, updates the resulting path state at that explicit `WorkspaceRevision`, and writes one tagged `WorkspaceConflictResolved` revision item. Its server response and incremental projection use the existing `WorkspaceConflictResolved` DTO/action. It MUST NOT create a synthetic `WorkspaceMutation`, `WorkspaceMutationAccepted`, or `WorkspaceEvent`.

`WorkspaceConflictResolved` has no `streamId` or `index`. During an incremental stream, revision-item position and ordering come from the active stream phase plus its `revision`; outside the stream it remains an ordinary live push.

## Snapshot and incremental ordering

Full snapshot:

```text
SnapshotBegin(snapshot) -> exactly entryCount SnapshotEntry (index 0..n-1,
path UTF-8 byte order) -> exactly conflictCount WorkspaceConflictCreated
(canonical conflictId UTF-8 byte order) -> SnapshotEnd -> Ack(finalRevision)
```

Incremental:

```text
SnapshotBegin(incremental) -> exactly eventCount ordered revision items
(strictly increasing tree revision; each item is either WorkspaceEvent for an
accepted mutation or server-push WorkspaceConflictResolved for a conflict
resolution) -> exactly conflictCount WorkspaceConflictCreated (canonical
conflictId UTF-8 byte order) -> SnapshotEnd -> Ack(finalRevision)
```

`eventCount` counts both members of that incremental union. `WorkspaceEvent.index` remains contiguous within the `WorkspaceEvent` subsequence and does not count `WorkspaceConflictResolved` items; the stream phase counter separately counts every union item. A successful resolve retains its original `WorkspaceConflictResolvedRequest` operation body and is emitted with the existing server-push `WorkspaceConflictResolved` DTO. It MUST NOT be synthesized as a `WorkspaceMutation` or nested in a `WorkspaceEvent`. `WorkspaceConflictResolved` and counted `WorkspaceConflictCreated` keep their existing bodies and gain no stream fields.

For full mode, `deliveredCount = entryCount + conflictCount`; for incremental mode, `deliveredCount = eventCount + conflictCount`. The addition is checked before comparison and an unrepresentable `uint32` total is invalid. Receiving fewer or more items, a wrong action in either phase, non-canonical conflict ordering, or a mismatched End invalidates the stream.

Begin through End is one read snapshot. The `conflictCount` pushes are the complete authoritative pending-conflict set from the same read snapshot as the selected entries/revision items and `finalRevision`. A client MUST validate the exact counts, phases, revision/conflict ordering, and matching End, then durably persist the completed snapshot before deleting locally known pending conflicts absent from that set. On interruption or validation failure it MUST retain the previous authoritative local set and discard the partial replacement.

No live out-of-stream workspace notification may interleave between Begin and End. Implementations MUST buffer concurrently created or resolved live pushes and deliver them only after End; those buffered pushes are not included in either count. Binary transfers MAY interleave by transfer ID because they are not workspace notifications and do not affect stream counters.

## Mutation and resolution idempotency

`operationId` is permanently unique within a client. The service keys terminal mutation/resolve results by `(clientId,operationId)`. An exact repeat of a terminal operation returns the original accepted/resolved payload and original revision, allocates no second revision, and does not change its JSON type. Reusing an operation ID with different canonical data yields `operation_reused`.

A merged resolve whose blob is missing is pending, not terminal. Its pending record is keyed by `(clientId,operationId)` and contains the canonical resolve-data digest, conflict revision, required content hash, `createdAt`, and `expiresAt=createdAt+24h`.

### Only valid missing-blob merged sequence

```text
1 client: WorkspaceConflictResolved request (choice=merged)
2 server: persist pending resolve
3 server: correlated WorkspaceConflictResolved failure
          code=blob_required, retryable=false (the request is now terminal)
4 server: WorkspaceBlobNeed(upload) push with the same operationId
5 client/server: BlobBegin request/success, binary chunks,
                  BlobEnd request/success
6 client: WorkspaceConflictResolved request with a new requestId and
          byte-equivalent data (same operationId/conflictRevision/path/hash/metadata)
7 server: atomically resolve and return correlated resolved success
```

The server MUST persist pending before step 3 and MUST send the correlated failure before BlobNeed. Missing-blob attempts do not enter the terminal idempotency table and therefore MUST NOT produce `operation_reused`.

After reconnect, an exact same-operation retry has three branches:

- blob still missing: return correlated `blob_required`, then repeat BlobNeed;
- blob exists and conflict revision remains current: resolve atomically;
- conflict revision is stale: return correlated `conflict_revision_stale`, then delete pending.

Pending TTL is exactly 24 hours. Expiry deletes only pending state; it does not modify a conflict or blob, and the client may retry using a new operation ID. An uploaded blob made unreferenced by a stale conflict is an orphan for normal grace-period GC, not synchronous deletion in resolve.

## Binary frames and BLAKE3

Go code uses `github.com/zeebo/blake3 v0.2.4`; Rust MUST use a compatible BLAKE3 implementation. Full content hashes are 32-byte BLAKE3 values rendered in the `blake3:<hex>` primitive. Each binary header stores the first 16 raw bytes of BLAKE3(payload). Receivers independently recompute both chunk and full-blob hashes.

The fixed header is exactly 64 bytes, with big-endian integers:

| Bytes | Field | Rule |
|---|---|---|
| 0..3 | magic | ASCII `FNS2` |
| 4 | version | `0x02` |
| 5 | direction | `0x01` upload, `0x02` download |
| 6 | flags | bit 0 final; bits 1..7 zero |
| 7 | header length | `0x40` |
| 8..23 | transfer ID | UUID raw 16 bytes |
| 24..31 | chunk index | uint64 |
| 32..39 | offset | uint64 |
| 40..43 | payload length | uint32, `1..1,048,576` for an actual frame |
| 44..47 | reserved | all zero |
| 48..63 | chunk digest | first 16 raw BLAKE3(payload) bytes |

The binary WebSocket frame length is exactly `64+payloadLength`. For a transfer, chunk index starts at 0 and is contiguous; offset equals `chunkIndex*chunkSize`; non-final chunks are full; final is set only on the last chunk. Duplicate/out-of-order chunks fail. A zero-byte blob has `chunkCount:0` in BlobBegin and BlobEnd and sends no binary frame; a constructed or received header with `payloadLength:0` is invalid. Empty-content BLAKE3 remains a digest/codec case only, never a valid wire-frame vector.

Upload order:

```text
BlobNeed(upload) push -> BlobBegin request -> BlobBegin success -> chunks
-> BlobEnd request -> BlobEnd success -> retry original Mutation/Resolve
```

Download order:

```text
BlobNeed(download) request -> BlobNeed success -> BlobBegin push -> chunks
-> BlobEnd push -> local validation -> BlobEnd acknowledgement request
(new requestId) -> BlobEnd correlated success
```

Temporary blob content MUST NOT become visible until hash, size, count, and order all pass.

## Limits and timeouts

| Limit | Value |
|---|---:|
| control frame | 65,536 bytes |
| binary chunk payload | 1,048,576 bytes |
| blob size | 5,368,709,120 bytes |
| active transfers per connection | 4 |
| active transfers per workspace | 16 |
| active transfers per user | 32 |
| heartbeat | 25 seconds |
| transfer idle expiry | 60 seconds |
| maximum transfer lifetime | 30 minutes |
| pending merged resolve TTL | 24 hours |

Limit exhaustion returns `blob_limit_exceeded` and MUST NOT evict an active transfer.

## Close and error behavior

- HTTP authentication failures occur before WebSocket upgrade.
- Unsafe framing closes with 1002; oversized frames close with 1009.
- Known and safely echoed unknown actions return a same-action failure envelope.
- Hash, size, or ordering failures terminate that transfer; a failed transfer ID cannot be reused.
- Business stale/conflict/blob state uses the specified non-retryable result or failure and state-machine transition.
- `server_busy` and `internal` messages are stable, safe English strings and are the only generally retryable failures.

## Go service consumption boundary

Task 2 consumes `github.com/zeebo/blake3 v0.2.4` and these exact protocol types: `WorkspaceRevision`, the distinct opaque `WorkspaceConflictRevision`, `WorkspaceContentHash`, `WorkspacePath`, all Present-aware nullable types, `WorkspaceFileMetadata`, `WorkspacePathState`, `WorkspaceMutation`, mutation accepted/rejected/event types, the three direction-specific BlobNeed DTOs, BlobBegin/BlobEnd/header APIs, and all conflict DTOs.

Task 2 implements persistence, secure joining, streaming hash calculation, terminal operation idempotency, pending resolve state, and blob GC without defining alternate wire structs. Exact terminal replay returns the original result/revision. Missing merged blobs follow the pending sequence above.

Task 3 consumes envelopes, stable errors, `WorkspaceV2Actions`, `WorkspaceV2ActionSpecs`, `NewWorkspaceV2Data`, `DecodeWorkspaceV2Data`, `EncodeWorkspaceV2Response`, and `EncodeWorkspaceV2UnknownActionFailure`. Its strict router MUST use `DecodeWorkspaceV2Data` for concrete wire data and the registry-checked encoders for output; it MUST NOT call an internal decoder, copy action/flow switches, or build an alternate unknown-action envelope. Task 3/service applies state-dependent validation after decoding when previous revision/conflict state is available.

## Cross-language fixture governance

The canonical fixture source is:

```text
fast-note-sync-service/testdata/workspace-sync-v2/
```

It contains a manifest plus six SHA-256-pinned data files. Go tests traverse every action/flow row, strict-decode the registered concrete type, validate, canonical re-encode, independently replay BLAKE3 from `payloadHex`, and compare exact binary header bytes. The control fixtures include counted-conflict full and incremental snapshot sequences; the incremental sequence contains `WorkspaceEvent -> WorkspaceConflictResolved -> WorkspaceEvent`, proving tree-revision ordering and the Event-only index subsequence, followed by an ordered pending-conflict set whose raw DTOs contain no stream fields. `invalid/paths.jsonl` has at least one cross-language row for every locked path category. Header fixtures contain no valid zero-payload frame; empty BLAKE3 is covered by the Go digest unit test instead.

Task 4 copies that directory byte-for-byte to:

```text
<rust-workspace>/crates/fns-protocol/tests/fixtures/workspace-sync-v2/
```

Rust tests are exactly:

```text
<rust-workspace>/crates/fns-protocol/tests/workspace_v2_fixtures.rs
<rust-workspace>/crates/fns-protocol/tests/workspace_v2_binary_header.rs
```

They MUST traverse every row, preserve JSON keys and required-null presence, decode/re-encode with byte parity, independently recompute full BLAKE3 and header first16, and store the Go manifest SHA-256 in `SOURCE_MANIFEST_SHA256`. Rust tree/path revisions are string-backed validated newtypes; conflict revision is a distinct positive equality-only newtype; hashes and paths are validated newtypes. Rust MUST NOT normalize/reorder fixture JSON or define a second set of wire names.
