import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { readAllClaudeViews } from "./read-claude-views.ts";
import { formatStatusBytes, serviceDisplayName } from "./remote-status.ts";

test("connected workspace exposes 服务器状态 and 对话状态", async () => {
  const views = await readAllClaudeViews();
  const api = await readFile(new URL("./api.ts", import.meta.url), "utf8");
  const session = await readFile(new URL("./session.ts", import.meta.url), "utf8");
  assert.match(views, />服务器状态</);
  assert.match(views, />对话状态</);
  assert.match(views, /fetchClaudeServerStatus/);
  assert.match(views, /fetchClaudeSessions/);
  assert.match(api, /lumio_claude_server_status/);
  assert.match(api, /lumio_claude_list_sessions/);
  assert.match(session, /fetchClaudeServerStatus|listClaudeServerStatus/);
  assert.match(session, /fetchClaudeSessions/);
});

test("status surfaces map snapshots without leaking banned words", () => {
  assert.equal(serviceDisplayName("sync"), "同步组件");
  assert.equal(serviceDisplayName("workspace"), "远端服务");
  assert.equal(serviceDisplayName("claude"), "Claude");
  assert.doesNotMatch(serviceDisplayName("sync"), /agent/i);
  assert.match(formatStatusBytes(1024), /1(\.0)? KB|1 KB/);
});
