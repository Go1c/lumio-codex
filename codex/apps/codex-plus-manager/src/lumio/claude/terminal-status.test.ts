import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { TERMINAL_FAIL_BANNER, terminalBanner } from "./terminal-status.ts";

test("terminalBanner shows 没能打开终端 only when start failed and there is no output", () => {
  assert.equal(terminalBanner(false, false), TERMINAL_FAIL_BANNER);
  assert.equal(terminalBanner(false, false, "正在打开终端…"), TERMINAL_FAIL_BANNER);
  assert.equal(TERMINAL_FAIL_BANNER, "没能打开终端。");
});

test("terminalBanner is null once any output has arrived", () => {
  assert.equal(terminalBanner(false, true), null);
  assert.equal(terminalBanner(true, true), null);
  assert.equal(terminalBanner(false, true, TERMINAL_FAIL_BANNER), null);
  assert.equal(terminalBanner(true, true, "正在打开终端…"), null);
});

test("terminalBanner keeps a non-fail status while the session is open without output", () => {
  assert.equal(terminalBanner(true, false, "正在打开终端…"), "正在打开终端…");
  assert.equal(terminalBanner(true, false, "连接已断开，正在重连…"), "连接已断开，正在重连…");
  assert.equal(terminalBanner(true, false), null);
  assert.equal(terminalBanner(true, false, TERMINAL_FAIL_BANNER), null);
});

test("terminalBanner copy never says agent or tmux", async () => {
  const source = await readFile(new URL("./terminal-status.ts", import.meta.url), "utf8");
  assert.doesNotMatch(source, /\bagent\b/i);
  assert.doesNotMatch(source, /\btmux\b/i);
  assert.doesNotMatch(TERMINAL_FAIL_BANNER, /\bagent\b/i);
  assert.doesNotMatch(TERMINAL_FAIL_BANNER, /\btmux\b/i);
});
