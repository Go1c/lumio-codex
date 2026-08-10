# Development Status

Status date: 2026-08-10

Development and release work are paused by the project owner. The current
client branch contains substantial bidirectional-sync work, but it has not met
the final acceptance criteria and must not be described or distributed as a
release candidate.

## Repository State at Pause

- Client repository: `/Users/cui/Sites/AI-Remote-Workspace/client`
- Client branch: `dev`
- Client base before the pause commit: `69c1be76befb4fd751155da7b2af88ca604399c3`
- Server repository:
  `/Users/cui/Sites/.worktrees/fast-note-sync-service/remote-claude-desktop-mvp-go`
- Server branch: `feature/remote-claude-desktop-mvp-go`
- Server commit: `39fadbfbfc62f26e4499b3cd27dbf403c0c9545a`

The commit containing this document is the handoff point. Use `git log -1` to
resolve its client commit instead of relying on an uncommitted diff hash.

## What Is Implemented

- Durable local and remote mutation handling for files, directories, deletes,
  renames, empty content, binary content, and streamed blobs.
- Explicit handling for BlobNeed, BlobBegin, BlobEnd, MutationAccepted,
  MutationRejected, Event, and Ack.
- Persistent outbox, pending intent, apply journal, cursor, Ack, duplicate
  receipt, duplicate blob, reconnect, and restart recovery paths.
- Conflict creation, preservation, listing, and durable resolution actions.
- SSH tunnel, real JWT credential lookup, WebSocket Hello, Subscribe, Snapshot,
  incremental events, and desktop auto-start/status wiring.
- Bounded stop, timeout, cancellation, cleanup, and observable error paths for
  the desktop process, Agent, Worker, SSH tunnel, and deployment commands.
- Project-specific systemd Agent state capture and rollback. The rollback now
  distinguishes absent, enabled/inactive, disabled/active, and
  disabled/inactive units instead of assuming every pre-existing Agent was
  enabled and active.

The protocol ownership and recovery chain is documented in
[`sync-event-chain.md`](sync-event-chain.md).

## Verification Completed

The following checks passed on 2026-08-10:

- Server `go test ./...` at server commit `39fadbfb`.
- Server `git diff --check` with a clean server worktree.
- Desktop source tests: 45 passed, 0 failed.
- Desktop production frontend build.
- Deployment unit tests after the final systemd rollback change: 18 passed,
  0 failed.
- Sync conflict tests before the final deployment-only change: 25 passed,
  0 failed.
- Remote stream tests before the final deployment-only change: 62 passed,
  0 failed.
- Real-service Run-06 completed with revision 19, both Agents reaped with exit
  code 0, empty pending runtime queues, stopped SSH controller, and verified
  evidence checksums.
- A packaged-app preflight used the real Keychain JWT and reached an online,
  quiescent revision-0 state against workspace 14. It then exited with its
  Worker and SSH processes cleaned up.

Evidence is machine-local and intentionally outside the Git repository:

- `/Users/cui/Sites/fns-workspace/acceptance-evidence/final-20260810`
- `/Users/cui/Sites/fns-workspace/acceptance-evidence/final-live-20260810`
- Run-06 result:
  `/Users/cui/Sites/fns-workspace/acceptance-evidence/final-live-20260810/controlled-e2e/final-controlled-20260810-06/result.json`
- Run-06 connection state:
  `/Users/cui/Sites/fns-workspace/acceptance-evidence/final-live-20260810/controlled-e2e-connection/final-controlled-20260810-06/state.json`
- Final rollback test log:
  `/Users/cui/Sites/fns-workspace/acceptance-evidence/final-20260810/deploy-tests-pause-check.log`

## Invalidated Results and Known Failures

- Diff candidate
  `33ece4727490488b249fac159b4f5907d7144884bfa7704f5ccb87c4baabf50d`
  is invalidated. It predates the project-specific Agent rollback fix.
- Run-06 is useful regression evidence but is not final release evidence because
  it ran before that fix. A fresh run must use a new workspace and binaries
  built from the eventual final commit.
- The complete required client commands have been rerun after the segment ack
  fix (see below). All pass: 663 tests passed, 0 failed, 2 ignored; clippy
  clean with `-D warnings`; `cargo fmt --check` clean; `git diff --check`
  clean.

- The saved `Test` project was diagnosed and the root cause fixed. The project
  was in a stopped `recovery_exhausted:core` state with `lastAckRevision=360`,
  `lastAppliedRevision=364`, `pending_ack_revision=NULL`, and a stream stalled
  at `from=360, final=383, end_received=0` with only 5 of 23 expected events
  arrived (4 applied, 1 ready). Root cause: the incremental-stream Ack model
  only set `pending_ack` at terminal stream completion
  (`finish_stream_if_ready`), so when the stream could not reach completion
  (e.g. SnapshotEnd lost, or a later item blocked), `last_ack` was permanently
  stranded and every reconnect re-subscribed and re-applied the same work. The
  fix adds a *segment ack* (`pending_segment_ack_revision`, migration 0005)
  that advances `last_ack` for a contiguous applied prefix when every expected
  event has arrived and been fully processed, without clearing the active
  stream. The `Test` project's SQLite state is preserved for reference; do not
  delete or rewrite it.
- No final current-commit macOS App or DMG exists. All bundles under `target/`
  predate the pause commit and are preview artifacts only.
- The old `/Applications/FNS Workspace.app` was an x86_64 build and was removed
  from `/Applications`. On this development Mac it is recoverable from
  `/Users/cui/.Trash/FNS Workspace-old-x86_64-20260810.app` until the Trash is
  emptied.
- Only an Apple Development signing identity is currently available. It is
  sufficient for local acceptance, but distribution to other Macs without a
  Gatekeeper warning requires a Developer ID certificate plus notarization and
  stapling.

## Resume Checklist

1. Confirm the client and server branches match the handoff commits and both
   worktrees are clean. Do not use destructive Git commands.
2. Reproduce and diagnose the saved `Test` project `core` failure without
   deleting its persisted outbox, pending intents, journal, cursor, or blobs.
3. Run the four exact client commands above and `go test ./...` plus
   `git diff --check` in the server repository.
4. Create a fresh real-service workspace 15 and run a new controlled E2E matrix
   from exact final binaries. Do not reuse workspace 14 for the deterministic
   matrix because its service revision is already 19.
5. Verify local-to-remote and remote-terminal-to-local create, modify, delete,
   rename, text, binary, empty, large, directory, nested directory, concurrent
   conflict, reconnect, Agent restart, and App restart behavior. Compare path,
   bytes, size, SHA-256, revision, Ack, outbox, pending intent, stream, journal,
   transfers, and observable error state for every case.
6. Commit the final tested code, build a Linux x86_64 Agent with provenance tied
   to that exact clean commit, then build and verify the arm64 App and DMG.
7. Run the packaged App against the real service, verify conflict UI resolution
   and process cleanup, then install it in `/Applications` and push the final
   commits.

## Credential and Signing Rules

- Authentication must continue to use a real scoped JWT from Keychain. Do not
  add an authentication bypass or place a JWT in command arguments, environment
  variables, logs, or evidence files.
- Until a clean final release build is intentionally started, cancel unexpected
  `security` password dialogs. During the final signed build, macOS may request
  permission for the signing process to use the private key. Never pass the
  login password through a shell command or environment variable.

At pause time there were no FNS Workspace, fns-agent, test-sync, controlled E2E,
or owned SSH processes left running.
