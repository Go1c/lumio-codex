import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { persistableClaudeState } from "./machine.ts";
import { initialClaudeState, reduceClaudeState, resolveClaudeSurface } from "./machine.ts";
import { parseSshTarget } from "./ssh-target.ts";
import { dispatchClaude, getClaudeState, resetClaudeStore } from "./store.ts";
import { DEFAULT_WORKSPACE, workspaceTabsVisible } from "../workspace.ts";
import { initialLumioState, reduceLumioState } from "../state.ts";
import type { LumioCodexApp } from "../types.ts";

function detectedApp(): LumioCodexApp {
  return { path: "/Applications/Codex.app", version: "1.0.0", source: "automatic" };
}

test("1 unsigned phases hide Codex/Claude tabs", () => {
  assert.equal(workspaceTabsVisible("signed-out"), false);
  assert.equal(workspaceTabsVisible("authenticating"), false);
  assert.equal(workspaceTabsVisible("provisioning"), false);
});

test("2 repair hides tabs", () => {
  assert.equal(workspaceTabsVisible("needs-repair"), false);
});

test("3 ready defaults to the Codex workspace", () => {
  assert.equal(workspaceTabsVisible("ready-online"), true);
  assert.equal(DEFAULT_WORKSPACE, "codex");
});

test("4 canceling the destination sheet does not start the download", async () => {
  const home = await readFile(new URL("../views/HomeView.tsx", import.meta.url), "utf8");
  assert.match(home, /取消不会开始安装/);
  assert.match(home, /onClick=\{\(\) => setDestinationOpen\(false\)\}/);
  assert.ok(home.includes("取消"));
});

