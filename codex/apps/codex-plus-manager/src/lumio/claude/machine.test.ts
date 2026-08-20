import assert from "node:assert/strict";
import test from "node:test";

import {
  CONNECT_STEPS,
  createProjectFromDraft,
  decideRemoteProjectRoot,
  emptyHostDraft,
  initialClaudeState,
  nextProjectName,
  persistableClaudeState,
  reduceClaudeState,
  resolveClaudeSurface,
  sshFieldsForProbe,
} from "./machine.ts";

import type { ClaudeEntitlement, ClaudeEvent, ClaudeProject, ClaudeState } from "./types.ts";

const entitled: ClaudeEntitlement = { status: "active", source: "local" };
const none: ClaudeEntitlement = { status: "none", source: "local" };

function project(name = "my-project"): ClaudeProject {
  return {
    id: `p-${name}`,
    name,
    host: "43.156.20.8",
    user: "root",
    port: 22,
    auth: "password",
    keyPath: null,
    hostAlias: null,
    remoteRoot: `/root/bestcodex/${name}`,
    localRoot: `~/BestCodex/${name}`,
    createdAt: "2026-08-16T00:00:00.000Z",
  };
}

function apply(events: ClaudeEvent[], start: ClaudeState = initialClaudeState()): ClaudeState {
  return events.reduce((state, event) => reduceClaudeState(state, event), start);
}

test("no entitlement shows the subscribe card even when a project exists", () => {
  assert.equal(resolveClaudeSurface({ entitlement: none, projectCount: 0, sheetOpen: false }), "subscribe");
  assert.equal(resolveClaudeSurface({ entitlement: none, projectCount: 2, sheetOpen: true }), "subscribe");
  assert.equal(resolveClaudeSurface({ entitlement: { status: "expired" }, projectCount: 0, sheetOpen: false }), "subscribe");
});

test("control plane unreachable with cached projects opens the workspace read-only", () => {
  assert.equal(
    resolveClaudeSurface({
      entitlement: none,
      projectCount: 1,
      sheetOpen: false,
      controlUnreachable: true,
    }),
    "workspace",
  );
});

test("control plane unreachable with zero projects stays on subscribe", () => {
  assert.equal(
    resolveClaudeSurface({
      entitlement: none,
      projectCount: 0,
      sheetOpen: false,
      controlUnreachable: true,
    }),
    "subscribe",
  );
});

test("entitlement without projects shows the first-run empty page", () => {
  assert.equal(resolveClaudeSurface({ entitlement: entitled, projectCount: 0, sheetOpen: false }), "empty");
  assert.equal(
    resolveClaudeSurface({ entitlement: { status: "trialing" }, projectCount: 0, sheetOpen: false }),
    "empty",
  );
});

test("entitlement with projects shows the workspace", () => {
  assert.equal(resolveClaudeSurface({ entitlement: entitled, projectCount: 1, sheetOpen: false }), "workspace");
});

test("an open connect sheet sits on empty when there are no projects and on workspace when there are", () => {
  assert.equal(resolveClaudeSurface({ entitlement: entitled, projectCount: 0, sheetOpen: true }), "connect");
  assert.equal(resolveClaudeSurface({ entitlement: entitled, projectCount: 1, sheetOpen: true }), "connect");
});

test("the connect sheet walks host, probe, setup, then first sync", () => {
  assert.deepEqual(CONNECT_STEPS, ["host", "probe", "setup", "sync"]);

  const opened = apply([
    { type: "entitlement-resolved", entitlement: entitled },
    { type: "open-connect" },
  ]);
  assert.equal(opened.page, "empty");
  assert.equal(opened.sheet?.step, "host");
  assert.equal(opened.sheet?.draft.user, "root");
  assert.equal(opened.sheet?.draft.port, 22);

  const probing = reduceClaudeState(opened, { type: "probe-started" });
  assert.equal(probing.sheet?.step, "host");
  assert.equal(probing.sheet?.probeStatus, "running");

  const probed = reduceClaudeState(probing, {
    type: "probe-finished",
    result: {
      ok: true,
      reachable: true,
      authenticated: true,
      target: "43.156.20.8:22",
      user: "root",
      distro: "Ubuntu 22.04",
      cpu: "4",
      memory: "8 GB",
      errorCode: null,
      detail: null,
    },
  });
  assert.equal(probed.sheet?.probeStatus, "ok");
  assert.equal(probed.sheet?.step, "probe");

  const setup = reduceClaudeState(probed, { type: "continue-setup" });
  assert.equal(setup.sheet?.step, "setup");

  const syncing = reduceClaudeState(setup, { type: "start-sync" });
  assert.equal(syncing.sheet?.step, "sync");
  assert.equal(syncing.sheet?.sync.state, "running");
});

