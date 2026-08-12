import type { LumioServiceSettings } from "./state.ts";

const EMAIL_PATTERN = /^[^\s@]+@[^\s@.]+(\.[^\s@.]+)+$/;
const DIGITS_ONLY_PATTERN = /^\d+$/;
const MIN_PASSWORD_LENGTH = 8;
const LONG_PASSWORD_LENGTH = 16;
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
  const at = normalized.lastIndexOf("@");
  // The whitelist may arrive either as "@example.com" or as a bare "example.com";
  // both must match the whole domain and nothing else.
  const domain = at === -1 ? null : normalized.slice(at);
  const allowed =
    domain !== null &&
    whitelist.some((suffix) => {
      const entry = suffix.trim().toLowerCase();
      if (entry === "" || entry === "@") return false;
      return (entry.startsWith("@") ? entry : `@${entry}`) === domain;
    });
  return allowed ? null : "AUTH_EMAIL_DOMAIN_NOT_ALLOWED";
}

export function formatEmailSuffixHint(whitelist: string[]): string | null {
  if (whitelist.length === 0) return null;
  return `支持的邮箱：${whitelist.join("、")}`;
}

export function sanitizeVerifyCode(raw: string): string {
  return raw.replace(/\D/g, "").slice(0, VERIFY_CODE_LENGTH);
}

function isCompleteVerifyCode(code: string): boolean {
  return code.length === VERIFY_CODE_LENGTH && DIGITS_ONLY_PATTERN.test(code);
}

export function passwordStrength(password: string): "weak" | "medium" | "strong" {
  if (password.length < MIN_PASSWORD_LENGTH) return "weak";
  let score = 0;
  if (/[a-z]/.test(password)) score += 1;
  if (/[A-Z]/.test(password)) score += 1;
  if (/\d/.test(password)) score += 1;
  if (/[^A-Za-z0-9]/.test(password)) score += 1;
  // A long passphrase earns a tier even when it draws on a single character class.
  if (password.length >= LONG_PASSWORD_LENGTH) score += 1;
  if (score >= 3) return "strong";
  if (score >= 2) return "medium";
  return "weak";
}

export function registerFormError(
  input: RegisterFormInput,
  settings: LumioServiceSettings,
): string | null {
  // EMAIL_FORMAT_INVALID is a client-side field code, deliberately absent from
  // LUMIO_ERROR_COPY; the server's suffix rejection below is a different condition.
  if (!isValidEmail(input.email)) return "EMAIL_FORMAT_INVALID";
  const suffixError = emailSuffixError(input.email, settings.emailSuffixWhitelist);
  if (suffixError !== null) return suffixError;
  if (settings.emailVerifyEnabled && !isCompleteVerifyCode(input.verifyCode)) {
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
