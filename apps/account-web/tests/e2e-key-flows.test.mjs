/**
 * Key account-flow E2E against shipped contract mock (no live backend).
 */
import { describe, it, beforeEach } from "node:test";
import assert from "node:assert/strict";

import { authErrors } from "@fns/control-api";
import { _resetMock, _debugCode } from "../src/lib/mock-control-api.ts";
import {
  registerFlow,
  verifyFlow,
  loginFlow,
  forgotFlow,
  resetFlow,
  refreshFlow,
  resendFlow,
} from "../src/features/auth/flows.ts";
import {
  clearAllSession,
  loadRefreshFromMemory,
  storeRefreshInMemory,
  assertSafeTokenPlacement,
  buildErrorReport,
} from "../src/lib/session-storage.ts";
import {
  createInitialAuthContext,
  reduceAuth,
  failEventFromKind,
} from "../src/features/auth/state-machine.ts";
import { antiEnumerationMessage } from "../src/lib/errors.ts";
import { renderScreen } from "../src/components/render-screen.ts";

beforeEach(() => {
  _resetMock();
  clearAllSession();
});

describe("key-flow E2E (mock)", () => {
  it("full path: register → verify → login → refresh (memory only)", async () => {
    const email = "e2e@example.com";
    const reg = await registerFlow(email, "password123");
    assert.equal(reg.ok, true);
    assert.equal(reg.next, "verify");

    const code = _debugCode(email);
    const ver = await verifyFlow(email, code);
    assert.equal(ver.ok, true);
    const mem = loadRefreshFromMemory();
    assert.ok(mem?.startsWith("rt_"));
    assert.doesNotThrow(() => assertSafeTokenPlacement("refresh", "memory"));

    clearAllSession();
    const login = await loginFlow(email, "password123");
    assert.equal(login.ok, true);
    assert.ok(login.data.sessionId);
    storeRefreshInMemory(loadRefreshFromMemory());

    const ref = await refreshFlow();
    assert.equal(ref.ok, true);
    assert.ok(loadRefreshFromMemory()?.startsWith("rt_"));
    const leaked = buildErrorReport({ refresh: loadRefreshFromMemory() });
    assert.equal(leaked.refresh, "[REDACTED]");
  });

  it("forgot → reset recovery path", async () => {
    await registerFlow("rec@example.com", "password123");
    const code = _debugCode("rec@example.com");
    await verifyFlow("rec@example.com", code);
    clearAllSession();

    const forgot = await forgotFlow("rec@example.com");
    assert.equal(forgot.ok, true);
    assert.equal(forgot.data.message, antiEnumerationMessage());
    assert.equal(forgot.data.code, authErrors.forgotPasswordAcceptedCode);

    const resetCode = _debugCode("rec@example.com");
    const reset = await resetFlow("rec@example.com", resetCode, "password456");
    assert.equal(reset.ok, true);

    const login = await loginFlow("rec@example.com", "password456");
    assert.equal(login.ok, true);
  });

  it("error states: expired / exhausted / rate_limited / offline / server", () => {
    const kinds = [
      ["challenge_expired", "expired"],
      ["challenge_exhausted", "error"],
      ["challenge_rate_limited", "rate_limited"],
      ["offline", "offline"],
      ["server", "error"],
      ["account_disabled", "disabled"],
    ];
    for (const [kind, status] of kinds) {
      const ev = failEventFromKind(kind, `msg-${kind}`, kind === "challenge_rate_limited" ? 60 : undefined);
      let ctx = createInitialAuthContext("verify");
      ctx = reduceAuth(ctx, ev);
      assert.equal(ctx.status, status, kind);
      const view = renderScreen(ctx);
      assert.ok(view);
      assert.match(view.statusClass, new RegExp(status));
    }
  });

  it("resend after register starts 60s gate in UI model", async () => {
    const reg = await registerFlow("cd@example.com", "password123");
    assert.equal(reg.data.resendAfterSeconds, 60);
    let ctx = createInitialAuthContext("verify");
    ctx = reduceAuth(ctx, {
      type: "RESEND_START",
      retryAfterSeconds: reg.data.resendAfterSeconds,
    });
    const view = renderScreen(ctx);
    assert.equal(view.resend.canResend, false);
    assert.equal(view.resend["data-countdown"], 60);
    // force resend via flow when not limited (seed new user without wait)
    _resetMock();
    await registerFlow("cd2@example.com", "password123");
    // second resend may be rate limited — acceptable
    const r = await resendFlow("cd2@example.com");
    assert.ok(r.ok === true || r.kind === "challenge_rate_limited");
  });
});
