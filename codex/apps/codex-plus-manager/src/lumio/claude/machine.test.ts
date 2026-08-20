import assert from "node:assert/strict";
import test from "node:test";

import {
  CONNECT_STEPS,
  createProjectFromDraft,
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
  assert.equal(probing.sheet?.step, "probe");
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
  assert.equal(state.stageTab, "terminal");
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
      host: "43.156.20.8",
      user: "root",
      port: 22,
      auth: "password",
      keyPath: "",
      hostAlias: "",
      projectName: "my-project",
    },
    "proj-1",
    "2026-08-16T00:00:00.000Z",
  );
  assert.equal(created.remoteRoot, "/root/bestcodex/my-project");
  assert.equal(created.localRoot, "~/BestCodex/my-project");
});
