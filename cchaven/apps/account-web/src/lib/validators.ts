/**
 * Form validators for account auth screens (email, password, 6-digit challenge).
 * Pure functions — unit-tested without DOM.
 */

import { challengePolicy } from "@fns/control-api";

const CODE_LENGTH = challengePolicy.emailCode.length;
const CODE_CHARSET = challengePolicy.emailCode.charset;

/** RFC-ish email shape; UI-level only (server remains authority). */
const EMAIL_RE = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

export type ValidationResult =
  | { ok: true; value: string }
  | { ok: false; message: string };

export function normalizeEmail(raw: string): string {
  return raw.trim().toLowerCase();
}

export function validateEmail(raw: string): ValidationResult {
  const value = normalizeEmail(raw);
  if (!value) {
    return { ok: false, message: "Email is required." };
  }
  if (!EMAIL_RE.test(value) || value.length > 254) {
    return { ok: false, message: "Enter a valid email address." };
  }
  return { ok: true, value };
}

export function validatePassword(raw: string, { minLength = 8 } = {}): ValidationResult {
  if (!raw) {
    return { ok: false, message: "Password is required." };
  }
  if (raw.length < minLength) {
    return { ok: false, message: `Password must be at least ${minLength} characters.` };
  }
  if (raw.length > 128) {
    return { ok: false, message: "Password is too long." };
  }
  return { ok: true, value: raw };
}

/** Exactly N digits per challengePolicy.emailCode (default 6). */
export function isValidChallengeCode(raw: string): boolean {
  if (typeof raw !== "string") return false;
  if (raw.length !== CODE_LENGTH) return false;
  if (CODE_CHARSET === "digits") {
    return /^\d+$/.test(raw);
  }
  return false;
}

export function validateChallengeCode(raw: string): ValidationResult {
  const value = raw.replace(/\s+/g, "");
  if (!value) {
    return { ok: false, message: "Verification code is required." };
  }
  if (!isValidChallengeCode(value)) {
    return {
      ok: false,
      message: `Enter the ${CODE_LENGTH}-digit code from your email.`,
    };
  }
  return { ok: true, value };
}

/**
 * Extract a challenge code from free-form paste (SMS/email body).
 * Prefers the first contiguous CODE_LENGTH digit run.
 */
export function extractChallengeCode(text: string): string | null {
  if (!text) return null;
  const collapsed = text.replace(/[\s\-]/g, "");
  const re = new RegExp(`(\\d{${CODE_LENGTH}})`);
  const m = collapsed.match(re);
  return m ? m[1] : null;
}

export function challengeCodeLength(): number {
  return CODE_LENGTH;
}

export function resendAfterSeconds(): number {
  return challengePolicy.emailCode.resendAfterSeconds;
}
