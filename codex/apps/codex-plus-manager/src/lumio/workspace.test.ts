import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import type { LumioPhase } from "./types.ts";
import { DEFAULT_WORKSPACE, workspaceTabsVisible } from "./workspace.ts";

const hiddenPhases: LumioPhase[] = [
  "bootstrapping",
  "signed-out",
  "authenticating",
  "provisioning",
  "needs-repair",
];

test("unsigned and repair phases hide Codex/Claude tabs", () => {
  for (const phase of hiddenPhases) {
    assert.equal(workspaceTabsVisible(phase), false, phase);
  }
});

test("ready phases show product tabs", () => {
  assert.equal(workspaceTabsVisible("ready-online"), true);
  assert.equal(workspaceTabsVisible("ready-offline"), true);
});

test("the default workspace is codex", () => {
  assert.equal(DEFAULT_WORKSPACE, "codex");
});

test("LumioApp keeps HomeView and ClaudeWorkspace mounted behind hidden", async () => {
  const shell = await readFile(new URL("../LumioApp.tsx", import.meta.url), "utf8");
  assert.match(shell, /workspaceTabsVisible/);
  assert.match(shell, /DEFAULT_WORKSPACE/);
  assert.match(shell, /<HomeView/);
  assert.match(shell, /<ClaudeWorkspace/);
  assert.match(shell, /hidden=\{!showCodexHome\}/);
  assert.match(shell, /hidden=\{!showClaude\}/);
  assert.match(shell, /lumio-workspace-pane/);
  assert.doesNotMatch(shell, /showClaude \? \(/);
  assert.doesNotMatch(shell, /workspace === "codex" && <HomeView/);
});

test("workspace panes fill the main stage instead of growing the window", async () => {
  const css = await readFile(new URL("../lumio-shell.css", import.meta.url), "utf8");
  assert.match(css, /\.lumio-workspace-pane/);
  assert.match(css, /\.lumio-workspace-pane\s*\{[^}]*height:\s*100%/);
  assert.match(css, /\.lumio-workspace-pane\s*\{[^}]*overflow:\s*hidden/);
});

test("Claude workspace hides the launcher footer so the 26px status bar is the window bottom", async () => {
  const shell = await readFile(new URL("../LumioApp.tsx", import.meta.url), "utf8");
  const css = await readFile(new URL("../lumio-shell.css", import.meta.url), "utf8");
  assert.match(shell, /is-claude-workspace/);
  assert.match(shell, /hidden=\{showClaude\}/);
  assert.match(css, /\.lumio-app\.is-claude-workspace\s*\{[^}]*grid-template-rows:\s*54px minmax\(0,\s*1fr\)/);
});
