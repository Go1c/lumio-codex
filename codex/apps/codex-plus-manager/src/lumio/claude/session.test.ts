import assert from "node:assert/strict";
import test from "node:test";

import { cancelClaudeConnect } from "./session.ts";
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
