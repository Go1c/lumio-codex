/**
 * 跨子域会话存储。
 *
 * 门户与产品站是三个独立的静态站点，没有共同的后端会话，所以令牌落在父域 Cookie
 * （`.lumiogame.com`）上让三站都能读到。父域 Cookie 必须能被前端读写，因此不是 HttpOnly；
 * 相应地服务端要收紧 CORS 与令牌有效期（见 web/README.md 的「对服务端的依赖」）。
 */

import { cookieDomainFor } from "@lumio/ui/config";

import type { TokenPair } from "./client";

const ACCESS_COOKIE = "lumio_at";
const REFRESH_COOKIE = "lumio_rt";
const EXPIRES_COOKIE = "lumio_at_exp";
const REFRESH_MAX_AGE = 30 * 24 * 3600;

export interface StoredSession {
  accessToken: string;
  refreshToken: string;
  /** access token 的到期时间戳（毫秒）。 */
  expiresAt: number;
}

export interface CookieOptions {
  maxAge: number;
  hostname?: string;
  secure?: boolean;
}

export function serializeCookie(name: string, value: string, options: CookieOptions): string {
  const hostname = options.hostname ?? location.hostname;
  const secure = options.secure ?? location.protocol === "https:";
  const domain = cookieDomainFor(hostname);

  const parts = [
    `${name}=${encodeURIComponent(value)}`,
    "Path=/",
    `Max-Age=${options.maxAge}`,
    "SameSite=Lax",
  ];
  if (domain) parts.push(`Domain=${domain}`);
  if (secure) parts.push("Secure");
  return parts.join("; ");
}

function readCookie(name: string): string | null {
  const match = document.cookie
    .split(";")
    .map((part) => part.trim())
    .find((part) => part.startsWith(`${name}=`));
  if (!match) return null;
  const value = decodeURIComponent(match.slice(name.length + 1));
  return value || null;
}

export function writeSession(tokens: TokenPair): void {
  const accessMaxAge = tokens.expiresIn > 0 ? tokens.expiresIn : 3600;
  const expiresAt = Date.now() + accessMaxAge * 1000;

  document.cookie = serializeCookie(ACCESS_COOKIE, tokens.accessToken, { maxAge: accessMaxAge });
  document.cookie = serializeCookie(EXPIRES_COOKIE, String(expiresAt), { maxAge: REFRESH_MAX_AGE });
  document.cookie = serializeCookie(REFRESH_COOKIE, tokens.refreshToken, {
    maxAge: REFRESH_MAX_AGE,
  });
}

export function readSession(): StoredSession | null {
  const accessToken = readCookie(ACCESS_COOKIE);
  if (!accessToken) return null;
  return {
    accessToken,
    refreshToken: readCookie(REFRESH_COOKIE) ?? "",
    expiresAt: Number(readCookie(EXPIRES_COOKIE) ?? 0),
  };
}

export function clearSession(): void {
  for (const name of [ACCESS_COOKIE, REFRESH_COOKIE, EXPIRES_COOKIE]) {
    document.cookie = serializeCookie(name, "", { maxAge: 0 });
  }
}
