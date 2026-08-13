/**
 * Live control-plane HTTP client (post Wave-1 integration).
 * Same MockResult shape as mock-control-api for drop-in switching.
 */

export type LiveResult<T = unknown> =
  | { ok: true; data: T }
  | { ok: false; code: string; message: string; retryAfterSeconds?: number };

function envGet(key: string): string | undefined {
  const g = globalThis as { process?: { env?: Record<string, string | undefined> } };
  return g.process?.env?.[key];
}

function baseURL(): string {
  if (typeof globalThis !== "undefined" && (globalThis as { CONTROL_PLANE_BASE?: string }).CONTROL_PLANE_BASE) {
    return (globalThis as { CONTROL_PLANE_BASE?: string }).CONTROL_PLANE_BASE!;
  }
  // Vite-style env when bundled
  try {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const env = (import.meta as any)?.env;
    if (env?.VITE_CONTROL_PLANE_BASE) return String(env.VITE_CONTROL_PLANE_BASE);
  } catch {
    /* node tests */
  }
  return envGet("CONTROL_PLANE_BASE") || envGet("VITE_CONTROL_PLANE_BASE") || "http://127.0.0.1:18088";
}

async function jfetch<T>(
  path: string,
  opts: { method?: string; body?: unknown; token?: string } = {},
): Promise<LiveResult<T>> {
  const headers: Record<string, string> = {
    "Content-Type": "application/json",
    "X-Request-Id": `web_${Date.now()}`,
  };
  if (opts.token) headers.Authorization = `Bearer ${opts.token}`;
  let res: Response;
  try {
    res = await fetch(`${baseURL()}${path}`, {
      method: opts.method || "GET",
      headers,
      body: opts.body !== undefined ? JSON.stringify(opts.body) : undefined,
    });
  } catch {
    return { ok: false, code: "offline", message: "You appear to be offline. Check your connection and try again." };
  }
  let json: Record<string, unknown> = {};
  try {
    json = (await res.json()) as Record<string, unknown>;
  } catch {
    json = {};
  }
  if (!res.ok || json.error === true) {
    return {
      ok: false,
      code: String(json.code || "internal_error"),
      message: String(json.message || "Something went wrong"),
      retryAfterSeconds: typeof json.retryAfterSeconds === "number" ? json.retryAfterSeconds : undefined,
    };
  }
  return { ok: true, data: json as T };
}

export async function register(email: string, password: string): Promise<LiveResult<{ code: string }>> {
  const r = await jfetch<{ code: string }>("/v1/auth/register", {
    method: "POST",
    body: { email, password },
  });
  return r;
}

export async function verifyEmail(
  email: string,
  code: string,
): Promise<LiveResult<{ accessToken: string; refreshToken: string; sessionId: string }>> {
  return jfetch("/v1/auth/verify-email", { method: "POST", body: { email, code } });
}

export async function resendVerification(email: string): Promise<LiveResult<Record<string, unknown>>> {
  return jfetch("/v1/auth/resend-verification", { method: "POST", body: { email } });
}

export async function login(
  email: string,
  password: string,
  device = "account-web",
): Promise<LiveResult<{ accessToken: string; refreshToken: string; sessionId: string }>> {
  return jfetch("/v1/auth/login", { method: "POST", body: { email, password, device } });
}

export async function forgotPassword(email: string): Promise<LiveResult<{ code: string }>> {
  return jfetch("/v1/auth/forgot-password", { method: "POST", body: { email } });
}

export async function resetPassword(
  email: string,
  code: string,
  newPassword: string,
): Promise<LiveResult<Record<string, unknown>>> {
  return jfetch("/v1/auth/reset-password", {
    method: "POST",
    body: { email, code, newPassword },
  });
}

export async function refresh(
  sessionId: string,
  refreshToken: string,
): Promise<LiveResult<{ accessToken: string; refreshToken: string; sessionId: string }>> {
  return jfetch("/v1/auth/refresh", {
    method: "POST",
    body: { sessionId, refreshToken },
  });
}

export async function revokeAll(accessToken: string): Promise<LiveResult<Record<string, unknown>>> {
  return jfetch("/v1/sessions/revoke-all", { method: "POST", body: {}, token: accessToken });
}

export async function exchangeAgentToken(
  workspaceId: string,
  accessToken: string,
): Promise<LiveResult<{ token: string; issuer: string; audience: string }>> {
  return jfetch(`/v1/workspaces/${encodeURIComponent(workspaceId)}/agent-token`, {
    method: "POST",
    body: {},
    token: accessToken,
  });
}

/** Factory: use live when CONTROL_PLANE_MODE=live or VITE_CONTROL_PLANE_MODE=live */
export function isLiveMode(): boolean {
  let viteMode: string | undefined;
  try {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    viteMode = (import.meta as any)?.env?.VITE_CONTROL_PLANE_MODE;
  } catch {
    /* ignore */
  }
  const mode = envGet("CONTROL_PLANE_MODE") || envGet("VITE_CONTROL_PLANE_MODE") || viteMode || "mock";
  return String(mode).toLowerCase() === "live";
}
