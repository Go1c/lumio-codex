import assert from "node:assert/strict";
import test from "node:test";

import { readFile } from "node:fs/promises";

import {
  DEFAULT_CLAUDE_PLAN_CENTS,
  beginNewProjectOnHost,
  cancelClaudeConnect,
  formatClaudeOrderYuan,
  formatClaudePlanYuan,
  runConnectSync,
} from "./session.ts";
import {
  dispatchClaude,
  draftPassword,
  getClaudeState,
  rememberProjectPassword,
  resetClaudeStore,
} from "./store.ts";

test("beginNewProjectOnHost reuses the sibling machine and remembered password", () => {
  resetClaudeStore();
  dispatchClaude({ type: "entitlement-resolved", entitlement: { status: "active", source: "local" } });
  dispatchClaude({
    type: "projects-hydrated",
    projects: [
      {
        id: "p-my-project",
        name: "my-project",
        host: "43.156.20.8",
        user: "ubuntu",
        port: 22,
        auth: "password",
        keyPath: null,
        hostAlias: null,
        remoteRoot: "~/bestcodex/my-project",
        localRoot: "~/BestCodex/my-project",
        createdAt: "2026-08-16T00:00:00.000Z",
      },
    ],
    activeProjectId: "p-my-project",
  });
  rememberProjectPassword("p-my-project", "s3cret");
  beginNewProjectOnHost("43.156.20.8");
  const sheet = getClaudeState().sheet;
  assert.equal(sheet?.mode, "project");
  assert.equal(sheet?.step, "probe");
  assert.equal(sheet?.draft.host, "43.156.20.8");
  assert.equal(sheet?.draft.user, "ubuntu");
  assert.equal(sheet?.draft.projectName, "my-project-2");
  assert.equal(draftPassword(), "s3cret");
});

test("canceling connect does not leave a project in the store", () => {
  resetClaudeStore();
  dispatchClaude({ type: "entitlement-resolved", entitlement: { status: "active", source: "local" } });
  dispatchClaude({ type: "open-connect" });
  dispatchClaude({ type: "draft-updated", draft: { host: "1.2.3.4" } });
  cancelClaudeConnect();
  assert.equal(getClaudeState().sheet, null);
  assert.equal(getClaudeState().projects.length, 0);
  assert.equal(getClaudeState().page, "empty");
});

test("setup inspect uses the user-chosen remote folder", async () => {
  const source = await readFile(new URL("./session.ts", import.meta.url), "utf8");
  assert.match(source, /draft\.remoteRoot/);
  assert.match(source, /replaceLastSegment/);
  assert.doesNotMatch(source, /const remoteRoot = remoteProjectRoot\(draft\.user, desired\)/);
});

test("file refresh passes the project id to private conflict state", async () => {
  const source = await readFile(new URL("./session.ts", import.meta.url), "utf8");
  assert.match(
    source,
    /listClaudeFiles\(\{(?:(?!\}\);)[\s\S])*\n\s+projectId,/,
  );
});

test("an already-installed host asks to reinstall instead of failing", async () => {
  const source = await readFile(new URL("./session.ts", import.meta.url), "utf8");
  assert.match(source, /componentsInstalled/);
  assert.match(source, /setup-needs-reinstall/);
  assert.match(source, /"reinstall"/);
  assert.match(source, /replace:\s*decision === "reinstall"/);
});

test("SYNC_ENGINE_UNAVAILABLE is not a keep-project success", async () => {
  resetClaudeStore();
  dispatchClaude({ type: "entitlement-resolved", entitlement: { status: "active", source: "local" } });
  dispatchClaude({ type: "open-connect" });
  dispatchClaude({ type: "draft-updated", draft: { host: "1.2.3.4", projectName: "docs-site" } });
  dispatchClaude({ type: "continue-setup" });
  dispatchClaude({ type: "setup-finished", ok: true });
  await runConnectSync();
  const state = getClaudeState();
  assert.equal(state.projects.length, 0);
  assert.notEqual(state.page, "workspace");
  assert.equal(state.sheet?.step, "sync");
  assert.equal(state.sheet?.sync.state, "fail");
  assert.equal(state.sheet?.sync.errorCode, "SYNC_ENGINE_UNAVAILABLE");
});

test("pay success applies receipt expiresAt and daysLeft instead of status only", async () => {
  const source = await readFile(new URL("./session.ts", import.meta.url), "utf8");
  assert.match(source, /entitlementFromSnapshot/);
  assert.match(source, /expiresAt: paid\.expiresAt/);
  assert.match(source, /daysLeft: paid\.daysLeft/);
  assert.match(source, /payload\?\.expiresAt/);
  assert.match(source, /payload\?\.daysLeft/);
});

test("plan fallback is 19.9 yuan and order amounts keep two decimals", () => {
  assert.equal(DEFAULT_CLAUDE_PLAN_CENTS, 1990);
  assert.equal(formatClaudePlanYuan(1990), "19.9");
  assert.equal(formatClaudeOrderYuan(1990), "19.90");
  assert.notEqual(formatClaudePlanYuan(DEFAULT_CLAUDE_PLAN_CENTS), "68");
});

test("hydrate and first sync kick CLI install then login for that host", async () => {
  const source = await readFile(new URL("./session.ts", import.meta.url), "utf8");
  assert.match(source, /CLAUDE_CLI_PROGRESS_EVENT/);
  assert.match(source, /CLAUDE_LOGIN_PROGRESS_EVENT/);
  assert.match(source, /continueClaudeInit/);
  assert.match(source, /ensureHostCli/);
  assert.match(source, /refreshHostLogin/);
  assert.match(source, /activateClaudeProject/);
  assert.match(source, /set-workspace-phase/);
  assert.match(source, /phase: "resume"/);
  assert.match(source, /phase: "offline"/);
  assert.match(source, /phase: "ready"/);
  assert.match(source, /closeClaudeChat/);
  assert.match(source, /listClaudeChats/);
});

test("a missing or stopped sync component does not take the whole workspace offline", async () => {
  const source = await readFile(new URL("./session.ts", import.meta.url), "utf8");
  assert.match(source, /isWorkspaceOfflineSyncError/);
  assert.match(source, /SSH_UNREACHABLE/);
  assert.match(source, /if \(sync\?\.state === "fail" && isWorkspaceOfflineSyncError\(sync\.errorCode\)\)/);
});

test("activateClaudeProject keeps a ready workspace mounted instead of bouncing to resume", async () => {
  const source = await readFile(new URL("./session.ts", import.meta.url), "utf8");
  assert.match(source, /currentPhase === "ready"/);
  assert.match(
    source,
    /if \(currentPhase === "ready"\)[\s\S]*?resumeClaudeSync\(projectId\)[\s\S]*?refreshClaudeFiles\(projectId\)[\s\S]*?return;/,
  );
});