test("canceling the connect sheet returns to the empty page when there are no projects", () => {
  const canceled = apply([
    { type: "entitlement-resolved", entitlement: entitled },
    { type: "open-connect" },
    { type: "probe-started" },
    { type: "cancel-connect" },
  ]);
  assert.equal(canceled.sheet, null);
  assert.equal(canceled.page, "empty");
});

test("canceling a new-server sheet with existing projects returns to the workspace", () => {
  let state = initialClaudeState();
  state = reduceClaudeState(state, { type: "entitlement-resolved", entitlement: entitled });
  state = reduceClaudeState(state, {
    type: "projects-hydrated",
    projects: [project()],
    activeProjectId: "p-my-project",
  });
  state = reduceClaudeState(state, { type: "open-connect" });
  assert.equal(state.sheet?.step, "host");
  state = reduceClaudeState(state, { type: "cancel-connect" });
  assert.equal(state.sheet, null);
  assert.equal(state.page, "workspace");
  assert.equal(state.activeProjectId, "p-my-project");
});

test("pasting ssh root@host fills the host draft user and host", () => {
  const state = apply([
    { type: "entitlement-resolved", entitlement: entitled },
    { type: "open-connect" },
    { type: "ssh-pasted", text: "ssh root@43.156.20.8" },
  ]);
  assert.equal(state.sheet?.draft.host, "43.156.20.8");
  assert.equal(state.sheet?.draft.user, "root");
});

test("a failed probe stays on the probe step with a human-readable code", () => {
  const state = apply([
    { type: "entitlement-resolved", entitlement: entitled },
    { type: "open-connect" },
    { type: "probe-started" },
    {
      type: "probe-finished",
      result: {
        ok: false,
        reachable: true,
        authenticated: false,
        target: "43.156.20.8:22",
        user: "root",
        distro: null,
        cpu: null,
        memory: null,
        errorCode: "SSH_AUTH_FAILED",
        detail: "无法登录 43.156.20.8。",
      },
    },
  ]);
  assert.equal(state.sheet?.step, "probe");
  assert.equal(state.sheet?.probeStatus, "fail");
  assert.equal(state.sheet?.probe?.errorCode, "SSH_AUTH_FAILED");
});

test("finishing first sync closes the sheet and opens the workspace on that project", () => {
  const created = project("docs-site");
  const state = apply([
    { type: "entitlement-resolved", entitlement: entitled },
    { type: "open-connect" },
    { type: "start-sync" },
    { type: "sync-finished", ok: true, project: created },
  ]);
  assert.equal(state.sheet, null);
  assert.equal(state.page, "workspace");
  assert.equal(state.projects.length, 1);
  assert.equal(state.activeProjectId, created.id);
  assert.equal(state.statusDrawer, "closed");
  assert.equal(state.workspacePhaseByProject[created.id], "init");
  assert.deepEqual(state.sessionsByProject, {});
});

test("a failed first sync stays on the sheet with an error code", () => {
  const state = apply([
    { type: "entitlement-resolved", entitlement: entitled },
    { type: "open-connect" },
    { type: "start-sync" },
    { type: "sync-finished", ok: false, project: project(), errorCode: "SYNC_FAILED" },
  ]);
  assert.equal(state.page, "empty");
  assert.equal(state.sheet?.step, "sync");
  assert.equal(state.sheet?.sync.state, "fail");
  assert.equal(state.sheet?.sync.errorCode, "SYNC_FAILED");
  assert.equal(state.projects.length, 0);
});

