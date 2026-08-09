import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import test, { after } from "node:test";
import { build } from "esbuild";

const testsDirectory = dirname(fileURLToPath(import.meta.url));
const desktopDirectory = dirname(testsDirectory);
const outputDirectory = await mkdtemp(join(desktopDirectory, ".fns-resolution-test-"));
const outputFile = join(outputDirectory, "conflict-resolution-action.mjs");

await build({
  entryPoints: [
    join(desktopDirectory, "src/components/ConflictResolutionAction.ts"),
  ],
  bundle: true,
  format: "esm",
  platform: "node",
  outfile: outputFile,
  logLevel: "silent",
});

const { runConflictResolution } = await import(pathToFileURL(outputFile).href);

after(async () => {
  await rm(outputDirectory, { recursive: true, force: true });
});

const identity = {
  requestId: "20000000-0000-4000-8000-000000000001",
  projectGeneration: "20000000-0000-4000-8000-000000000002",
};
const conflict = {
  conflictId: "20000000-0000-4000-8000-000000000003",
  conflictRevision: "17",
};

for (const choice of ["current", "incoming"]) {
  test(`${choice} invokes the backend with identity and refreshes after its receipt`, async () => {
    const events = [];
    const receipt = { status: "queued", operationId: `operation-${choice}` };
    const result = await runConflictResolution({
      invokeCommand: async (command, args) => {
        events.push({ type: "invoke", command, args });
        return receipt;
      },
      projectId: "project-one",
      identity,
      conflict,
      choice,
      refresh: async () => {
        events.push({ type: "refresh" });
      },
    });

    assert.deepEqual(result, { ok: true, receipt });
    assert.deepEqual(events, [
      {
        type: "invoke",
        command: "resolve_sync_conflict",
        args: {
          projectId: "project-one",
          identity,
          input: {
            conflictId: conflict.conflictId,
            conflictRevision: conflict.conflictRevision,
            choice,
          },
        },
      },
      { type: "refresh" },
    ]);
  });
}

test("a rejected resolution remains an error and still refreshes current state", async () => {
  const failure = { primary: "conflict_revision_stale", cleanup: [] };
  let refreshes = 0;
  const result = await runConflictResolution({
    invokeCommand: async () => {
      throw failure;
    },
    projectId: "project-one",
    identity,
    conflict,
    choice: "incoming",
    refresh: async () => {
      refreshes += 1;
    },
  });

  assert.deepEqual(result, { ok: false, error: failure });
  assert.equal(refreshes, 1);
});
