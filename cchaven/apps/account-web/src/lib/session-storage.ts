/**
 * Access/session storage rules for account-web.
 * Refresh tokens must NEVER enter LocalStorage, URL, logs, or error reports.
 * Web: refresh is process-memory only. Desktop: OS secure storage boundary.
 */

export type SecureSession = {
  accessToken: string;
  /** Refresh is held only in memory (web) or OS secure storage (desktop). */
  refreshToken?: string;
  sessionId?: string;
};

const ACCESS_KEY = "fns.account.access";
const SESSION_KEY = "fns.account.sessionId";
// intentionally no REFRESH_KEY in localStorage

let memoryRefresh: string | undefined;
let memorySessionId: string | undefined;

export type TokenPlacement = "localStorage" | "url" | "memory" | "secure" | "error_log";

/** Structural guard: refuse to write refresh into localStorage, URL, or error logs. */
export function assertSafeTokenPlacement(kind: "access" | "refresh", location: TokenPlacement): void {
  if (kind === "refresh" && (location === "localStorage" || location === "url" || location === "error_log")) {
    throw new Error("refresh token must not be stored in LocalStorage, URL, or error logs");
  }
}

export function storeAccessToken(token: string): void {
  assertSafeTokenPlacement("access", "localStorage");
  if (typeof localStorage !== "undefined") {
    localStorage.setItem(ACCESS_KEY, token);
  }
}

export function loadAccessToken(): string | null {
  if (typeof localStorage === "undefined") return null;
  return localStorage.getItem(ACCESS_KEY);
}

export function clearAccessToken(): void {
  if (typeof localStorage !== "undefined") {
    localStorage.removeItem(ACCESS_KEY);
  }
}

/** Web: keep refresh in process memory only — never LocalStorage/URL. */
export function storeRefreshInMemory(token: string): void {
  assertSafeTokenPlacement("refresh", "memory");
  memoryRefresh = token;
}

export function loadRefreshFromMemory(): string | undefined {
  return memoryRefresh;
}

export function clearRefreshMemory(): void {
  memoryRefresh = undefined;
}

/** Session id is non-secret correlation id; memory preferred, localStorage ok for web refresh calls. */
export function storeSessionId(id: string): void {
  memorySessionId = id;
  if (typeof localStorage !== "undefined") {
    localStorage.setItem(SESSION_KEY, id);
  }
}

export function loadSessionId(): string | undefined {
  if (memorySessionId) return memorySessionId;
  if (typeof localStorage === "undefined") return undefined;
  return localStorage.getItem(SESSION_KEY) ?? undefined;
}

export function clearSessionId(): void {
  memorySessionId = undefined;
  if (typeof localStorage !== "undefined") {
    localStorage.removeItem(SESSION_KEY);
  }
}

export function clearAllSession(): void {
  clearAccessToken();
  clearRefreshMemory();
  clearSessionId();
}

/**
 * Policy helper used by tests and desktop adapters:
 * allowed placements for refresh tokens.
 */
export function allowedRefreshPlacements(): ReadonlyArray<TokenPlacement> {
  return ["memory", "secure"];
}

export function isRefreshPlacementAllowed(location: TokenPlacement): boolean {
  return location === "memory" || location === "secure";
}

/** Redact token-like material before any error report / log. */
export function sanitizeErrorForReport(err: unknown): { message: string } {
  const msg = err instanceof Error ? err.message : String(err);
  const cleaned = msg
    .replace(/rt_[A-Za-z0-9_-]+/gi, "[REDACTED]")
    .replace(/at_[A-Za-z0-9_-]+/gi, "[REDACTED]")
    .replace(/Bearer\s+\S+/gi, "Bearer [REDACTED]")
    .replace(/refresh[_-]?token["']?\s*[:=]\s*["']?[^"'\\s]+/gi, "refresh_token=[REDACTED]");
  return { message: cleaned };
}

/** Explicitly refuse serializing refresh into any report payload. */
export function buildErrorReport(fields: Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(fields)) {
    if (/refresh/i.test(k)) {
      out[k] = "[REDACTED]";
      continue;
    }
    if (typeof v === "string") {
      out[k] = sanitizeErrorForReport(v).message;
    } else {
      out[k] = v;
    }
  }
  return out;
}
