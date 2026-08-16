import assert from "node:assert/strict";
import test from "node:test";

import {
  CONNECT_STEPS,
  createProjectFromDraft,
  initialClaudeState,
  nextProjectName,
  persistableClaudeState,
  reduceClaudeState,
  resolveClaudeSurface,
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

test("the next project name increments when my-project is taken", () => {
  assert.equal(nextProjectName([]), "my-project");
  assert.equal(nextProjectName(["my-project"]), "my-project-2");
  assert.equal(nextProjectName(["my-project", "my-project-2"]), "my-project-3");
});

test("unavailable sync engine still creates the project as offline", () => {
  const created = project("docs-site");
  const state = apply([
    { type: "entitlement-resolved", entitlement: entitled },
    { type: "open-connect" },
    { type: "sync-finished", ok: true, project: created },
    {
      type: "project-sync-updated",
      projectId: created.id,
      sync: {
        state: "offline",
        filesDone: 0,
        filesTotal: 0,
        errorCode: "SYNC_ENGINE_UNAVAILABLE",
        conflicts: 0,
      },
    },
  ]);
  assert.equal(state.page, "workspace");
  assert.equal(state.syncByProject[created.id].state, "offline");
});

test("createProjectFromDraft writes BestCodex directory presets", () => {
  const created = createProjectFromDraft(
    {
      host: "43.156.20.8",
      user: "root",
      port: 22,
      auth: "password",
      keyPath: "",
      projectName: "my-project",
    },
    "proj-1",
    "2026-08-16T00:00:00.000Z",
  );
  assert.equal(created.remoteRoot, "/root/bestcodex/my-project");
  assert.equal(created.localRoot, "~/BestCodex/my-project");
});
