/**
 * 跨子域会话存储。
 *
 * 门户与产品站是三个独立的静态站点，没有共同的后端会话，所以令牌落在父域 Cookie
 * （`.lumiogame.com`）上让三站都能读到。父域 Cookie 必须能被前端读写，因此不是 HttpOnly；
 * 相应地服务端要收紧 CORS 与令牌有效期（见 web/README.md 的「对服务端的依赖」）。
 */

import { cookieDomainFor } from "@lumio/ui/config";

import { refreshTokens, type TokenPair } from "./client";

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

/** 仅读 refresh cookie：access cookie 到期被浏览器删除后，它是会话仍然可续期的唯一凭据。 */
export function readRefreshToken(): string | null {
  return readCookie(REFRESH_COOKIE);
}

/** access 或 refresh 任一存在即视为「可能有会话」，供初始态做乐观渲染。 */
export function hasSession(): boolean {
  return readCookie(ACCESS_COOKIE) !== null || readCookie(REFRESH_COOKIE) !== null;
}

/** Web Locks 的最小结构类型：只用到 request，避免绑定 DOM lib 版本。 */
interface LocksLike {
  request(name: string, callback: () => Promise<boolean>): Promise<boolean>;
}

/**
 * 轮换会话令牌。
 *
 * Sub2API 的 refresh 是轮转式（旧令牌立即失效、复用会触发整族撤销），多标签页同时
 * 刷新会互相作废。因此在 Web Locks（可用时）内执行，并在锁内重读 cookie：后来者
 * 会看到先到者刚写入的新令牌，直接复用而不是拿旧令牌去撞 REFRESH_TOKEN_REUSED。
 *
 * `force` 为 true 时即使 access cookie 仍在也照样轮换——用于「服务端已明确拒绝
 * 当前 access」的场景；为 false 时锁内发现已有 access（别的标签页刚转过）就直接复用。
 */
export async function rotateSession(force: boolean): Promise<boolean> {
  const run = async (): Promise<boolean> => {
    const refreshToken = readCookie(REFRESH_COOKIE);
    if (!refreshToken) return false;
    if (!force && readCookie(ACCESS_COOKIE)) return true;
    const refreshed = await refreshTokens(refreshToken);
    writeSession(refreshed);
    return true;
  };

  const locks = (navigator as Navigator & { locks?: LocksLike }).locks;
  if (locks?.request) return locks.request("lumio-auth-refresh", run);
  return run();
}

export function clearSession(): void {
  for (const name of [ACCESS_COOKIE, REFRESH_COOKIE, EXPIRES_COOKIE]) {
    document.cookie = serializeCookie(name, "", { maxAge: 0 });
  }
}
