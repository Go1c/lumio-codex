import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

test("claude sessions switch requests terminal tab", async () => {
  const pane = await readFile(
    new URL("../src/features/remote-monitor/ClaudeSessionsPane.tsx", import.meta.url),
    "utf8",
  );
  assert.match(pane, /onRequestTerminalTab/);
  assert.match(pane, /switchClaudeSession/);
  assert.match(pane, /killClaudeSession/);
  assert.match(pane, /confirm\(/);
});
