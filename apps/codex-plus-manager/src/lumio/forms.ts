import type { LumioServiceSettings } from "./state.ts";

const EMAIL_PATTERN = /^[^\s@]+@[^\s@.]+(\.[^\s@.]+)+$/;
const MIN_PASSWORD_LENGTH = 8;
const VERIFY_CODE_LENGTH = 6;

export interface RegisterFormInput {
  email: string;
  verifyCode: string;
  password: string;
  confirmPassword: string;
  acceptedDocumentIds: string[];
}

export function isValidEmail(email: string): boolean {
  return EMAIL_PATTERN.test(email.trim());
}

export function emailSuffixError(email: string, whitelist: string[]): string | null {
  if (whitelist.length === 0) return null;
  const normalized = email.trim().toLowerCase();
  const allowed = whitelist.some((suffix) => normalized.endsWith(suffix.trim().toLowerCase()));
  return allowed ? null : "AUTH_EMAIL_DOMAIN_NOT_ALLOWED";
}

export function formatEmailSuffixHint(whitelist: string[]): string | null {
  if (whitelist.length === 0) return null;
  return `支持的邮箱：${whitelist.join("、")}`;
}

export function sanitizeVerifyCode(raw: string): string {
  return raw.replace(/\D/g, "").slice(0, VERIFY_CODE_LENGTH);
}

export function passwordStrength(password: string): "weak" | "medium" | "strong" {
  if (password.length < MIN_PASSWORD_LENGTH) return "weak";
  let variety = 0;
  if (/[a-z]/.test(password)) variety += 1;
  if (/[A-Z]/.test(password)) variety += 1;
  if (/\d/.test(password)) variety += 1;
  if (/[^A-Za-z0-9]/.test(password)) variety += 1;
  if (variety >= 3) return "strong";
  if (variety >= 2) return "medium";
  return "weak";
}

export function registerFormError(
  input: RegisterFormInput,
  settings: LumioServiceSettings,
): string | null {
  if (!isValidEmail(input.email)) return "AUTH_EMAIL_DOMAIN_NOT_ALLOWED";
  const suffixError = emailSuffixError(input.email, settings.emailSuffixWhitelist);
  if (suffixError !== null) return suffixError;
  if (settings.emailVerifyEnabled && input.verifyCode.length !== VERIFY_CODE_LENGTH) {
    return "AUTH_CODE_REQUIRED";
  }
  if (input.password.length < MIN_PASSWORD_LENGTH) return "PASSWORD_TOO_SHORT";
  if (input.password !== input.confirmPassword) return "PASSWORD_MISMATCH";
  if (settings.agreementEnabled) {
    const accepted = new Set(input.acceptedDocumentIds);
    const missing = settings.agreementDocuments.some((doc) => !accepted.has(doc.id));
    if (missing) return "AGREEMENTS_NOT_ACCEPTED";
  }
  return null;
}
