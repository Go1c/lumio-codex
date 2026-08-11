import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const workspaceSource = await readFile(
  new URL("../src/components/WorkspaceView.tsx", import.meta.url),
  "utf8",
);

test("server status and claude sessions tabs are wired", () => {
  assert.match(workspaceSource, /key:\s*"server-status"/);
  assert.match(workspaceSource, /key:\s*"claude-sessions"/);
  assert.match(workspaceSource, /ServerStatusPane/);
  assert.match(workspaceSource, /ClaudeSessionsPane/);
  assert.match(workspaceSource, /createRemoteMonitorClient/);
});
