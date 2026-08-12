# Task 5.7 implementation report

## Scope and authority

Implemented the SQLite client-state and transaction layer in the isolated
`remote-mvp-task5-sync-core-v2` worktree at the clean Rust dev baseline
`8a9034d36dc18810d4744aaaaf993dc97414a889`. Only the Task 5.7 paths were
changed: `crates/fns-sync-core/src/{lib.rs,error.rs,model.rs,state.rs,store.rs,ids.rs}`,
the exact schema migration, the deterministic store-test support module, and
`tests/store.rs`. `crates/fns-fs` was not changed; no Go source, retired
fingerprint, protocol fixture, push, PR, or deployment was touched. The
normative Go authority remains the requested `ba4caa45bb766dc4f1bc983e134d6b272a70cd05`
with manifest SHA
`86f52715e7827ac99873850961ee84ffd99610a5f0009b16033d5706b18f9e7e`.

## Implementation

- Added `migrations/0001_client_state.sql` with the requested schema version 1
  and all cursor, path, outbox, local-intent, stream, revision-item,
  stream-conflict, apply-journal, applied-operation, conflict, and hash-cache
  tables and constraints.
- Added `SqliteState::open` using read/write/create/full-mutex SQLite flags.
  Every connection applies foreign keys, WAL journaling, FULL synchronous mode,
  a 5000 ms busy timeout, and 1000-page WAL auto-checkpointing. Version 0 is
  migrated in one immediate transaction; only version 1 is accepted after
  migration. Workspace/client identity is persisted and mismatches are
  rejected before use.
- Added typed cursor, path, outbox, local-intent, stream, stream-revision,
  stream-conflict, apply-journal, applied-operation, conflict, and hash-cache
  records with exact durable stage enums. Canonical DTO bytes come from
  `serde_json::to_vec`; stored digests are raw 32-byte BLAKE3 values.
- Added transactional outbox enqueue/replace, select-and-dispatch, stream
  begin/item persistence, apply journal, applied-operation receipt, conflict,
  cursor, and rollback APIs. Dispatched operation bodies cannot be replaced;
  permanent receipts accept only the original operation/revision/digest.
- Added validating row conversion through Task 4 IDs, revisions, paths,
  concrete DTO deserialization, and DTO validators. Corrupt cursor revisions
  return a safe `CorruptState` error. SQLite errors are classified as safe
  storage failures without propagating a database path or SQL statement.
- Implemented `fns_fs::HashCache` for `SqliteState`, persisting and validating
  the exact `FileFingerprint` JSON and content hash across reopen; fingerprint
  mismatches are cache misses, while database failures remain cache errors.

## TDD evidence

RED was run before the implementation with:

```text
cargo test -p fns-sync-core --test store
```

Compilation failed as expected because `SqliteState` and `SyncError` were not
yet defined (the initial test also reported closure type inference failures
that depended on those missing APIs).

GREEN and focused coverage:

```text
cargo test --locked -p fns-sync-core --test store
running 9 tests
test result: ok. 9 passed; 0 failed
```

The nine focused tests cover exact schema/pragmas/u64::MAX revision,
reopen persistence, identity mismatch, all-table rollback, dispatched-body
immutability, one active stream, permanent applied receipt, safe corrupt
revision handling, and persistent hash-cache behavior.

## Verification

The following final checks passed:

```text
cargo test --locked -p fns-sync-core --test store     # 9 passed
cargo test --locked -p fns-sync-core                  # unit 0, store 9, doctest 0 passed
cargo clippy --locked -p fns-sync-core --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
```

No changes are present under `crates/fns-fs`. The product commit hash is
reported separately after commit creation.

## Concern

The brief names `fns_fs::HashCacheError::Unavailable`, but the reviewed,
out-of-scope `fns-fs` implementation available at this baseline exposes only
`HashCacheError::{Io,Invalid}`. To honor the no-edit constraint, SQLite
failures map to the existing safe `HashCacheError::Io` variant; adding or
renaming an `fns-fs` variant would require an explicit scope decision.
