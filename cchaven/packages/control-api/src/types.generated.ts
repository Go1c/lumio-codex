/* GENERATED FILE — do not edit by hand. Source: contracts/control-plane/src/policy.json */
export type UserState = "pending_email" | "active" | "locked" | "disabled";

export type StableErrorCode =
  | "auth_invalid_credentials"
  | "auth_registration_accepted"
  | "auth_recovery_accepted"
  | "auth_email_not_verified"
  | "auth_account_locked"
  | "auth_account_disabled"
  | "challenge_expired"
  | "challenge_attempts_exhausted"
  | "challenge_already_consumed"
  | "challenge_rate_limited"
  | "challenge_invalid"
  | "challenge_purpose_mismatch"
  | "session_expired"
  | "session_revoked"
  | "session_reuse_detected"
  | "token_invalid"
  | "token_audience_mismatch"
  | "token_issuer_mismatch"
  | "token_scope_missing"
  | "rate_limited"
  | "validation_failed"
  | "idempotency_conflict"
  | "not_found"
  | "internal_error";

export interface ApiError {
  error: true;
  code: StableErrorCode;
  message: string;
  requestId: string;
  details?: Record<string, unknown>;
  retryAfterSeconds?: number;
}

export interface TokenBoundaryConfig {
  issuer: string;
  audience: string;
  scopes: string[];
  lifetimeSeconds: number;
  storage: string;
  verificationEntry: string;
}