test("selecting a project only changes the active id and keeps the rest of the session", () => {
  let state = initialClaudeState();
  state = reduceClaudeState(state, { type: "entitlement-resolved", entitlement: entitled });
  state = reduceClaudeState(state, {
    type: "projects-hydrated",
    projects: [project("my-project"), project("docs-site")],
    activeProjectId: "p-my-project",
  });
  state = reduceClaudeState(state, {
    type: "append-terminal",
    projectId: "p-my-project",
    line: { kind: "ok", text: "attached" },
  });
  state = reduceClaudeState(state, { type: "select-project", projectId: "p-docs-site" });
  assert.equal(state.activeProjectId, "p-docs-site");
  assert.equal(state.terminalByProject["p-my-project"][0]?.text, "attached");
  assert.equal(state.page, "workspace");
});

test("a persistable snapshot never includes a password field", () => {
  const state = apply([
    { type: "entitlement-resolved", entitlement: entitled },
    { type: "open-connect" },
    { type: "draft-updated", draft: { host: "10.0.0.1" } },
    { type: "sync-finished", ok: true, project: project() },
  ]);
  const persisted = JSON.stringify(persistableClaudeState(state));
  assert.doesNotMatch(persisted, /"password"\s*:/);
  assert.doesNotMatch(persisted, /secret/i);
});

test("paying and order history stay in session state and out of persistable snapshots", () => {
  const paying = apply([{ type: "pay-started" }]);
  assert.equal(paying.paying, true);
  const failed = reduceClaudeState(paying, {
    type: "pay-failed",
    errorCode: "ACCOUNT_INSUFFICIENT_BALANCE",
    forceRecharge: true,
  });
  assert.equal(failed.paying, false);
  assert.equal(failed.payMode, "recharge");
  const listed = reduceClaudeState(failed, {
    type: "orders-loaded",
    orders: [
      {
        orderNo: "BC1",
        amountCents: 1990,
        status: "paid",
        createdAt: "2026-08-19T00:00:00.000Z",
      },
    ],
  });
  assert.equal(listed.orders[0]?.amountCents, 1990);
  const persisted = JSON.stringify(persistableClaudeState(listed));
  assert.doesNotMatch(persisted, /"paying"/);
  assert.doesNotMatch(persisted, /"orders"/);
});

test("the next project name increments when my-project is taken", () => {
  assert.equal(nextProjectName([]), "my-project");
  assert.equal(nextProjectName(["my-project"]), "my-project-2");
  assert.equal(nextProjectName(["my-project", "my-project-2"]), "my-project-3");
});

test("a missing remote folder is created; an existing one asks reuse or a new name", () => {
  assert.deepEqual(decideRemoteProjectRoot("my-project", []), {
    action: "create",
    name: "my-project",
  });
  assert.deepEqual(decideRemoteProjectRoot("my-project", ["my-project"]), {
    action: "choose",
    existingName: "my-project",
    nextName: "my-project-2",
  });
  assert.deepEqual(decideRemoteProjectRoot("my-project", ["my-project", "my-project-2"]), {
    action: "choose",
    existingName: "my-project",
    nextName: "my-project-3",
  });
});

test("an existing remote folder pauses setup so the user can choose", () => {
  const state = apply([
    { type: "entitlement-resolved", entitlement: entitled },
    { type: "open-connect" },
    { type: "continue-setup" },
    {
      type: "setup-choose-root",
      existingName: "my-project",
      existingRoot: "~/bestcodex/my-project",
      nextName: "my-project-2",
      nextRoot: "~/bestcodex/my-project-2",
    },
    { type: "start-sync" },
  ]);
  assert.equal(state.sheet?.step, "setup");
  assert.equal(state.sheet?.setupStatus, "choose");
  assert.equal(state.sheet?.rootChoice?.nextName, "my-project-2");
  assert.equal(state.projects.length, 0);
});

test("continue-setup starts on the inspect phase so the sheet is not silent", () => {
  const state = apply([
    { type: "entitlement-resolved", entitlement: entitled },
    { type: "open-connect" },
    { type: "continue-setup" },
  ]);
  assert.equal(state.sheet?.setupStatus, "running");
  assert.equal(state.sheet?.setupProgress?.phase, "inspect");
  assert.equal(state.sheet?.setupProgress?.step, 1);
  assert.match(state.sheet?.setupProgress?.detail ?? "", /正在检查服务器/);
});

