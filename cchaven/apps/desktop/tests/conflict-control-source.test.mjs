import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

// The conflict page is one view: the three human answers of 交互设计 5.5 on top
// of the engine's conflict control surface. These assertions guard the parts of
// that wiring which are easy to lose in a refactor.
const viewSource = await readFile(
  new URL("../src/components/ConflictsView.tsx", import.meta.url),
  "utf8",
);
const scopeSource = await readFile(
  new URL("../src/components/ConflictRequestScope.ts", import.meta.url),
  "utf8",
);
const libSource = await readFile(
  new URL("../src-tauri/src/lib.rs", import.meta.url),
  "utf8",
);
const workspaceSource = await readFile(
  new URL("../src/components/Workspace.tsx", import.meta.url),
  "utf8",
);
const apiSource = await readFile(
  new URL("../src/lib/api.ts", import.meta.url),
  "utf8",
);
const syncSource = await readFile(
  new URL("../src-tauri/src/sync.rs", import.meta.url),
  "utf8",
);
const bridgeSource = await readFile(
  new URL("../src-tauri/src/conflict_bridge.rs", import.meta.url),
  "utf8",
);

test("the conflicts tab is backed by the engine, not by reading its database", () => {
  assert.match(workspaceSource, /"conflicts"/);
  assert.match(workspaceSource, /<ConflictsView/);
  // The engine's raw conflict list is consumed by the Rust bridge and reaches
  // the page already projected onto the three human answers; the operation
  // control surface stays available to the UI.
  assert.match(apiSource, /"list_sync_conflict_operations"/);
  assert.match(apiSource, /"cancel_sync_conflict_generation"/);
  assert.match(apiSource, /"cancel_sync_conflict_request"/);
  assert.match(apiSource, /"resolve_conflict"/);
  assert.match(syncSource, /pub async fn list_sync_conflicts/);
  assert.match(syncSource, /pub async fn resolve_sync_conflict/);
  // The desktop must never reach around the agent into its own state file.
  assert.doesNotMatch(viewSource, /state\.sqlite|sqlite|openDatabase/);
});

test("every durable engine choice is reachable from a human answer", () => {
  // The user picks one of three; the bridge maps those onto the engine's four.
  for (const choice of ["Current", "Incoming", "Delete"]) {
    assert.match(bridgeSource, new RegExp(`WorkspaceConflictChoice::${choice}`));
  }
  assert.match(bridgeSource, /pub fn engine_choice/);
  assert.match(bridgeSource, /Resolution::KeepBoth/);
  // Every resolution carries the conflict's own revision, so a stale answer is
  // rejected by the engine rather than silently overwriting a newer one.
  assert.match(libSource, /conflict_id:\s*view\.conflict_id/);
  assert.match(libSource, /conflict_revision:\s*view\.conflict_revision/);
  assert.match(libSource, /identity: Option<sync::ConflictControlIdentity>/);
});

test("resolution requests are cancellable and project-generation scoped", () => {
  assert.match(viewSource, /new ConflictRequestScope\(\)/);
  assert.match(viewSource, /identity,?/);
  assert.match(viewSource, /scope\?\.acceptsResolution\(identity\)/);
  assert.match(viewSource, /scope\.deactivate\(\)/);
  assert.match(viewSource, /cleanup\.projectGeneration/);
  assert.match(viewSource, /activeProjectRef\.current === projectId/);
  assert.match(scopeSource, /projectGeneration/);
});

test("pending, blocked and failed states stay visible to the user", () => {
  assert.match(viewSource, /conflicts\.operationFailed/);
  assert.match(viewSource, /conflicts\.queuedAs/);
  assert.match(viewSource, /pendingResolution/);
  assert.match(viewSource, /canResolve === false/);
  assert.match(viewSource, /conflicts\.recentDecisions/);
  assert.match(viewSource, /operation\.receipt\?\.operationId/);
  assert.match(viewSource, /cancelConflictRequest/);
  assert.doesNotMatch(viewSource, /console\.(?:log|warn|error|debug)/);
});

test("the engine's own pending state reaches the page", () => {
  assert.match(bridgeSource, /can_resolve: view\.can_resolve/);
  assert.match(bridgeSource, /pending_resolution/);
});
