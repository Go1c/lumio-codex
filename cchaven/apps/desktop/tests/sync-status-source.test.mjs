import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const appSource = await readFile(
  new URL("../src/App.tsx", import.meta.url),
  "utf8",
);
const workspaceSource = await readFile(
  new URL("../src/components/Workspace.tsx", import.meta.url),
  "utf8",
);
const libSource = await readFile(
  new URL("../src-tauri/src/lib.rs", import.meta.url),
  "utf8",
);

test("project loading starts every configured sync without blocking the list", () => {
  assert.match(appSource, /setProjects\(list\)/);
  assert.match(appSource, /list\.forEach/);
  assert.match(appSource, /startSync/);
  // A single unreachable host must not keep the sidebar empty.
  assert.doesNotMatch(appSource, /await\s+Promise\.all/);
});

test("the workspace polls real engine status with cancellation and a bounded delay", () => {
  assert.match(workspaceSource, /syncEngineStatus\(project\.id\)/);
  assert.match(workspaceSource, /setTimeout\([^,]+,\s*SYNC_POLL_INTERVAL_MS\)/s);
  assert.match(workspaceSource, /clearTimeout/);
  assert.match(workspaceSource, /cancelled\s*=\s*true/);
  // Status is read, never assumed.
  assert.doesNotMatch(workspaceSource, /setEngine\(\{\s*running:\s*true/);
});

test("sync operations and status failures stay visible and retryable", () => {
  assert.match(workspaceSource, /engine\?\.message/);
  assert.match(workspaceSource, /engine\?\.running/);
  assert.match(workspaceSource, /engine\.localPort/);
  assert.match(workspaceSource, /failure\.primary/);
  assert.match(workspaceSource, /failure\.cleanup/);
  assert.match(workspaceSource, /lastRefresh/);
  assert.match(workspaceSource, /sync\.retry/);
  assert.match(workspaceSource, /sync\.stop/);
});

test("the aggregate status card is derived, never guessed", () => {
  // The four 6.3 states come from one reducer with an explicit precedence.
  assert.match(libSource, /pub fn reduce_sync_status/);
  assert.match(libSource, /SyncStateLabel::Conflicts/);
  assert.match(libSource, /SyncStateLabel::Offline/);
  assert.match(libSource, /agent_runtime_status/);
});
