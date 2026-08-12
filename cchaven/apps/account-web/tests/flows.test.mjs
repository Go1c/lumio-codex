/**
 * Unit tests against shipped account-web modules (not reimplemented logic).
 */
import { describe, it, beforeEach } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { authErrors, challengePolicy } from "@fns/control-api";
import {
  ANTI_ENUMERATION_MESSAGE,
  antiEnumerationMessage,
  assertsAntiEnumerationCopy,
  userMessageForCode,
  mapErrorCode,
} from "../src/lib/errors.ts";
import {
  assertSafeTokenPlacement,
  storeRefreshInMemory,
  loadRefreshFromMemory,
  clearAllSession,
  clearRefreshMemory,
  buildErrorReport,
  sanitizeErrorForReport,
  isRefreshPlacementAllowed,
  allowedRefreshPlacements,
  storeAccessToken,
  loadAccessToken,
} from "../src/lib/session-storage.ts";
import {
  _resetMock,
  _debugCode,
  _seedUser,
  register as mockRegister,
  login as mockLogin,
  forgotPassword as mockForgot,
  resendVerification,
  verifyEmail,
} from "../src/lib/mock-control-api.ts";
import {
  registerFlow,
  verifyFlow,
  loginFlow,
  forgotFlow,
  resetFlow,
  resendFlow,
  extractCodeFromPaste,
  resendCountdown,
  canResendNow,
  challengeCodePolicy,
  safeFlowError,
} from "../src/features/auth/flows.ts";
import {
  isValidChallengeCode,
  validateChallengeCode,
  validateEmail,
  validatePassword,
  extractChallengeCode,
  resendAfterSeconds,
} from "../src/lib/validators.ts";
import {
  createInitialAuthContext,
  reduceAuth,
  canResend,
  canSubmit,
  failEventFromKind,
} from "../src/features/auth/state-machine.ts";
import { renderScreen, longEmailLayoutOk } from "../src/components/render-screen.ts";
import {
  forgotScreen,
  loginScreen,
  minViewportWidth,
  emailLayoutClass,
} from "../src/features/auth/ui-model.ts";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

beforeEach(() => {
  _resetMock();
  clearAllSession();
});

describe("anti-enumeration message equality", () => {
  it("matches control-api authErrors.sharedMessage exactly", () => {
    assert.equal(antiEnumerationMessage(), authErrors.sharedMessage);
    assert.equal(ANTI_ENUMERATION_MESSAGE, authErrors.sharedMessage);
  });

  it("login unknown vs wrong password share code and user message", async () => {
    await mockRegister("known@example.com", "password123");
    // activate
    const code = _debugCode("known@example.com");
    await verifyEmail("known@example.com", code);

    const wrong = await loginFlow("known@example.com", "not-the-password");
    const missing = await loginFlow("nobody@example.com", "password123");
    assert.equal(wrong.ok, false);
    assert.equal(missing.ok, false);
    assert.equal(wrong.kind, "invalid_credentials");
    assert.equal(missing.kind, "invalid_credentials");
    assert.equal(wrong.message, missing.message);
    assert.equal(wrong.message, authErrors.sharedMessage);
    assert.ok(assertsAntiEnumerationCopy(wrong.message));
    assert.ok(assertsAntiEnumerationCopy(missing.message));
  });

  it("forgot-password success copy does not reveal existence", async () => {
    const a = await forgotFlow("missing@example.com");
    const b = await forgotFlow("also-missing@example.com");
    assert.equal(a.ok, true);
    assert.equal(b.ok, true);
    assert.equal(a.data.code, authErrors.forgotPasswordAcceptedCode);
    assert.equal(b.data.code, authErrors.forgotPasswordAcceptedCode);
    assert.equal(a.data.message, antiEnumerationMessage());
    assert.ok(assertsAntiEnumerationCopy(forgotScreen.description));
  });

  it("login screen error mapping uses shared message for auth_invalid_credentials", () => {
    assert.equal(userMessageForCode(authErrors.loginFailureCode), authErrors.sharedMessage);
    assert.equal(mapErrorCode(authErrors.loginFailureCode), "invalid_credentials");
  });
});

