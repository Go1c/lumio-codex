import assert from "node:assert/strict";
import test from "node:test";

import {
  DEFAULT_CLAUDE_PLAN_CENTS,
  cancelClaudeConnect,
  formatClaudeOrderYuan,
  formatClaudePlanYuan,
  runConnectSync,
} from "./session.ts";
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

test("plan fallback is 19.9 yuan and order amounts keep two decimals", () => {
  assert.equal(DEFAULT_CLAUDE_PLAN_CENTS, 1990);
  assert.equal(formatClaudePlanYuan(1990), "19.9");
  assert.equal(formatClaudeOrderYuan(1990), "19.90");
  assert.notEqual(formatClaudePlanYuan(DEFAULT_CLAUDE_PLAN_CENTS), "68");
});
