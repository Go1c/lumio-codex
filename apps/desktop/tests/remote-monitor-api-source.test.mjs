import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const src = await readFile(
  new URL("../src/lib/remoteMonitorApi.ts", import.meta.url),
  "utf8",
);

test("remote monitor API exposes four commands and camelCase types", () => {
  assert.match(src, /get_server_status/);
  assert.match(src, /list_claude_sessions/);
  assert.match(src, /switch_claude_session/);
  assert.match(src, /kill_claude_session/);
  assert.match(src, /ourServicesMemoryRssBytes/);
  assert.match(src, /export type ServerStatusSnapshot/);
  assert.match(src, /export type ClaudeSessionsSnapshot/);
});
