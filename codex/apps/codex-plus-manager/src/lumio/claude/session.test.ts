import assert from "node:assert/strict";
import test from "node:test";

import { cancelClaudeConnect, runConnectSync } from "./session.ts";
import { dispatchClaude, getClaudeState, resetClaudeStore } from "./store.ts";

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
