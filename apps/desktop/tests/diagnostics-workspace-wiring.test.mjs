import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const workspaceSource = await readFile(
  new URL("../src/components/WorkspaceView.tsx", import.meta.url),
  "utf8",
);

test("logs tab is wired through DiagnosticsPane and typed facade only", () => {
  assert.match(workspaceSource, /key:\s*"logs"/);
  assert.match(workspaceSource, /DiagnosticsPane/);
  assert.match(workspaceSource, /createInvokeDiagnosticsClient/);
  assert.match(workspaceSource, /activeTab === "logs"/);
  // Diagnostics path must not free-tail arbitrary files from WorkspaceView.
  assert.doesNotMatch(workspaceSource, /readTextFile|tail -f/);
});

test("logs diagnostics client is project-scoped via DiagnosticsPane props", () => {
  assert.match(workspaceSource, /projectId=\{project\.id\}/);
  assert.match(workspaceSource, /client=\{diagnosticsClient\}/);
});
