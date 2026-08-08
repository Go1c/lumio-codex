# Task 9 implementation report

## Scope

Implemented the Task 9 inbound snapshot/incremental stream state machine in
`fns-sync-core`: validated stream identity and ordering, durable stream rows,
blob wait/replay, journaled filesystem application, local divergence
preservation, full-snapshot reconciliation, authoritative conflict replacement,
conflict-resolution postimage application, and durable Ack confirmation/replay.

The intended Task 9 files were changed. Three small compatibility extensions
are recorded explicitly:

- `src/effect.rs`: added the compile-required `DownloadBlob` and `SendAck`
  command variants consumed by the Task 9 engine/tests.
- `src/state.rs`: added the typed state accessors required by the new stream
  journal/conflict/cursor paths.
- `tests/local_mutations.rs`: added unreachable match arms for the two new
  command variants; existing mutation assertions and behavior are unchanged.

## TDD evidence

- RED: the new `remote_stream` test target was run before implementation and
  failed at compile time with 76 missing API/command errors.
- First GREEN iteration exposed 9 behavioral failures; each was traced to
  stream ordering, repeated blob delivery, conflict replacement, Ack timing,
  snapshot ordering, or preserved-state handling.
- The independent review also exposed missing conflict-resolution materialization
  and no-Ack stream cleanup; both now have production fixes and focused coverage.
- Final focused GREEN:

  `cargo test --locked -p fns-sync-core --test remote_stream`

  Result: 20 passed, 0 failed.

## Verification

- `cargo fmt --all -- --check` — PASS
- `cargo test --locked -p fns-sync-core --test local_mutations` — PASS (21)
- `cargo test --locked -p fns-sync-core --test remote_stream` — PASS (20)
- `cargo test --locked -p fns-sync-core` — PASS (21 + 20 + 23)
- `cargo check --locked --workspace` — PASS
- `cargo test --locked -p fns-fs` — PASS (9 + 33 + 24 + 5 + 11 + 16 + 4)
- `cargo clippy --locked --workspace --all-targets -- -D warnings` — PASS
- `cargo test --locked --workspace --quiet` — PASS, exit 0
- `git diff --check` — PASS

## Known gaps

- Runtime crash/restart execution across every journal stage is not exercised
  by this focused Task 9 delivery. Startup now removes prepared journals and
  finalizes postimage-complete journals, but partial rename recovery and
  synthetic crash-stage coverage remain untested.
- No Windows runtime execution was performed in this task; workspace tests
  and the existing cross-platform source contracts remain unchanged.
- No commit, merge, push, PR, or deployment has been performed.

## Status

Implementation and local verification are complete; independent reviewer
approval is still required before commit or merge.
