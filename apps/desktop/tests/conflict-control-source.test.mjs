import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const paneSource = await readFile(
  new URL("../src/components/ConflictsPane.tsx", import.meta.url),
  "utf8",
);
const contentSource = await readFile(
  new URL("../src/components/ConflictPaneContent.tsx", import.meta.url),
  "utf8",
);
const actionSource = await readFile(
  new URL("../src/components/ConflictResolutionAction.ts", import.meta.url),
  "utf8",
);
const workspaceSource = await readFile(
  new URL("../src/components/WorkspaceView.tsx", import.meta.url),
  "utf8",
);
const syncSource = await readFile(
  new URL("../src-tauri/src/sync.rs", import.meta.url),
  "utf8",
);

test("workspace exposes a real conflicts view backed by Tauri commands", () => {
  assert.match(workspaceSource, /"conflicts"/);
  assert.match(workspaceSource, /<ConflictsPane/);
  assert.match(
    paneSource,
    /invoke<ConflictView\[\]>\("list_sync_conflicts"/,
  );
  assert.match(actionSource, /"resolve_sync_conflict"/);
  assert.match(paneSource, /runConflictResolution/);
  assert.match(paneSource, /"list_sync_conflict_operations"/);
  assert.match(paneSource, /"cancel_sync_conflict_generation"/);
  assert.match(syncSource, /pub async fn list_sync_conflicts/);
  assert.match(syncSource, /pub async fn resolve_sync_conflict/);
  assert.doesNotMatch(paneSource, /state\.sqlite|sqlite|openDatabase/);
});

test("all four durable conflict choices are available", () => {
  for (const choice of ["current", "incoming", "merged", "delete"]) {
    assert.match(contentSource, new RegExp(`onResolve\\(conflict, "${choice}"\\)`));
  }
  assert.match(actionSource, /conflictId:\s*conflict\.conflictId/);
  assert.match(actionSource, /conflictRevision:\s*conflict\.conflictRevision/);
  assert.match(contentSource, /receipt\.operationId/);
});

test("polling and resolution use cancellable project-scoped identities", () => {
  assert.match(paneSource, /new ConflictRequestScope\(\)/);
  assert.match(paneSource, /identity,/);
  assert.match(paneSource, /scope\.acceptsResolution\(identity\)/);
  assert.match(paneSource, /scope\.deactivate\(\)/);
  assert.match(paneSource, /projectGeneration:\s*cleanup\.projectGeneration/);
  assert.match(paneSource, /activeProjectRef\.current === projectId/);
  assert.match(paneSource, /clearTimeout/);
  assert.match(contentSource, /disabled=\{disabled/);
});

test("conflict failures and pending operation ids remain observable", () => {
  assert.match(contentSource, /Conflict operation failed/);
  assert.match(paneSource, /cleanup=/);
  assert.match(contentSource, /Resolution queued as/);
  assert.match(contentSource, /pendingResolution/);
  assert.match(contentSource, /blockedReason/);
  assert.match(contentSource, /Recent decisions/);
  assert.match(contentSource, /operation\.receipt\.operationId/);
  assert.doesNotMatch(paneSource, /console\.(?:log|warn|error|debug)/);
});
