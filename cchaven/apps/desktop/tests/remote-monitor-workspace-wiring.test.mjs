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

test("server status and claude sessions tabs are wired", () => {
  assert.match(workspaceSource, /key:\s*"server-status"/);
  assert.match(workspaceSource, /key:\s*"claude-sessions"/);
  assert.match(workspaceSource, /ServerStatusPane/);
  assert.match(workspaceSource, /ClaudeSessionsPane/);
  assert.match(workspaceSource, /client=\{remoteMonitorClient\}/);
  assert.match(apiSource, /createRemoteMonitorClient/);
});

test("switching a session hands the user back to the terminal", () => {
  assert.match(workspaceSource, /onRequestTerminalTab=\{\(\) => setActiveTab\("terminal"\)\}/);
});