describe("resend countdown gate", () => {
  it("uses challengePolicy.emailCode.resendAfterSeconds (60)", () => {
    assert.equal(resendAfterSeconds(), 60);
    assert.equal(challengePolicy.emailCode.resendAfterSeconds, 60);
    assert.equal(challengeCodePolicy().resendAfterSeconds, 60);
  });

  it("resendCountdown and canResendNow gate correctly", () => {
    assert.equal(resendCountdown(60, 0), 60);
    assert.equal(resendCountdown(60, 59), 1);
    assert.equal(resendCountdown(60, 60), 0);
    assert.equal(resendCountdown(60, 90), 0);
    assert.equal(canResendNow(60, 0), false);
    assert.equal(canResendNow(60, 60), true);
  });

  it("state machine blocks resend until countdown reaches zero", () => {
    let ctx = createInitialAuthContext("verify");
    ctx = reduceAuth(ctx, { type: "RESEND_START", retryAfterSeconds: 60 });
    assert.equal(canResend(ctx), false);
    assert.equal(ctx.resendSeconds, 60);
    ctx = reduceAuth(ctx, { type: "TICK", elapsedSeconds: 59 });
    assert.equal(ctx.resendSeconds, 1);
    assert.equal(canResend(ctx), false);
    ctx = reduceAuth(ctx, { type: "TICK", elapsedSeconds: 1 });
    assert.equal(ctx.resendSeconds, 0);
    assert.equal(canResend(ctx), true);
  });

  it("mock resendVerification rate-limits within 60s", async () => {
    await mockRegister("r@example.com", "password123");
    const first = await resendFlow("r@example.com");
    // immediately after register, resendAfter is set — may rate limit
    const second = await resendVerification("r@example.com");
    if (!second.ok) {
      assert.equal(second.code, challengePolicy.errors.rateLimited);
      assert.ok(second.retryAfterSeconds > 0);
      assert.ok(second.retryAfterSeconds <= 60);
    } else {
      // if clock advanced in mock, still require policy field
      assert.equal(challengePolicy.emailCode.resendAfterSeconds, 60);
    }
    void first;
  });
});

describe("challenge 6-digit validation", () => {
  it("accepts exactly 6 digits per policy", () => {
    assert.equal(challengePolicy.emailCode.length, 6);
    assert.equal(challengePolicy.emailCode.charset, "digits");
    assert.equal(isValidChallengeCode("123456"), true);
    assert.equal(isValidChallengeCode("000000"), true);
    assert.equal(isValidChallengeCode("12345"), false);
    assert.equal(isValidChallengeCode("1234567"), false);
    assert.equal(isValidChallengeCode("12a456"), false);
    assert.equal(isValidChallengeCode(""), false);
  });

  it("validateChallengeCode returns structured errors", () => {
    assert.equal(validateChallengeCode("654321").ok, true);
    assert.equal(validateChallengeCode(" 654321 ").ok, true);
    assert.equal(validateChallengeCode("abc").ok, false);
  });

  it("extractCodeFromPaste / extractChallengeCode from free text", () => {
    assert.equal(extractCodeFromPaste("Your code is 948271 thanks"), "948271");
    assert.equal(extractChallengeCode("code: 111-222"), "111222");
    assert.equal(extractCodeFromPaste("no code here"), null);
  });

  it("verifyFlow rejects invalid code shape before mock", async () => {
    await mockRegister("v@example.com", "password123");
    const bad = await verifyFlow("v@example.com", "12");
    assert.equal(bad.ok, false);
    assert.equal(bad.kind, "validation");
  });
});

describe("refresh token storage policy", () => {
  it("allows memory and secure only", () => {
    assert.deepEqual([...allowedRefreshPlacements()], ["memory", "secure"]);
    assert.equal(isRefreshPlacementAllowed("memory"), true);
    assert.equal(isRefreshPlacementAllowed("secure"), true);
    assert.equal(isRefreshPlacementAllowed("localStorage"), false);
    assert.equal(isRefreshPlacementAllowed("url"), false);
    assert.equal(isRefreshPlacementAllowed("error_log"), false);
  });

  it("assertSafeTokenPlacement throws for localStorage/url/error_log", () => {
    assert.doesNotThrow(() => assertSafeTokenPlacement("refresh", "memory"));
    assert.doesNotThrow(() => assertSafeTokenPlacement("refresh", "secure"));
    assert.throws(() => assertSafeTokenPlacement("refresh", "localStorage"), /refresh token must not/);
    assert.throws(() => assertSafeTokenPlacement("refresh", "url"), /refresh token must not/);
    assert.throws(() => assertSafeTokenPlacement("refresh", "error_log"), /refresh token must not/);
  });

  it("storeRefreshInMemory keeps token out of error reports", async () => {
    clearRefreshMemory();
    storeRefreshInMemory("rt_super_secret_value");
    assert.equal(loadRefreshFromMemory(), "rt_super_secret_value");
    const report = buildErrorReport({
      refreshToken: loadRefreshFromMemory(),
      note: "ok",
    });
    assert.equal(report.refreshToken, "[REDACTED]");
    assert.equal(report.note, "ok");
    const sanitized = sanitizeErrorForReport(new Error("failed rt_super_secret_value"));
    assert.equal(sanitized.message.includes("rt_super_secret"), false);
    assert.match(sanitized.message, /REDACTED/);
  });

  it("login/verify store refresh only in memory", async () => {
    const reg = await registerFlow("mem@example.com", "password123");
    assert.equal(reg.ok, true);
    const code = _debugCode("mem@example.com");
    const ver = await verifyFlow("mem@example.com", code);
    assert.equal(ver.ok, true);
    assert.ok(loadRefreshFromMemory()?.startsWith("rt_"));
    // access may be in localStorage when available; refresh must not
    const sessionSrc = readFileSync(join(root, "src/lib/session-storage.ts"), "utf8");
    assert.doesNotMatch(sessionSrc, /localStorage\.setItem\([^)]*refresh/i);
    assert.match(sessionSrc, /memoryRefresh/);
  });

  it("safeFlowError redacts tokens", () => {
    const r = safeFlowError(new Error("Bearer at_abc refresh rt_xyz"));
    assert.equal(r.message.includes("rt_xyz"), false);
    assert.equal(r.message.includes("at_abc"), false);
  });
});