test("setup progress updates the live install phase without leaving running", () => {
  const state = apply([
    { type: "entitlement-resolved", entitlement: entitled },
    { type: "open-connect" },
    { type: "continue-setup" },
    {
      type: "setup-progress",
      phase: "upload",
      step: 3,
      total: 4,
      detail: "正在把同步组件传到服务器（1 / 2）…",
    },
  ]);
  assert.equal(state.sheet?.step, "setup");
  assert.equal(state.sheet?.setupStatus, "running");
  assert.equal(state.sheet?.setupProgress?.phase, "upload");
  assert.equal(state.sheet?.setupProgress?.step, 3);
  assert.equal(state.sheet?.setupProgress?.detail, "正在把同步组件传到服务器（1 / 2）…");
});

test("setup progress is ignored after setup has already finished", () => {
  const state = apply([
    { type: "entitlement-resolved", entitlement: entitled },
    { type: "open-connect" },
    { type: "continue-setup" },
    { type: "setup-finished", ok: true },
    {
      type: "setup-progress",
      phase: "upload",
      step: 3,
      total: 4,
      detail: "正在把同步组件传到服务器（2 / 2）…",
    },
  ]);
  assert.equal(state.sheet?.setupStatus, "ok");
  assert.notEqual(state.sheet?.setupProgress?.phase, "upload");
});

test("prepare failure does not advance the sheet to sync", () => {
  const state = apply([
    { type: "entitlement-resolved", entitlement: entitled },
    { type: "open-connect" },
    { type: "continue-setup" },
    {
      type: "setup-finished",
      ok: false,
      detail: "没能在服务器上装好同步组件。",
      errorCode: "SSH_PREPARE_FAILED",
    },
    { type: "start-sync" },
  ]);
  assert.equal(state.sheet?.step, "setup");
  assert.equal(state.sheet?.setupStatus, "fail");
  assert.equal(state.sheet?.setupDetail, "没能在服务器上装好同步组件。");
  assert.equal(state.projects.length, 0);
});

test("unconfirmed first-sync copy does not create a project", () => {
  const state = apply([
    { type: "entitlement-resolved", entitlement: entitled },
    { type: "open-connect" },
    { type: "start-sync" },
    {
      type: "sync-finished",
      ok: false,
      project: project("docs-site"),
      errorCode: "SYNC_COPY_UNCONFIRMED",
    },
  ]);
  assert.equal(state.page, "empty");
  assert.equal(state.sheet?.step, "sync");
  assert.equal(state.sheet?.sync.state, "fail");
  assert.equal(state.sheet?.sync.errorCode, "SYNC_COPY_UNCONFIRMED");
  assert.equal(state.projects.length, 0);
});

test("unavailable sync engine does not create a project", () => {
  const state = apply([
    { type: "entitlement-resolved", entitlement: entitled },
    { type: "open-connect" },
    { type: "start-sync" },
    {
      type: "sync-finished",
      ok: false,
      project: project("docs-site"),
      errorCode: "SYNC_ENGINE_UNAVAILABLE",
    },
  ]);
  assert.equal(state.page, "empty");
  assert.equal(state.sheet?.step, "sync");
  assert.equal(state.sheet?.sync.state, "fail");
  assert.equal(state.sheet?.sync.errorCode, "SYNC_ENGINE_UNAVAILABLE");
  assert.equal(state.projects.length, 0);
});

test("password connect mode does not send a leftover host alias", () => {
  const fields = sshFieldsForProbe({
    ...emptyHostDraft(),
    host: "43.156.20.8",
    hostAlias: "prod",
    auth: "password",
  });
  assert.equal(fields.host, "43.156.20.8");
  assert.equal(fields.hostAlias, "");
  assert.equal(fields.keyPath, "");
});

test("local SSH connect mode keeps the host alias without requiring an IP", () => {
  const fields = sshFieldsForProbe({
    ...emptyHostDraft(),
    host: "",
    hostAlias: "prod",
    auth: "config",
  });
  assert.equal(fields.hostAlias, "prod");
  assert.equal(fields.host, "");
});

