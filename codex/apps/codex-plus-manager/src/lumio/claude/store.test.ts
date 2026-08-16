import assert from "node:assert/strict";
import test from "node:test";

import { persistableClaudeState } from "./machine.ts";
import {
  dispatchClaude,
  getClaudeState,
  rememberProjectPassword,
  resetClaudeStore,
  setDraftPassword,
  subscribeClaudeStore,
} from "./store.ts";

test("the module store keeps an in-flight connect sheet after a consumer unmounts", () => {
  resetClaudeStore();
  const seen: string[] = [];
  const stop = subscribeClaudeStore(() => {
    seen.push(getClaudeState().sheet?.step ?? "none");
  });
  stop();

  dispatchClaude({ type: "entitlement-resolved", entitlement: { status: "active", source: "local" } });
  dispatchClaude({ type: "open-connect" });
  dispatchClaude({ type: "draft-updated", draft: { host: "43.156.20.8" } });
  dispatchClaude({ type: "probe-started" });

  const afterUnmount = getClaudeState();
  assert.equal(afterUnmount.page, "empty");
  assert.equal(afterUnmount.sheet?.step, "probe");
  assert.equal(afterUnmount.sheet?.draft.host, "43.156.20.8");
  assert.equal(afterUnmount.sheet?.probeStatus, "running");
});

test("getClaudeState returns the same module singleton across reads", () => {
  resetClaudeStore();
  dispatchClaude({ type: "entitlement-resolved", entitlement: { status: "trialing", source: "local" } });
  const first = getClaudeState();
  const second = getClaudeState();
  assert.equal(first, second);
  assert.equal(first.entitlement.status, "trialing");
});

test("resetClaudeStore forgets the persisted snapshot", () => {
  resetClaudeStore();
  dispatchClaude({ type: "entitlement-resolved", entitlement: { status: "active", source: "local" } });
  dispatchClaude({
    type: "sync-finished",
    ok: true,
    project: {
      id: "p1",
      name: "my-project",
      host: "1.2.3.4",
      user: "root",
      port: 22,
      auth: "password",
      keyPath: null,
      hostAlias: null,
      remoteRoot: "/root/bestcodex/my-project",
      localRoot: "~/BestCodex/my-project",
      createdAt: "2026-08-16T00:00:00.000Z",
    },
  });
  if (typeof localStorage !== "undefined") {
    assert.ok((JSON.parse(localStorage.getItem("bestcodex.claude.v1") ?? "{}") as { projects?: unknown[] }).projects?.length);
  }
  resetClaudeStore();
  assert.equal(getClaudeState().projects.length, 0);
  if (typeof localStorage !== "undefined") {
    assert.equal(localStorage.getItem("bestcodex.claude.v1"), null);
  }
});

test("persistable Claude JSON has no password or secret fields", () => {
  resetClaudeStore();
  setDraftPassword("hunter2-secret");
  rememberProjectPassword("p1", "hunter2-secret");
  dispatchClaude({ type: "entitlement-resolved", entitlement: { status: "active", source: "local" } });
  dispatchClaude({
    type: "sync-finished",
    ok: true,
    project: {
      id: "p1",
      name: "my-project",
      host: "1.2.3.4",
      user: "root",
      port: 22,
      auth: "password",
      keyPath: null,
      hostAlias: null,
      remoteRoot: "/root/bestcodex/my-project",
      localRoot: "~/BestCodex/my-project",
      createdAt: "2026-08-16T00:00:00.000Z",
    },
  });
  const json = JSON.stringify(persistableClaudeState(getClaudeState()));
  assert.doesNotMatch(json, /"password"\s*:/);
  assert.doesNotMatch(json, /secret/i);
  assert.doesNotMatch(json, /hunter2/);
});

test("sync progress survives after every subscriber unsubscribes", () => {
  resetClaudeStore();
  dispatchClaude({ type: "entitlement-resolved", entitlement: { status: "active", source: "local" } });
  dispatchClaude({ type: "open-connect" });
  dispatchClaude({ type: "start-sync" });
  const stop = subscribeClaudeStore(() => {});
  stop();
  dispatchClaude({ type: "sync-progress", filesDone: 3, filesTotal: 10 });
  assert.equal(getClaudeState().sheet?.sync.filesDone, 3);
  assert.equal(getClaudeState().sheet?.step, "sync");
});