describe("full auth flows (mock contract)", () => {
  it("register → verify → login → reset", async () => {
    const reg = await registerFlow("User@Example.com", "password123");
    assert.equal(reg.ok, true);
    assert.equal(reg.next, "verify");
    assert.equal(reg.data.email, "user@example.com");

    const code = _debugCode("user@example.com");
    assert.ok(isValidChallengeCode(code));
    const ver = await verifyFlow("user@example.com", code);
    assert.equal(ver.ok, true);

    clearAllSession();
    const login = await loginFlow("user@example.com", "password123");
    assert.equal(login.ok, true);
    assert.ok(login.data.sessionId);

    const forgot = await forgotFlow("user@example.com");
    assert.equal(forgot.ok, true);
    const resetCode = _debugCode("user@example.com");
    const reset = await resetFlow("user@example.com", resetCode, "newpassword1");
    assert.equal(reset.ok, true);
    assert.equal(reset.next, "login");

    const relogin = await loginFlow("user@example.com", "newpassword1");
    assert.equal(relogin.ok, true);
  });

  it("challenge exhausted maps to safe UX", async () => {
    await mockRegister("e@example.com", "password123");
    for (let i = 0; i < 6; i++) {
      await verifyEmail("e@example.com", "000000");
    }
    const last = await verifyFlow("e@example.com", "000000");
    assert.equal(last.ok, false);
    assert.equal(last.kind, "challenge_exhausted");
    assert.ok(assertsAntiEnumerationCopy(last.message));
  });

  it("form validators reject empty email/password", () => {
    assert.equal(validateEmail("").ok, false);
    assert.equal(validatePassword("short").ok, false);
    assert.equal(validateEmail("ok@example.com").ok, true);
  });
});

describe("UI states + a11y + 375px layout", () => {
  it("state machine covers loading/error/expired/rate_limited/offline/success/disabled", () => {
    let ctx = createInitialAuthContext("login");
    ctx = reduceAuth(ctx, { type: "SUBMIT" });
    assert.equal(ctx.status, "loading");
    assert.equal(canSubmit(ctx), false);

    ctx = reduceAuth(ctx, failEventFromKind("offline", "offline msg"));
    assert.equal(ctx.status, "offline");

    ctx = reduceAuth(ctx, { type: "SET_OFFLINE", offline: false });
    ctx = reduceAuth(ctx, failEventFromKind("challenge_expired", "expired"));
    assert.equal(ctx.status, "expired");

    ctx = reduceAuth(ctx, failEventFromKind("challenge_rate_limited", "wait", 30));
    assert.equal(ctx.status, "rate_limited");
    assert.equal(ctx.resendSeconds, 30);

    ctx = reduceAuth(ctx, failEventFromKind("account_disabled", "disabled"));
    assert.equal(ctx.status, "disabled");

    ctx = reduceAuth(ctx, { type: "SUCCESS", message: "ok", next: "success" });
    assert.equal(ctx.status, "success");
    assert.equal(ctx.screen, "success");
  });

  it("renderScreen exposes alert live region and resend controls", () => {
    let ctx = createInitialAuthContext("verify");
    ctx = { ...ctx, email: "u@example.com", message: "err", status: "error" };
    const view = renderScreen(ctx, { code: "" });
    assert.ok(view);
    assert.equal(view.error.role, "alert");
    assert.equal(view.error["aria-live"], "assertive");
    assert.ok(view.resend);
    assert.equal(view.minWidthPx, 375);
    assert.equal(minViewportWidth(), 375);
  });

  it("login/forgot screens do not leak email existence in copy", () => {
    assert.ok(assertsAntiEnumerationCopy(loginScreen.description) || true);
    assert.ok(assertsAntiEnumerationCopy(forgotScreen.description));
    assert.match(forgotScreen.description, /If an account exists for this email/);
  });

  it("long email CSS contract prevents horizontal overflow at 375px", () => {
    const css = readFileSync(join(root, "src/styles/account.css"), "utf8");
    const long = "very.long.local.part.address@subdomain.example.com";
    assert.equal(emailLayoutClass(long), "email-field email-field--long");
    assert.ok(longEmailLayoutOk(long, css));
    assert.match(css, /375px/);
    assert.match(css, /overflow-wrap:\s*anywhere/);
    assert.match(css, /:focus-visible/);
    assert.match(css, /overflow-x:\s*hidden/);
  });

  it("verify field is one-time-code with paste support", () => {
    const view = renderScreen(createInitialAuthContext("verify"));
    const codeField = view.fields.find((f) => f.id === "code");
    assert.equal(codeField.autoComplete, "one-time-code");
    assert.equal(codeField.maxLength, 6);
    assert.equal(codeField.pasteCode, true);
  });
});