test("createProjectFromDraft writes BestCodex directory presets", () => {
  const created = createProjectFromDraft(
    {
      ...emptyHostDraft(),
      host: "43.156.20.8",
      projectName: "my-project",
    },
    "proj-1",
    "2026-08-16T00:00:00.000Z",
  );
  assert.equal(created.remoteRoot, "~/bestcodex/my-project");
  assert.equal(created.localRoot, "~/BestCodex/my-project");
});

test("createProjectFromDraft keeps user-chosen folders", () => {
  const created = createProjectFromDraft(
    {
      ...emptyHostDraft(),
      projectName: "shop",
      localRoot: "/Users/me/code/shop",
      remoteRoot: "~/sites/shop",
    },
    "proj-2",
    "2026-08-16T00:00:00.000Z",
  );
  assert.equal(created.localRoot, "/Users/me/code/shop");
  assert.equal(created.remoteRoot, "~/sites/shop");
});

test("initial Claude state defaults new session fields without changing stageTab", () => {
  const state = initialClaudeState();
  assert.equal(state.statusDrawer, "closed");
  assert.equal("stageTab" in state, false);
  assert.deepEqual(state.sessionsByProject, {});
  assert.deepEqual(state.activeSessionByProject, {});
  assert.deepEqual(state.collapsedHosts, {});
  assert.deepEqual(state.cliByHost, {});
  assert.deepEqual(state.loginByHost, {});
  assert.deepEqual(state.workspacePhaseByProject, {});
});

test("open-session appends the session and makes it active", () => {
  const state = apply([
    { type: "open-session", projectId: "p-my-project", sessionId: "s1" },
    { type: "open-session", projectId: "p-my-project", sessionId: "s2" },
  ]);
  assert.equal(state.sessionsByProject["p-my-project"].length, 2);
  assert.equal(state.sessionsByProject["p-my-project"][1]?.id, "s2");
  assert.equal(state.sessionsByProject["p-my-project"][0]?.title, null);
  assert.equal(state.sessionsByProject["p-my-project"][0]?.titleLocked, false);
  assert.equal(state.sessionsByProject["p-my-project"][0]?.running, false);
  assert.equal(state.activeSessionByProject["p-my-project"], "s2");
});

test("close-session removes the session and keeps nextSessionId active", () => {
  const state = apply([
    { type: "open-session", projectId: "p-docs", sessionId: "s1" },
    { type: "open-session", projectId: "p-docs", sessionId: "s2" },
    { type: "close-session", projectId: "p-docs", sessionId: "s1", nextSessionId: "s2" },
  ]);
  assert.deepEqual(
    state.sessionsByProject["p-docs"].map((session) => session.id),
    ["s2"],
  );
  assert.equal(state.activeSessionByProject["p-docs"], "s2");
});

test("close-session synthesizes the next session when it is not already in the list", () => {
  const state = apply([
    { type: "open-session", projectId: "p-docs", sessionId: "s1" },
    { type: "close-session", projectId: "p-docs", sessionId: "s1", nextSessionId: "s2" },
  ]);
  assert.equal(state.sessionsByProject["p-docs"].length, 1);
  assert.equal(state.sessionsByProject["p-docs"][0]?.id, "s2");
  assert.equal(state.sessionsByProject["p-docs"][0]?.title, null);
  assert.equal(state.sessionsByProject["p-docs"][0]?.titleLocked, false);
  assert.equal(state.sessionsByProject["p-docs"][0]?.running, false);
  assert.equal(state.activeSessionByProject["p-docs"], "s2");
});

test("select-session only changes the active session id", () => {
  const opened = apply([
    { type: "open-session", projectId: "p-docs", sessionId: "s1" },
    { type: "open-session", projectId: "p-docs", sessionId: "s2" },
  ]);
  const state = reduceClaudeState(opened, {
    type: "select-session",
    projectId: "p-docs",
    sessionId: "s1",
  });
  assert.equal(state.activeSessionByProject["p-docs"], "s1");
  assert.equal(state.sessionsByProject["p-docs"].length, 2);
  assert.equal(state.statusDrawer, "closed");
});

