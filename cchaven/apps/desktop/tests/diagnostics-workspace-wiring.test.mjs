import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const workspaceSource = await readFile(
  new URL("../src/components/Workspace.tsx", import.meta.url),
  "utf8",
);
const apiSource = await readFile(
  new URL("../src/lib/api.ts", import.meta.url),
  "utf8",
);

test("logs tab is wired through DiagnosticsPane and the typed facade only", () => {
  assert.match(workspaceSource, /key:\s*"logs"/);
  assert.match(workspaceSource, /DiagnosticsPane/);
  assert.match(workspaceSource, /activeTab === "logs"/);
  // The diagnostics client is built once, from the same invoke seam as the rest
  // of the app, so browser mock mode drives the same component.
  assert.match(apiSource, /createInvokeDiagnosticsClient/);
  // Diagnostics must not free-tail arbitrary files from the workspace.
  assert.doesNotMatch(workspaceSource, /readTextFile|tail -f/);
});

test("the diagnostics client is project-scoped through props", () => {
  assert.match(workspaceSource, /projectId=\{project\.id\}/);
  assert.match(workspaceSource, /client=\{diagnosticsClient\}/);
});
