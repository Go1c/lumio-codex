/**
 * Account auth flow controllers used by UI (and unit-tested without DOM).
 */

import { authErrors, challengePolicy } from "@fns/control-api";
import * as api from "../../lib/control-api-client.js";
import {
  storeAccessToken,
  storeRefreshInMemory,
  assertSafeTokenPlacement,
  clearAllSession,
  loadRefreshFromMemory,
  storeSessionId,
  sanitizeErrorForReport,
} from "../../lib/session-storage.js";
import {
  antiEnumerationMessage,
  userMessageForCode,
  type AccountErrorKind,
  mapErrorCode,
} from "../../lib/errors.js";
import {
  extractChallengeCode,
  validateChallengeCode,
  validateEmail,
  validatePassword,
  resendAfterSeconds,
} from "../../lib/validators.js";

export type FlowResult =
  | { ok: true; next: string; data?: Record<string, unknown> }
  | { ok: false; kind: AccountErrorKind; message: string; retryAfterSeconds?: number };

function fail(code: string, retryAfterSeconds?: number): FlowResult {
  const kind = mapErrorCode(code);
  // Login-adjacent anti-enumeration codes always surface shared message.
  if (
    code === authErrors.loginFailureCode ||
    code === authErrors.forgotPasswordAcceptedCode
  ) {
    return {
      ok: false,
      kind: "invalid_credentials",
      message: antiEnumerationMessage(),
      retryAfterSeconds,
    };
  }
  return { ok: false, kind, message: userMessageForCode(code), retryAfterSeconds };
}

function offlineFail(): FlowResult {
  return {
    ok: false,
    kind: "offline",
    message: userMessageForCode(undefined).includes("offline")
      ? "You appear to be offline. Check your connection and try again."
      : "You appear to be offline. Check your connection and try again.",
  };
}

function isOffline(): boolean {
  return typeof navigator !== "undefined" && navigator.onLine === false;
}

export async function registerFlow(email: string, password: string): Promise<FlowResult> {
  if (isOffline()) return offlineFail();
  const e = validateEmail(email);
  if (!e.ok) return { ok: false, kind: "validation", message: e.message };
  const p = validatePassword(password);
  if (!p.ok) return { ok: false, kind: "validation", message: p.message };
  const r = await api.register(e.value, p.value);
  if (!r.ok) return fail(r.code);
  return {
    ok: true,
    next: "verify",
    data: {
      email: e.value,
      resendAfterSeconds: resendAfterSeconds(),
    },
  };
}

export async function verifyFlow(email: string, code: string): Promise<FlowResult> {
  if (isOffline()) return offlineFail();
  const e = validateEmail(email);
  if (!e.ok) return { ok: false, kind: "validation", message: e.message };
  const c = validateChallengeCode(code);
  if (!c.ok) return { ok: false, kind: "validation", message: c.message };
  const r = await api.verifyEmail(e.value, c.value);
  if (!r.ok) return fail(r.code);
  // Refresh stays in memory only — never LocalStorage/URL/logs.
  assertSafeTokenPlacement("access", "localStorage");
  assertSafeTokenPlacement("refresh", "memory");
  storeAccessToken(r.data.accessToken);
  storeRefreshInMemory(r.data.refreshToken);
  if (r.data.sessionId) storeSessionId(r.data.sessionId);
  return { ok: true, next: "home", data: { sessionId: r.data.sessionId } };
}

export async function resendFlow(email: string): Promise<FlowResult> {
  if (isOffline()) return offlineFail();
  const e = validateEmail(email);
  if (!e.ok) return { ok: false, kind: "validation", message: e.message };
  const r = await api.resendVerification(e.value);
  if (!r.ok) return fail(r.code, r.retryAfterSeconds);
  return {
    ok: true,
    next: "verify",
    data: { resendAfterSeconds: r.data.retryAfterSeconds ?? resendAfterSeconds() },
  };
}