test("session-title-locked writes the title and locks it", () => {
  const state = apply([
    { type: "open-session", projectId: "p-docs", sessionId: "s1" },
    { type: "session-title-locked", projectId: "p-docs", sessionId: "s1", title: "抽重试逻辑" },
  ]);
  assert.equal(state.sessionsByProject["p-docs"][0]?.title, "抽重试逻辑");
  assert.equal(state.sessionsByProject["p-docs"][0]?.titleLocked, true);
});

test("session-running toggles the running flag", () => {
  const state = apply([
    { type: "open-session", projectId: "p-docs", sessionId: "s1" },
    { type: "session-running", projectId: "p-docs", sessionId: "s1", running: true },
  ]);
  assert.equal(state.sessionsByProject["p-docs"][0]?.running, true);
});

test("toggle-server-group flips collapsedHosts for that host", () => {
  const collapsed = reduceClaudeState(initialClaudeState(), {
    type: "toggle-server-group",
    host: "108.80.81.15",
  });
  assert.equal(collapsed.collapsedHosts["108.80.81.15"], true);
  const opened = reduceClaudeState(collapsed, { type: "toggle-server-group", host: "108.80.81.15" });
  assert.equal(opened.collapsedHosts["108.80.81.15"], false);
});

test("toggle-server-group can set an explicit override so offline groups expand", () => {
  const opened = reduceClaudeState(initialClaudeState(), {
    type: "toggle-server-group",
    host: "192.168.1.40",
    collapsed: false,
  });
  assert.equal(opened.collapsedHosts["192.168.1.40"], false);
});

test("cli-install-progress merges status by host", () => {
  const detecting = reduceClaudeState(initialClaudeState(), {
    type: "cli-install-progress",
    host: "108.80.81.15",
    phase: "detect",
    version: "1.0.0",
  });
  assert.equal(detecting.cliByHost["108.80.81.15"]?.phase, "detect");
  assert.equal(detecting.cliByHost["108.80.81.15"]?.version, "1.0.0");
  const failed = reduceClaudeState(detecting, {
    type: "cli-install-progress",
    host: "108.80.81.15",
    phase: "fail",
    errorCode: "CLI_INSTALL_FAILED",
    detail: "没能装上 Claude",
  });
  assert.equal(failed.cliByHost["108.80.81.15"]?.phase, "fail");
  assert.equal(failed.cliByHost["108.80.81.15"]?.version, "1.0.0");
  assert.equal(failed.cliByHost["108.80.81.15"]?.errorCode, "CLI_INSTALL_FAILED");
  assert.equal(failed.cliByHost["108.80.81.15"]?.detail, "没能装上 Claude");
});

test("login-status merges by host", () => {
  const loggingIn = reduceClaudeState(initialClaudeState(), {
    type: "login-status",
    host: "108.80.81.15",
    phase: "logging-in",
  });
  assert.equal(loggingIn.loginByHost["108.80.81.15"]?.phase, "logging-in");
  const loggedIn = reduceClaudeState(loggingIn, {
    type: "login-status",
    host: "108.80.81.15",
    phase: "logged-in",
    errorCode: null,
  });
  assert.equal(loggedIn.loginByHost["108.80.81.15"]?.phase, "logged-in");
  assert.equal(loggedIn.loginByHost["108.80.81.15"]?.errorCode, null);
});

test("set-status-drawer and set-workspace-phase write through", () => {
  const drawer = reduceClaudeState(initialClaudeState(), {
    type: "set-status-drawer",
    pane: "conflicts",
  });
  assert.equal(drawer.statusDrawer, "conflicts");
  const phased = reduceClaudeState(drawer, {
    type: "set-workspace-phase",
    projectId: "p-docs",
    phase: "resume",
  });
  assert.equal(phased.workspacePhaseByProject["p-docs"], "resume");
  assert.equal(phased.statusDrawer, "conflicts");
});

test("session fields stay out of persistable snapshots", () => {
  const state = apply([
    { type: "open-session", projectId: "p-docs", sessionId: "s1" },
    { type: "set-status-drawer", pane: "server" },
  ]);
  const persisted = JSON.stringify(persistableClaudeState(state));
  assert.doesNotMatch(persisted, /"sessionsByProject"/);
  assert.doesNotMatch(persisted, /"statusDrawer"/);
  assert.doesNotMatch(persisted, /"cliByHost"/);
});
