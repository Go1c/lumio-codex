# Controlled real-service E2E

`controlled_ssh_e2e.py` runs the existing `test-sync` matrix with two local
WebSocket endpoints forwarded through one owned SSH connection to
the explicitly selected SSH host. Both forwards terminate at
`127.0.0.1:9000` on the remote host.

This validates two local Agent roots against the real remote service. It does
not by itself prove writes in the remote terminal workspace or restart the
packaged Tauri application. Those two acceptance checks must be run separately
against the remote filesystem and the actual `.app` process. The `app_restart`
effect below specifically restarts this driver's owned connection controller.

The driver does not read the JWT and does not accept it in an argument or an
environment variable. The caller must provide the read end of an anonymous
pipe as an inherited file descriptor. `test-sync` is the only process that
reads that descriptor.

## Build

```sh
cargo build --locked -p fns-agent --bin fns-agent -p test-sync --bin test-sync
```

## Run

The four workspace/state directories must be absolute, distinct, and empty.
The example assumes that the caller has already attached the JWT pipe to file
descriptor 3 without printing its contents.

Use a workspace without pre-existing unresolved conflicts and prevent
unrelated clients from writing during this deterministic matrix. Existing
workspace contents may be synchronized as a baseline, but unrelated conflicts
or concurrent mutations make the exact conflict checkpoint invalid.

```sh
scripts/test-sync/controlled_ssh_e2e.py run \
  --test-sync "$PWD/target/debug/test-sync" \
  --agent-binary "$PWD/target/debug/fns-agent" \
  --workspace-id 10000000-0000-4000-8000-000000000002 \
  --client-id-a CLIENT_UUID_A \
  --client-id-b CLIENT_UUID_B \
  --root-a /absolute/empty/root-a \
  --root-b /absolute/empty/root-b \
  --state-a /absolute/empty/state-a \
  --state-b /absolute/empty/state-b \
  --run-id REAL_REMOTE_RUN_ID \
  --evidence-root /absolute/private/e2e-evidence \
  --token-fd 3 \
  --ssh-host your-ssh-host
```

When omitted, `--evidence-root` defaults to `target/e2e-evidence` in the client
repository. The same resolved path is passed to `test-sync` and used by the
connection hook, so `connection.jsonl` is included before the harness writes
its checksums.

The SSH destination is always required. The remote listener defaults to
`127.0.0.1:9000` and can be overridden for a different environment:

```text
--ssh-host your-ssh-host
--remote-host 127.0.0.1
--remote-port 9000
```

## Effect proof

The harness invokes generated, private hooks and an independently pinned
observer:

- `reconnect` terminates and reaps the old SSH process, confirms both local
  listeners are closed, starts a replacement SSH process, confirms both
  forwards reach the remote listener, and only then advances generation. The
  controller PID stays unchanged.
- `app_restart` stops and reaps the SSH process and old controller, confirms
  the listeners are closed, then starts a new controller PID at the next
  generation.
- The observer reads the controller's atomically replaced state and verifies
  it against a live Unix-socket ping before returning its PID/generation.
- The hook receipt must exactly match the independent before/after observer
  results or `test-sync` fails.

`target/e2e-evidence/<run-id>/connection.jsonl` is captured before the harness
finalizes `SHA256SUMS`. It records controller and tunnel PIDs, generation
changes, bounded termination outcome, port-closure proof, and restored
readiness. Complete connection lifecycle evidence, including final cleanup,
is written separately to
`target/e2e-connection-evidence/<run-id>/` with its own `SHA256SUMS`.

All controller, SSH, hook, observer, and harness operations have explicit
timeouts. Normal completion, failure, timeout, SIGINT, and SIGTERM all enter a
bounded TERM/KILL/reap cleanup path.

## Offline regression test

This test substitutes an isolated localhost listener for SSH. It does not
connect to the VPS or read a JWT.

```sh
/usr/bin/python3 -m unittest discover \
  -s scripts/test-sync/tests \
  -p 'test_*.py' \
  -v
```
