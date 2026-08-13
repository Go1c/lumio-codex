/**
 * Control-plane client switchboard: mock (default) vs live HTTP.
 * Flows import from here so tests stay on mock; set CONTROL_PLANE_MODE=live for real stack.
 */

import * as mock from "./mock-control-api.js";
import * as live from "./live-control-api.js";
import { loadSessionId, storeSessionId } from "./session-storage.js";

export type ClientResult<T = unknown> =
  | { ok: true; data: T }
  | { ok: false; code: string; message: string; retryAfterSeconds?: number };

function useLive(): boolean {
  return live.isLiveMode();
}

export async function register(email: string, password: string): Promise<ClientResult<{ code: string }>> {
  if (useLive()) return live.register(email, password);
  return mock.register(email, password);
}

export async function verifyEmail(
  email: string,
  code: string,
): Promise<ClientResult<{ accessToken: string; refreshToken: string; sessionId: string }>> {
  if (useLive()) {
    const r = await live.verifyEmail(email, code);
    if (r.ok && r.data.sessionId) storeSessionId(r.data.sessionId);
    return r;
  }
  return mock.verifyEmail(email, code);
}

export async function resendVerification(
  email: string,
): Promise<ClientResult<{ retryAfterSeconds?: number }>> {
  if (useLive()) return live.resendVerification(email);
  return mock.resendVerification(email);
}

export async function login(
  email: string,
  password: string,
): Promise<ClientResult<{ accessToken: string; refreshToken: string; sessionId: string }>> {
  if (useLive()) {
    const r = await live.login(email, password);
    if (r.ok && r.data.sessionId) storeSessionId(r.data.sessionId);
    return r;
  }
  return mock.login(email, password);
}

export async function forgotPassword(email: string): Promise<ClientResult<{ code: string }>> {
  if (useLive()) return live.forgotPassword(email);
  return mock.forgotPassword(email);
}

export async function resetPassword(
  email: string,
  code: string,
  newPassword: string,
): Promise<ClientResult<Record<string, unknown>>> {
  if (useLive()) return live.resetPassword(email, code, newPassword);
  const r = await mock.resetPassword(email, code, newPassword);
  if (!r.ok) return r;
  return { ok: true, data: (r.data ?? {}) as Record<string, unknown> };
}

/** Mock signature: refresh(token). Live uses stored sessionId + token. */
export async function refresh(
  refreshToken: string,
): Promise<ClientResult<{ accessToken: string; refreshToken: string; sessionId?: string }>> {
  if (useLive()) {
    const sid = loadSessionId();
    if (!sid) return { ok: false, code: "session_expired", message: "session missing" };
    const r = await live.refresh(sid, refreshToken);
    if (r.ok && r.data.sessionId) storeSessionId(r.data.sessionId);
    return r;
  }
  return mock.refresh(refreshToken);
}

export async function logout(): Promise<ClientResult> {
  if (useLive()) {
    // best-effort; access token optional
    return { ok: true, data: {} };
  }
  return mock.logout();
}

// re-export mock test helpers (tests import mock directly for seeding)
export { mock, live };