test("5 switching away does not unmount HomeView or ClaudeWorkspace", async () => {
  const shell = await readFile(new URL("../../LumioApp.tsx", import.meta.url), "utf8");
  assert.match(shell, /hidden=\{!showCodexHome\}/);
  assert.match(shell, /hidden=\{!showClaude\}/);
  assert.match(shell, /<HomeView/);
  assert.match(shell, /<ClaudeWorkspace/);
  assert.doesNotMatch(shell, /showClaude \? \(/);
});

test("6 offline cannot install without an app, can launch when installed, cannot pay", () => {
  const noApp = reduceLumioState(initialLumioState(), {
    type: "bootstrapped",
    payload: {
      version: "1.0.0",
      platform: "macos",
      arch: "aarch64",
      codexApp: null,
      account: { email: "a@b.c", balance: 1, planLabel: null },
      telemetryEnabled: false,
      autoUpdateEnabled: true,
    },
  });
  const offlineMissing = reduceLumioState(noApp, { type: "offline-ready", cachedAt: null });
  assert.equal(offlineMissing.phase, "ready-offline");
  assert.equal(offlineMissing.actions.canLaunch, false);
  assert.equal(offlineMissing.actions.canPay, false);

  const withApp = reduceLumioState(offlineMissing, { type: "codex-app-changed", app: detectedApp() });
  assert.equal(withApp.actions.canLaunch, true);
  assert.equal(withApp.actions.canPay, false);
  assert.equal(withApp.actions.canRefresh, false);
});

test("7 none entitlement is the subscribe surface and opens the portal", async () => {
  assert.equal(
    resolveClaudeSurface({ entitlement: { status: "none" }, projectCount: 0, sheetOpen: false }),
    "subscribe",
  );
  const sub = await readFile(new URL("../views/claude/ClaudeSubscribe.tsx", import.meta.url), "utf8");
  assert.match(sub, /开通 Claude/);
  assert.match(sub, /¥19\.9/);
  assert.doesNotMatch(sub, /purchase|cardNumber|支付/);
  const shell = await readFile(new URL("../../LumioApp.tsx", import.meta.url), "utf8");
  const portal = await readFile(new URL("./portal.ts", import.meta.url), "utf8");
  assert.match(portal, /https:\/\/bestcodex\.app\/account/);
  assert.match(shell, /openInBrowser\(CLAUDE_ACCOUNT_URL\)/);
});

test("8 entitled with zero projects is empty, not an ssh form", async () => {
  assert.equal(
    resolveClaudeSurface({ entitlement: { status: "active" }, projectCount: 0, sheetOpen: false }),
    "empty",
  );
  const empty = await readFile(new URL("../views/claude/ClaudeEmpty.tsx", import.meta.url), "utf8");
  assert.doesNotMatch(empty, /type="password"/);
  assert.match(empty, /连接一台服务器/);
});

test("9 cancel connect returns to empty with no projects", () => {
  resetClaudeStore();
  dispatchClaude({ type: "entitlement-resolved", entitlement: { status: "active", source: "local" } });
  dispatchClaude({ type: "open-connect" });
  dispatchClaude({ type: "cancel-connect" });
  assert.equal(getClaudeState().sheet, null);
  assert.equal(getClaudeState().projects.length, 0);
  assert.equal(getClaudeState().page, "empty");
});

test("10 paste ssh root@1.2.3.4 splits user and host", () => {
  assert.deepEqual(parseSshTarget("ssh root@1.2.3.4"), { host: "1.2.3.4", user: "root", port: null });
});

test("11 failed probe stays on probe with SSH_AUTH_FAILED", () => {
  let state = initialClaudeState();
  state = reduceClaudeState(state, {
    type: "entitlement-resolved",
    entitlement: { status: "active", source: "local" },
  });
  state = reduceClaudeState(state, { type: "open-connect" });
  state = reduceClaudeState(state, { type: "probe-started" });
  state = reduceClaudeState(state, {
    type: "probe-finished",
    result: {
      ok: false,
      reachable: true,
      authenticated: false,
      target: "1.2.3.4:22",
      user: "root",
      distro: null,
      cpu: null,
      memory: null,
      errorCode: "SSH_AUTH_FAILED",
      detail: "无法登录",
    },
  });
  assert.equal(state.sheet?.step, "probe");
  assert.equal(state.sheet?.probe?.errorCode, "SSH_AUTH_FAILED");
});

test("12 first-sync progress survives unsubscribe", () => {
  resetClaudeStore();
  dispatchClaude({ type: "entitlement-resolved", entitlement: { status: "active", source: "local" } });
  dispatchClaude({ type: "open-connect" });
  dispatchClaude({ type: "start-sync" });
  dispatchClaude({ type: "sync-progress", filesDone: 2, filesTotal: 8 });
  assert.equal(getClaudeState().sheet?.sync.filesDone, 2);
});

test("13 selecting a project only changes the active id", () => {
  let state = initialClaudeState();
  state = reduceClaudeState(state, {
    type: "entitlement-resolved",
    entitlement: { status: "active", source: "local" },
  });
  const first = {
    id: "p-my-project",
    name: "my-project",
    host: "1.2.3.4",
    user: "root",
    port: 22,
    auth: "password" as const,
    keyPath: null,
    remoteRoot: "/root/bestcodex/my-project",
    localRoot: "~/BestCodex/my-project",
    createdAt: "2026-08-16T00:00:00.000Z",
  };
  const second = { ...first, id: "p-docs-site", name: "docs-site" };
  state = reduceClaudeState(state, {
    type: "projects-hydrated",
    projects: [first, second],
    activeProjectId: first.id,
  });
  state = reduceClaudeState(state, {
    type: "append-terminal",
    projectId: first.id,
    line: { kind: "ok", text: "attached" },
  });
  state = reduceClaudeState(state, { type: "select-project", projectId: second.id });
  assert.equal(state.activeProjectId, second.id);
  assert.equal(state.terminalByProject[first.id][0]?.text, "attached");
  assert.equal(state.page, "workspace");
});

test("14 files and conflicts tabs render", async () => {
  const home = await readFile(new URL("../views/claude/ClaudeHome.tsx", import.meta.url), "utf8");
  assert.match(home, />文件</);
  assert.match(home, />冲突</);
  assert.match(home, /FilesPane|stageTab === "files"/);
  assert.match(home, /ConflictsPane|stageTab === "conflicts"/);
});

test("15 secrets do not appear in view source literals or persistable snapshots", async () => {
  const files = [
    "../views/claude/ClaudeConnect.tsx",
    "../views/claude/ClaudeHome.tsx",
    "../views/claude/ClaudeWorkspace.tsx",
    "../views/claude/ClaudeSubscribe.tsx",
    "./session.ts",
    "./store.ts",
  ];
  for (const rel of files) {
    const source = await readFile(new URL(rel, import.meta.url), "utf8");
    assert.doesNotMatch(source, /sk-|ghp_|eyJhbGci|BEGIN OPENSSH PRIVATE KEY/);
    assert.doesNotMatch(source, /password:\s*["'][^"']+["']/);
  }
  resetClaudeStore();
  const persisted = JSON.stringify(persistableClaudeState(getClaudeState()));
  assert.equal(persisted.includes("password"), false);
});
