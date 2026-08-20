import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { tagColorDiff } from "./color-diff.ts";

test("color-diff tags an unchanged line, a deleted line, and an inserted line", () => {
  const tagged = tagColorDiff("keep\nremove\n", "keep\ninsert\n");
  const same = tagged.find((line) => line.tag === "same");
  const deleted = tagged.find((line) => line.tag === "del");
  const added = tagged.find((line) => line.tag === "add");
  assert.equal(same?.text, "keep");
  assert.equal(deleted?.text, "remove");
  assert.equal(added?.text, "insert");
  const tags = new Set(tagged.map((line) => line.tag));
  assert.ok(tags.has("same") && tags.has("del") && tags.has("add"));
});

test("conflict detail is a single colored view, not two raw file dumps", async () => {
  const home = await readFile(new URL("../views/claude/ClaudeHome.tsx", import.meta.url), "utf8");
  const css = await readFile(
    new URL("../views/claude/claude-workspace.css", import.meta.url),
    "utf8",
  );
  assert.match(home, /tagColorDiff/);
  assert.match(home, /lumio-claude-color-diff/);
  assert.doesNotMatch(home, /lumio-claude-diff/);
  assert.doesNotMatch(css, /\.lumio-claude-diff\s*\{[^}]*grid-template-columns:\s*1fr 1fr/s);
  assert.match(css, /\.lumio-claude-color-diff/);
  assert.match(css, /\.is-add/);
  assert.match(css, /\.is-del/);
});
