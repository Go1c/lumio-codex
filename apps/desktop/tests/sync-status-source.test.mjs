import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const appSource = await readFile(
  new URL("../src/App.tsx", import.meta.url),
  "utf8",
);
const workspaceSource = await readFile(
  new URL("../src/components/WorkspaceView.tsx", import.meta.url),
  "utf8",
);

test("project loading starts every configured sync without blocking the project list", () => {
  assert.match(appSource, /setProjects\(list\)/);
  assert.match(appSource, /list\.forEach/);
  assert.match(appSource, /start_sync/);
  assert.doesNotMatch(appSource, /await\s+Promise\.all/);
});

test("selected workspace polls real sync status with cancellation and a bounded delay", () => {
  assert.match(workspaceSource, /invoke<SyncStatus>\("sync_status"/);
  assert.match(workspaceSource, /setTimeout\([^,]+,\s*SYNC_POLL_INTERVAL_MS\)/s);
  assert.match(workspaceSource, /clearTimeout/);
  assert.match(workspaceSource, /cancelled\s*=\s*true/);
  assert.doesNotMatch(workspaceSource, /setSyncState\("synced"\)/);
});

test("sync operations and status failures stay visible and retryable", () => {
  assert.match(workspaceSource, /status\?\.message/);
  assert.match(workspaceSource, /status(?:\?|)\.running/);
  assert.match(workspaceSource, /status\?\.localPort/);
  assert.match(workspaceSource, /failure\.primary/);
  assert.match(workspaceSource, /failure\.cleanup/);
  assert.match(workspaceSource, /lastRefresh/);
  assert.match(workspaceSource, /Retry start/);
  assert.match(workspaceSource, /Stop sync/);
  assert.doesNotMatch(workspaceSource, /No sync conflicts/);
});
