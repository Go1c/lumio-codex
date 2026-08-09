const ACCOUNT_RECOVERY_CODES = new Set([
  "auth_required",
  "authentication_rejected",
  "forbidden",
  "insecure_credential",
  "credential_missing",
  "credential_integrity",
  "scope_mismatch",
  "client_type_mismatch",
  "credential_access",
]);

export function stableErrorCode(error: unknown): string | null {
  if (typeof error === "string") {
    const normalized = error.toLowerCase();
    return [...ACCOUNT_RECOVERY_CODES].find((code) => normalized.includes(code)) ??
      normalized.match(/[a-z][a-z0-9_]+/)?.[0] ??
      null;
  }
  if (error instanceof Error) return stableErrorCode(error.message);
  if (!error || typeof error !== "object") return null;
  if ("primary" in error) return stableErrorCode(error.primary);
  if ("code" in error) return stableErrorCode(error.code);
  return "message" in error ? stableErrorCode(error.message) : null;
}

export function isAuthenticationFailure(error: unknown): boolean {
  const code = stableErrorCode(error);
  return code !== null && ACCOUNT_RECOVERY_CODES.has(code);
}

export function accountConnectionFailureMessage(error: unknown): string {
  const code = stableErrorCode(error);
  switch (code) {
    case "authentication_rejected":
      return "The username or password was not accepted.";
    case "forbidden":
    case "scope_mismatch":
    case "client_type_mismatch":
      return "This account could not authorize workspace sync.";
    case "credential_integrity":
    case "insecure_credential":
      return "The saved sign-in is invalid. Connect the account again.";
    case "credential_access":
      return "macOS could not access the saved sign-in. Check Keychain access and try again.";
    case "tunnel_acquire_failed":
      return "The remote server could not be reached through SSH.";
    case "timeout":
      return "The server did not respond in time. Try again.";
    case "workspace_access_rejected":
      return "This account cannot access the selected workspace.";
    case "workspace_identity_mismatch":
      return "The remote server returned a different workspace. Check the project settings.";
    default:
      return code
        ? `Account connection did not finish (${code}).`
        : "Account connection did not finish. Check the server and try again.";
  }
}
