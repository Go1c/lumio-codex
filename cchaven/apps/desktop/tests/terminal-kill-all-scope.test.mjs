import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const terminalRs = await readFile(
  new URL("../src-tauri/src/terminal.rs", import.meta.url),
  "utf8",
);

test("kill_all_sessions does not pkill claude globally", () => {
  const fnStart = terminalRs.indexOf("pub fn kill_all_sessions");
  assert.ok(fnStart >= 0);
  const fnBody = terminalRs.slice(fnStart, fnStart + 1200);
  assert.doesNotMatch(fnBody, /pkill\s+-f\s+claude/);
  assert.match(fnBody, /tmux kill-session/);
});
