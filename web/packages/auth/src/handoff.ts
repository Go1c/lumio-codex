/**
 * 跨注册域一次性会话交接。
 *
 * 令牌只放 URL 片段（不进服务端日志），落地后立刻 replaceState 抹掉。
 * 只对官方入口主机生效；refresh 轮换不允许两套 Cookie 长期并存，
 * 所以交接是单向的：离开遗留主机，落到规范账号 origin。
 */

import { isOfficialAccountHost } from "@lumio/ui/config";

import type { TokenPair } from "./client";
import { readSession, writeSession } from "./session";

const AT = "lumio_at";
const RT = "lumio_rt";
const EXP = "lumio_exp";
const MAX_EXPIRES_IN = 30 * 24 * 3600;

export interface HandoffLocation {
  hash: string;
  href: string;
  hostname: string;
  pathname: string;
  search: string;
}

export interface HandoffHistory {
  replaceState(state: unknown, title: string, url: string): void;
}

export function parseHandoffHash(hash: string): TokenPair | null {
  const raw = hash.startsWith("#") ? hash.slice(1) : hash;
  if (!raw) return null;
  const params = new URLSearchParams(raw);
  const accessToken = params.get(AT);
  const refreshToken = params.get(RT);
  const expiresIn = Number(params.get(EXP));
  if (!accessToken || !refreshToken) return null;
  if (!Number.isFinite(expiresIn) || expiresIn <= 0 || expiresIn > MAX_EXPIRES_IN) return null;
  if (!isTokenShape(accessToken) || !isTokenShape(refreshToken)) return null;
  return { accessToken, refreshToken, expiresIn };
}

export function buildHandoffHash(tokens: TokenPair): string {
  const params = new URLSearchParams({
    [AT]: tokens.accessToken,
    [RT]: tokens.refreshToken,
    [EXP]: String(tokens.expiresIn > 0 ? tokens.expiresIn : 3600),
  });
  return `#${params.toString()}`;
}

export function stripHandoffHash(hash: string): string {
  const raw = hash.startsWith("#") ? hash.slice(1) : hash;
  if (!raw) return "";
  const params = new URLSearchParams(raw);
  params.delete(AT);
  params.delete(RT);
  params.delete(EXP);
  return serializeHashParams(params);
}

function serializeHashParams(params: URLSearchParams): string {
  const parts: string[] = [];
  params.forEach((value, key) => {
    parts.push(value === "" ? key : `${encodeURIComponent(key)}=${encodeURIComponent(value)}`);
  });
  return parts.length > 0 ? `#${parts.join("&")}` : "";
}

export function isHandoffHash(hash: string): boolean {
  return parseHandoffHash(hash) !== null;
}

export function withHandoff(url: string, tokens: TokenPair): string {
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return url;
  }
  if (!isOfficialAccountHost(parsed.hostname)) return url;
  const existing = parsed.hash;
  const incoming = new URLSearchParams(buildHandoffHash(tokens).slice(1));
  const merged = new URLSearchParams(existing.startsWith("#") ? existing.slice(1) : existing);
  incoming.forEach((value, key) => merged.set(key, value));
  parsed.hash = serializeHashParams(merged).slice(1);
  return parsed.toString();
}

export function consumeHandoff(location: HandoffLocation, history: HandoffHistory): boolean {
  if (!isOfficialAccountHost(location.hostname)) return false;
  const tokens = parseHandoffHash(location.hash);
  if (!tokens) return false;
  writeSession(tokens);
  const nextHash = stripHandoffHash(location.hash);
  history.replaceState(null, "", `${originOf(location)}${location.pathname}${location.search}${nextHash}`);
  return true;
}

/** 应用启动时调用一次：有交接片段就落 Cookie 并擦掉地址栏。 */
export function consumeHandoffFromWindow(): boolean {
  if (typeof window === "undefined") return false;
  return consumeHandoff(window.location, window.history);
}

/** 读出现有会话并换成交接用的 expiresIn（秒）。没有 refresh 就不交。 */
export function sessionTokensForHandoff(): TokenPair | null {
  const stored = readSession();
  if (!stored?.accessToken || !stored.refreshToken) return null;
  const remaining = Math.floor((stored.expiresAt - Date.now()) / 1000);
  return {
    accessToken: stored.accessToken,
    refreshToken: stored.refreshToken,
    expiresIn: remaining > 0 ? remaining : 3600,
  };
}

function isTokenShape(value: string): boolean {
  return value.length > 0 && value.length < 4096 && !/[\s#]/.test(value);
}

function originOf(location: HandoffLocation): string {
  try {
    return new URL(location.href).origin;
  } catch {
    return "";
  }
}
