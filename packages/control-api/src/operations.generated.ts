/* GENERATED FILE — do not edit by hand. Source: contracts/control-plane/src/policy.json */
export const operations = {
  register: { method: "POST", path: "/v1/auth/register", auth: false },
  verifyEmail: { method: "POST", path: "/v1/auth/verify-email", auth: false },
  resendVerification: { method: "POST", path: "/v1/auth/resend-verification", auth: false },
  login: { method: "POST", path: "/v1/auth/login", auth: false },
  refresh: { method: "POST", path: "/v1/auth/refresh", auth: false },
  logout: { method: "POST", path: "/v1/auth/logout", auth: true },
  forgotPassword: { method: "POST", path: "/v1/auth/forgot-password", auth: false },
  resetPassword: { method: "POST", path: "/v1/auth/reset-password", auth: false },
  listSessions: { method: "GET", path: "/v1/sessions", auth: true },
  revokeSession: { method: "DELETE", path: "/v1/sessions/{sessionId}", auth: true },
  revokeAllSessions: { method: "POST", path: "/v1/sessions/revoke-all", auth: true },
  exchangeAgentToken: { method: "POST", path: "/v1/workspaces/{workspaceId}/agent-token", auth: true },
} as const;

export type OperationId = keyof typeof operations;
export const operationIds = Object.keys(operations) as OperationId[];
