import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import test, { after } from "node:test";
import { build } from "esbuild";

const testsDirectory = dirname(fileURLToPath(import.meta.url));
const desktopDirectory = dirname(testsDirectory);
const outputDirectory = await mkdtemp(join(desktopDirectory, ".fns-scope-test-"));
const outputFile = join(outputDirectory, "conflict-request-scope.mjs");

await build({
  entryPoints: [
    join(desktopDirectory, "src/components/ConflictRequestScope.ts"),
  ],
  bundle: true,
  format: "esm",
  platform: "node",
  outfile: outputFile,
  logLevel: "silent",
});

const { ConflictRequestScope } = await import(pathToFileURL(outputFile).href);

after(async () => {
  await rm(outputDirectory, { recursive: true, force: true });
});

function uuidSequence(...values) {
  let index = 0;
  return () => values[index++];
}

test("a delayed response cannot update a deactivated project scope", async () => {
  const scope = new ConflictRequestScope(
    uuidSequence("project-generation", "request-id"),
  );
  const identity = scope.beginResolution("conflict:7");
  assert.deepEqual(identity, {
    projectGeneration: "project-generation",
    requestId: "request-id",
  });

  let release;
  const delayed = new Promise((resolve) => {
    release = resolve;
  });
  const accepted = delayed.then(() => scope.acceptsResolution(identity));
  scope.deactivate();
  release();

  assert.equal(await accepted, false);
});

test("cleanup reports the old generation and every active request", () => {
  const scope = new ConflictRequestScope(
    uuidSequence("old-generation", "active-request"),
  );
  scope.beginResolution("conflict:8");

  assert.deepEqual(scope.deactivate(), {
    projectGeneration: "old-generation",
    activeRequestIds: ["active-request"],
  });
  assert.equal(scope.beginResolution("conflict:8"), null);
});

test("duplicate resolve attempts are rejected until the first one settles", () => {
  const scope = new ConflictRequestScope(
    uuidSequence("generation", "request-one", "request-two"),
  );
  const first = scope.beginResolution("conflict:9");
  assert.equal(scope.beginResolution("conflict:9"), null);

  scope.finishResolution(first);
  assert.deepEqual(scope.beginResolution("conflict:9"), {
    projectGeneration: "generation",
    requestId: "request-two",
  });
});

test("overlapping refreshes queue one follow-up per project scope", () => {
  const scope = new ConflictRequestScope(uuidSequence("generation"));
  assert.equal(scope.beginRefresh(), true);
  assert.equal(scope.beginRefresh(), false);
  assert.equal(scope.finishRefresh(), true);
  assert.equal(scope.beginRefresh(), true);
  assert.equal(scope.finishRefresh(), false);
});

test("a queued refresh is discarded when its project scope deactivates", () => {
  const scope = new ConflictRequestScope(uuidSequence("generation"));
  assert.equal(scope.beginRefresh(), true);
  assert.equal(scope.beginRefresh(), false);
  scope.deactivate();
  assert.equal(scope.finishRefresh(), false);
  assert.equal(scope.beginRefresh(), false);
});
