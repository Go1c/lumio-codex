import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const terminalRs = await readFile(
  new URL("../src-tauri/src/terminal.rs", import.meta.url),
  "utf8",
);

test("killing sessions targets only this project's tmux session", () => {
  const start = terminalRs.indexOf("pub fn kill_session_command");
  assert.ok(start >= 0, "kill_session_command is missing");
  const body = terminalRs.slice(start, start + 600);
  // A host-wide sweep would take out other projects sharing the machine.
  assert.doesNotMatch(body, /pkill\s+-f\s+claude/);
  assert.doesNotMatch(body, /kill-server/);
  assert.match(body, /tmux kill-session/);
  assert.match(body, /sanitize_session_name/);

  assert.match(terminalRs, /pub async fn kill_all_sessions/);
});
