import assert from "node:assert/strict";
import test from "node:test";

import {
  DEFAULT_SESSION_TITLE,
  SESSION_TITLE_WIDTH,
  clipWidth,
  firstSubmittedLine,
  isSlashCommand,
  lockTitleFromInput,
} from "./session-title.ts";

test("default session title is 新对话", () => {
  assert.equal(DEFAULT_SESSION_TITLE, "新对话");
  assert.equal(SESSION_TITLE_WIDTH, 22);
});

test("clipWidth keeps English text that fits the budget", () => {
  assert.equal(clipWidth("retry helper", 22), "retry helper");
});

test("clipWidth truncates English past the budget with an ellipsis", () => {
  assert.equal(clipWidth("abcdefghijklmnopqrstuvwxyz", 22), "abcdefghijklmnopqrstuv…");
});

test("clipWidth counts CJK characters as width 2", () => {
  assert.equal(clipWidth("你好世界", 4), "你好…");
  assert.equal(clipWidth("你好", 4), "你好");
});

test("slash commands are ignored as title sources", () => {
  assert.equal(isSlashCommand("/clear"), true);
  assert.equal(isSlashCommand("  /model "), true);
  assert.equal(isSlashCommand("fix the retry"), false);
  assert.equal(firstSubmittedLine("/clear"), null);
  assert.equal(firstSubmittedLine("  /model\nmore"), null);
});

test("empty and whitespace submissions are ignored", () => {
  assert.equal(firstSubmittedLine(""), null);
  assert.equal(firstSubmittedLine("   \nhello"), null);
  assert.equal(firstSubmittedLine("\n"), null);
});

test("multiline paste only uses the first line", () => {
  assert.equal(firstSubmittedLine("抽重试逻辑\n第二行"), "抽重试逻辑");
  assert.equal(firstSubmittedLine("retry helper  \nnext"), "retry helper");
});

test("lockTitleFromInput locks on the first real submitted line", () => {
  const locked = lockTitleFromInput({ title: null, titleLocked: false }, "抽重试逻辑\n第二行");
  assert.equal(locked.titleLocked, true);
  assert.equal(locked.title, "抽重试逻辑");
});

test("lockTitleFromInput does not change a locked title", () => {
  const current = { title: "抽重试逻辑", titleLocked: true };
  assert.deepEqual(lockTitleFromInput(current, "另一句话"), current);
});

test("lockTitleFromInput skips slash commands and blank input", () => {
  const current = { title: null, titleLocked: false };
  assert.deepEqual(lockTitleFromInput(current, "/clear"), current);
  assert.deepEqual(lockTitleFromInput(current, "  "), current);
});

test("lockTitleFromInput ignores terminal device replies so a new Claude chat stays 新对话", () => {
  const current = { title: null, titleLocked: false };
  assert.deepEqual(lockTitleFromInput(current, "[>0;276;0c[O[I[<0;3"), current);
  assert.deepEqual(lockTitleFromInput(current, "[?1;2c"), current);
  assert.equal(firstSubmittedLine("[>0;276;0c"), null);
});

test("lockTitleFromInput clips long titles to the default width", () => {
  const locked = lockTitleFromInput(
    { title: null, titleLocked: false },
    "abcdefghijklmnopqrstuvwxyz",
  );
  assert.equal(locked.title, "abcdefghijklmnopqrstuv…");
  assert.equal(locked.titleLocked, true);
});
