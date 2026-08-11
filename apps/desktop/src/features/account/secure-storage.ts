/**
 * Desktop account credential storage.
 * Refresh tokens go to OS secure storage (Keychain/credential manager), never LocalStorage/URL/logs.
 */

export type SecureStorageDriver = {
  set(key: string, value: string): Promise<void>;
  get(key: string): Promise<string | null>;
  delete(key: string): Promise<void>;
};

const REFRESH_KEY = "fns.account.refresh";
const ACCESS_MEMORY_KEY = "fns.account.access.memory";

/** In-process fallback used in unit tests when Tauri keychain is unavailable. */
export class MemorySecureStorage implements SecureStorageDriver {
  private map = new Map<string, string>();
  async set(key: string, value: string): Promise<void> {
    this.map.set(key, value);
  }
  async get(key: string): Promise<string | null> {
    return this.map.get(key) ?? null;
  }
  async delete(key: string): Promise<void> {
    this.map.delete(key);
  }
}

let driver: SecureStorageDriver = new MemorySecureStorage();
const accessMemory = new Map<string, string>();

export function setSecureStorageDriver(d: SecureStorageDriver): void {
  driver = d;
}

export function getSecureStorageDriver(): SecureStorageDriver {
  return driver;
}

export async function storeRefreshToken(token: string): Promise<void> {
  if (!token) throw new Error("empty refresh");
  // Boundary: secure storage only — never browser web storage / URL.
  await driver.set(REFRESH_KEY, token);
}

export async function loadRefreshToken(): Promise<string | null> {
  return driver.get(REFRESH_KEY);
}

export async function clearRefreshToken(): Promise<void> {
  await driver.delete(REFRESH_KEY);
}

export function storeAccessInMemory(token: string): void {
  accessMemory.set(ACCESS_MEMORY_KEY, token);
}

export function loadAccessFromMemory(): string | null {
  return accessMemory.get(ACCESS_MEMORY_KEY) ?? null;
}

export function clearAccessMemory(): void {
  accessMemory.delete(ACCESS_MEMORY_KEY);
}

export async function clearDesktopSession(): Promise<void> {
  clearAccessMemory();
  await clearRefreshToken();
}

/** Device binding / browser callback payload (desktop OAuth-style). */
export type DeviceBinding = {
  deviceId: string;
  callbackUrl: string;
  state: string;
};

export function buildDeviceBinding(deviceId: string, redirectBase: string): DeviceBinding {
  const state = `st_${deviceId}_${Date.now()}`;
  const callbackUrl = `${redirectBase.replace(/\/$/, "")}/account/callback?state=${encodeURIComponent(state)}`;
  return { deviceId, callbackUrl, state };
}

export function parseBrowserCallback(url: string): { code?: string; state?: string; error?: string } {
  try {
    const u = new URL(url);
    return {
      code: u.searchParams.get("code") ?? undefined,
      state: u.searchParams.get("state") ?? undefined,
      error: u.searchParams.get("error") ?? undefined,
    };
  } catch {
    return { error: "invalid_callback" };
  }
}

/**
 * Apply tokens from a browser callback / login response.
 * Refresh → secure storage; access → memory only.
 */
export async function applySessionTokens(tokens: {
  accessToken: string;
  refreshToken: string;
}): Promise<void> {
  storeAccessInMemory(tokens.accessToken);
  await storeRefreshToken(tokens.refreshToken);
}

/** Never serialize refresh into error reports. */
export function sanitizeAccountLog(fields: Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(fields)) {
    if (/refresh|token|password|code|secret/i.test(k)) {
      out[k] = "[REDACTED]";
    } else if (typeof v === "string" && /^(rt_|at_|Bearer )/i.test(v)) {
      out[k] = "[REDACTED]";
    } else if (typeof v === "string") {
      out[k] = v
        .replace(/rt_[A-Za-z0-9_-]+/gi, "[REDACTED]")
        .replace(/at_[A-Za-z0-9_-]+/gi, "[REDACTED]");
    } else {
      out[k] = v;
    }
  }
  return out;
}

/** Policy: refresh placements allowed on desktop. */
export function desktopRefreshPlacement(): "secure" {
  return "secure";
}
