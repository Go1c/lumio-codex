/**
 * User-facing auth error mapping. Never reveals whether an email exists.
 */

import { authErrors } from "@fns/control-api";

export type AccountErrorKind =
  | "invalid_credentials"
  | "email_not_verified"
  | "challenge_expired"
  | "challenge_exhausted"
  | "challenge_rate_limited"
  | "challenge_invalid"
  | "account_locked"
  | "account_disabled"
  | "offline"
  | "server"
  | "validation"
  | "unknown";

/** Contract anti-enumeration shared message (login + forgot-password UX). */
export const ANTI_ENUMERATION_MESSAGE = authErrors.sharedMessage;

const MESSAGES: Record<AccountErrorKind, string> = {
  invalid_credentials: ANTI_ENUMERATION_MESSAGE,
  email_not_verified: "Please verify your email before signing in.",
  challenge_expired: "This verification code has expired. Request a new one.",
  challenge_exhausted: "Too many attempts. Request a new verification code.",
  challenge_rate_limited: "Please wait before requesting another code.",
  challenge_invalid: "That verification code is not valid.",
  account_locked: "This account is temporarily locked.",
  account_disabled: "This account is disabled.",
  offline: "You appear to be offline. Check your connection and try again.",
  server: "Something went wrong on our side. Please try again later.",
  validation: "Please check the form and try again.",
  unknown: "Something went wrong. Please try again.",
};

const CODE_MAP: Record<string, AccountErrorKind> = {
  [authErrors.loginFailureCode]: "invalid_credentials",
  [authErrors.registerConflictCode]: "invalid_credentials",
  [authErrors.forgotPasswordAcceptedCode]: "invalid_credentials",
  auth_email_not_verified: "email_not_verified",
  auth_account_locked: "account_locked",
  auth_account_disabled: "account_disabled",
  challenge_expired: "challenge_expired",
  challenge_attempts_exhausted: "challenge_exhausted",
  challenge_already_consumed: "challenge_invalid",
  challenge_rate_limited: "challenge_rate_limited",
  challenge_invalid: "challenge_invalid",
  challenge_purpose_mismatch: "challenge_invalid",
  rate_limited: "challenge_rate_limited",
  validation_failed: "validation",
  internal_error: "server",
};

export function mapErrorCode(code: string | undefined | null): AccountErrorKind {
  if (!code) return "unknown";
  return CODE_MAP[code] ?? "unknown";
}

export function userMessageFor(kind: AccountErrorKind): string {
  return MESSAGES[kind];
}

export function userMessageForCode(code: string | undefined | null): string {
  return userMessageFor(mapErrorCode(code));
}

/**
 * Login + forgot-password copy must use the same anti-enumeration message
 * whether the email exists or not (control-api authErrors.sharedMessage).
 */
export function antiEnumerationMessage(): string {
  return ANTI_ENUMERATION_MESSAGE;
}

/** Ensures copy never includes “email not found” / “account exists” leakage phrases. */
export function assertsAntiEnumerationCopy(message: string): boolean {
  const lower = message.toLowerCase();
  if (message === ANTI_ENUMERATION_MESSAGE) return true;
  if (lower.includes("if an account exists for this email")) return true;
  const leaks = [
    "email not found",
    "no account with",
    "account does not exist",
    "user not found",
    "email already registered",
    "unknown email",
    "no user with",
  ];
  return !leaks.some((p) => lower.includes(p));
}