export async function loginFlow(email: string, password: string): Promise<FlowResult> {
  if (isOffline()) return offlineFail();
  const e = validateEmail(email);
  if (!e.ok) return { ok: false, kind: "validation", message: e.message };
  const p = validatePassword(password);
  if (!p.ok) return { ok: false, kind: "validation", message: p.message };
  const r = await api.login(e.value, p.value);
  if (!r.ok) {
    // Anti-enumeration: unknown email and wrong password share message.
    if (r.code === authErrors.loginFailureCode) {
      return {
        ok: false,
        kind: "invalid_credentials",
        message: antiEnumerationMessage(),
      };
    }
    return fail(r.code);
  }
  assertSafeTokenPlacement("refresh", "memory");
  storeAccessToken(r.data.accessToken);
  storeRefreshInMemory(r.data.refreshToken);
  if (r.data.sessionId) storeSessionId(r.data.sessionId);
  return { ok: true, next: "home", data: { sessionId: r.data.sessionId } };
}

export async function forgotFlow(email: string): Promise<FlowResult> {
  if (isOffline()) return offlineFail();
  const e = validateEmail(email);
  if (!e.ok) return { ok: false, kind: "validation", message: e.message };
  const r = await api.forgotPassword(e.value);
  if (!r.ok) return fail(r.code);
  // Same UX whether email exists — surface shared recovery copy.
  return {
    ok: true,
    next: "reset",
    data: {
      email: e.value,
      message: antiEnumerationMessage(),
      code: r.data.code,
    },
  };
}

export async function resetFlow(
  email: string,
  code: string,
  password: string,
): Promise<FlowResult> {
  if (isOffline()) return offlineFail();
  const e = validateEmail(email);
  if (!e.ok) return { ok: false, kind: "validation", message: e.message };
  const c = validateChallengeCode(code);
  if (!c.ok) return { ok: false, kind: "validation", message: c.message };
  const p = validatePassword(password);
  if (!p.ok) return { ok: false, kind: "validation", message: p.message };
  const r = await api.resetPassword(e.value, c.value, p.value);
  if (!r.ok) return fail(r.code);
  clearAllSession();
  return { ok: true, next: "login" };
}

export async function refreshFlow(): Promise<FlowResult> {
  if (isOffline()) return offlineFail();
  const rt = loadRefreshFromMemory();
  if (!rt) {
    return { ok: false, kind: "unknown", message: userMessageForCode("session_expired") };
  }
  const r = await api.refresh(rt);
  if (!r.ok) {
    clearAllSession();
    return fail(r.code);
  }
  assertSafeTokenPlacement("refresh", "memory");
  storeAccessToken(r.data.accessToken);
  storeRefreshInMemory(r.data.refreshToken);
  return { ok: true, next: "home" };
}

/** Paste-friendly: extract 6-digit code from clipboard text. */
export function extractCodeFromPaste(text: string): string | null {
  return extractChallengeCode(text);
}

/** Resend countdown seconds from now until allowed. */
export function resendCountdown(retryAfterSeconds: number, elapsedSeconds = 0): number {
  return Math.max(0, Math.ceil(retryAfterSeconds - elapsedSeconds));
}

/** Gate: resend only when countdown is zero. */
export function canResendNow(retryAfterSeconds: number, elapsedSeconds = 0): boolean {
  return resendCountdown(retryAfterSeconds, elapsedSeconds) === 0;
}

export function challengeCodePolicy() {
  return {
    length: challengePolicy.emailCode.length,
    charset: challengePolicy.emailCode.charset,
    resendAfterSeconds: challengePolicy.emailCode.resendAfterSeconds,
    ttlSeconds: challengePolicy.emailCode.ttlSeconds,
    maxAttempts: challengePolicy.emailCode.maxAttempts,
  };
}

/** Safe logging helper — never include refresh tokens. */
export function safeFlowError(err: unknown): { message: string } {
  return sanitizeErrorForReport(err);
}
