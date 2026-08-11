/**
 * Contract mock for account-web development (R-00006).
 * Implements Wave 1 auth flows without a live backend.
 */

import {
  operations,
  authErrors,
  challengePolicy,
} from "@fns/control-api";

export type MockUser = {
  email: string;
  password: string;
  state: "pending_email" | "active" | "locked" | "disabled";
  code?: string;
  codeExpiresAt?: number;
  resendAfter?: number;
  attempts?: number;
};

export type MockResult<T = unknown> =
  | { ok: true; data: T }
  | { ok: false; code: string; message: string; retryAfterSeconds?: number };

const users = new Map<string, MockUser>();
let nowMs = () => Date.now();

export function _setClock(fn: () => number): void {
  nowMs = fn;
}

export function _resetMock(): void {
  users.clear();
  nowMs = () => Date.now();
}

function norm(email: string): string {
  return email.trim().toLowerCase();
}

function sixDigit(): string {
  const len = challengePolicy.emailCode.length;
  let s = "";
  for (let i = 0; i < len; i++) {
    s += String(Math.floor(Math.random() * 10));
  }
  return s;
}

function antiMessage(): string {
  return authErrors.sharedMessage;
}

function ttlMs(): number {
  return challengePolicy.emailCode.ttlSeconds * 1000;
}

function resendMs(): number {
  return challengePolicy.emailCode.resendAfterSeconds * 1000;
}

function maxAttempts(): number {
  return challengePolicy.emailCode.maxAttempts;
}

export async function register(
  email: string,
  password: string,
): Promise<MockResult<{ code: string }>> {
  void operations.register;
  const key = norm(email);
  if (!key || password.length < 8) {
    return { ok: false, code: "validation_failed", message: "Invalid form" };
  }
  // Anti-enumeration: same success shape whether or not email is new.
  if (!users.has(key)) {
    users.set(key, {
      email: key,
      password,
      state: "pending_email",
      code: sixDigit(),
      codeExpiresAt: nowMs() + ttlMs(),
      resendAfter: nowMs() + resendMs(),
      attempts: 0,
    });
  }
  return {
    ok: true,
    data: {
      code: authErrors.registerConflictCode,
    },
  };
}

export async function verifyEmail(
  email: string,
  code: string,
): Promise<MockResult<{ accessToken: string; refreshToken: string; sessionId: string }>> {
  void operations.verifyEmail;
  const u = users.get(norm(email));
  if (!u || u.state !== "pending_email") {
    return { ok: false, code: challengePolicy.errors.invalid, message: "invalid" };
  }
  if (u.codeExpiresAt != null && nowMs() > u.codeExpiresAt) {
    return { ok: false, code: challengePolicy.errors.expired, message: "expired" };
  }
  u.attempts = (u.attempts ?? 0) + 1;
  if (u.attempts > maxAttempts()) {
    return { ok: false, code: challengePolicy.errors.exhausted, message: "exhausted" };
  }
  if (u.code !== code) {
    return { ok: false, code: challengePolicy.errors.invalid, message: "invalid" };
  }
  u.state = "active";
  u.code = undefined;
  u.codeExpiresAt = undefined;
  return {
    ok: true,
    data: {
      accessToken: "at_mock",
      refreshToken: "rt_mock_secret",
      sessionId: "sess_verify",
    },
  };
}

export async function resendVerification(
  email: string,
): Promise<MockResult<{ retryAfterSeconds?: number }>> {
  void operations.resendVerification;
  const u = users.get(norm(email));
  // Always appear successful for unknown emails (anti-enumeration).
  if (!u) {
    return { ok: true, data: {} };
  }
  if (u.resendAfter && nowMs() < u.resendAfter) {
    return {
      ok: false,
      code: challengePolicy.errors.rateLimited,
      message: "wait",
      retryAfterSeconds: Math.ceil((u.resendAfter - nowMs()) / 1000),
    };
  }
  u.code = sixDigit();
  u.codeExpiresAt = nowMs() + ttlMs();
  u.resendAfter = nowMs() + resendMs();
  u.attempts = 0;
  return { ok: true, data: { retryAfterSeconds: challengePolicy.emailCode.resendAfterSeconds } };
}

export async function login(
  email: string,
  password: string,
): Promise<MockResult<{ accessToken: string; refreshToken: string; sessionId: string }>> {
  void operations.login;
  const u = users.get(norm(email));
  // Same failure code + message whether missing user or wrong password.
  if (!u || u.password !== password) {
    return {
      ok: false,
      code: authErrors.loginFailureCode,
      message: antiMessage(),
    };
  }
  if (u.state === "pending_email") {
    return { ok: false, code: "auth_email_not_verified", message: "verify" };
  }
  if (u.state === "locked") {
    return { ok: false, code: "auth_account_locked", message: "locked" };
  }
  if (u.state === "disabled") {
    return { ok: false, code: "auth_account_disabled", message: "disabled" };
  }
  return {
    ok: true,
    data: {
      accessToken: "at_mock",
      refreshToken: "rt_mock_secret",
      sessionId: "sess_mock",
    },
  };
}

export async function forgotPassword(email: string): Promise<MockResult<{ code: string }>> {
  void operations.forgotPassword;
  void email;
  const u = users.get(norm(email));
  if (u) {
    u.code = sixDigit();
    u.codeExpiresAt = nowMs() + ttlMs();
    u.attempts = 0;
  }
  // Always accepted — never reveal existence.
  return {
    ok: true,
    data: { code: authErrors.forgotPasswordAcceptedCode },
  };
}

export async function resetPassword(
  email: string,
  code: string,
  newPassword: string,
): Promise<MockResult> {
  void operations.resetPassword;
  const u = users.get(norm(email));
  if (!u || !u.code) {
    return { ok: false, code: challengePolicy.errors.invalid, message: "invalid" };
  }
  if (u.codeExpiresAt != null && nowMs() > u.codeExpiresAt) {
    return { ok: false, code: challengePolicy.errors.expired, message: "expired" };
  }
  u.attempts = (u.attempts ?? 0) + 1;
  if (u.attempts > maxAttempts()) {
    return { ok: false, code: challengePolicy.errors.exhausted, message: "exhausted" };
  }
  if (u.code !== code) {
    return { ok: false, code: challengePolicy.errors.invalid, message: "invalid" };
  }
  if (newPassword.length < 8) {
    return { ok: false, code: "validation_failed", message: "Invalid form" };
  }
  u.password = newPassword;
  u.code = undefined;
  u.codeExpiresAt = undefined;
  u.state = "active";
  return { ok: true, data: {} };
}

export async function refresh(
  refreshToken: string,
): Promise<MockResult<{ accessToken: string; refreshToken: string }>> {
  void operations.refresh;
  if (!refreshToken || !refreshToken.startsWith("rt_")) {
    return { ok: false, code: "session_expired", message: "expired" };
  }
  return {
    ok: true,
    data: {
      accessToken: "at_mock_rotated",
      refreshToken: "rt_mock_rotated",
    },
  };
}

export async function logout(): Promise<MockResult> {
  void operations.logout;
  return { ok: true, data: {} };
}

/** Test helper: last code for a user (capture-mailer equivalent). */
export function _debugCode(email: string): string | undefined {
  return users.get(norm(email))?.code;
}

/** Test helper: seed a user in a specific state. */
export function _seedUser(user: MockUser): void {
  users.set(norm(user.email), { ...user, email: norm(user.email) });
}
