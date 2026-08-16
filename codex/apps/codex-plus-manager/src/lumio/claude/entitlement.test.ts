import assert from "node:assert/strict";
import test from "node:test";

import {
  CLAUDE_CONTROL_API,
  fetchClaudeEntitlementFromControlPlane,
  hasClaudeEntitlement,
  resolveClaudeEntitlement,
} from "./entitlement.ts";

test("only active and trialing entitlements unlock Claude", () => {
  assert.equal(hasClaudeEntitlement({ status: "active" }), true);
  assert.equal(hasClaudeEntitlement({ status: "trialing" }), true);
  assert.equal(hasClaudeEntitlement({ status: "none" }), false);
  assert.equal(hasClaudeEntitlement({ status: "expired" }), false);
  assert.equal(hasClaudeEntitlement(null), false);
});

test("a remote control-plane snapshot wins over a stale local one", () => {
  const resolved = resolveClaudeEntitlement({
    account: { email: "a@b.c", balance: 1, planLabel: null },
    remote: { status: "active", source: "control-plane" },
    local: { status: "none", source: "local" },
  });
  assert.equal(resolved.status, "active");
  assert.equal(resolved.source, "control-plane");
});

test("a persisted local snapshot is used when the control plane is silent", () => {
  const resolved = resolveClaudeEntitlement({
    account: { email: "a@b.c", balance: 1, planLabel: "Pro" },
    remote: null,
    local: { status: "trialing", source: "local" },
  });
  assert.equal(resolved.status, "trialing");
  assert.equal(resolved.source, "local");
});

test("a Codex plan label is not treated as a Claude subscription", () => {
  const resolved = resolveClaudeEntitlement({
    account: { email: "a@b.c", balance: 9, planLabel: "Pro" },
    remote: null,
    local: null,
  });
  assert.equal(resolved.status, "none");
});

test("a Claude-shaped plan label can unlock when nothing else is readable", () => {
  const resolved = resolveClaudeEntitlement({
    account: { email: "a@b.c", balance: 0, planLabel: "Claude 月付" },
    remote: null,
    local: null,
  });
  assert.equal(resolved.status, "active");
  assert.equal(resolved.source, "account");
});

test("missing account and snapshots stay unsubscribed", () => {
  const resolved = resolveClaudeEntitlement({ account: null, remote: null, local: null });
  assert.equal(resolved.status, "none");
});

test("control-plane fetch reads entitlement status from the CC API", async () => {
  const calls: string[] = [];
  const fetcher = async (input: string | URL) => {
    calls.push(String(input));
    return new Response(JSON.stringify({ status: "active", days_left: 12 }), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    });
  };
  const remote = await fetchClaudeEntitlementFromControlPlane(fetcher);
  assert.equal(remote?.status, "active");
  assert.equal(remote?.source, "control-plane");
  assert.equal(calls[0], `${CLAUDE_CONTROL_API}/api/v1/me/entitlement`);
});

test("a failed control-plane fetch does not invent an entitlement", async () => {
  const fetcher = async () => new Response("no", { status: 401 });
  assert.equal(await fetchClaudeEntitlementFromControlPlane(fetcher), null);
});

test("control-plane envelopes unwrap data.status", async () => {
  const fetcher = async () =>
    new Response(JSON.stringify({ data: { status: "trialing", days_left: 3 } }), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    });
  const remote = await fetchClaudeEntitlementFromControlPlane(fetcher);
  assert.equal(remote?.status, "trialing");
  assert.equal(remote?.source, "control-plane");
});
