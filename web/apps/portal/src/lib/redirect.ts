import { useCallback } from "react";
import { useNavigate } from "react-router-dom";

import { sessionTokensForHandoff, withHandoff, type TokenPair } from "@lumio/auth";
import {
  bounceToCanonicalUrl,
  canonicalAccountOrigin,
  resolveNext,
  shouldBounceToCanonical,
} from "@lumio/ui";

export type RedirectTarget = { kind: "internal"; path: string } | { kind: "external"; url: string };

export interface RedirectContext {
  currentOrigin?: string;
  tokens?: TokenPair | null;
}

/** `next` 是用户可控输入，先过白名单再决定用前端路由还是整页跳转。 */
export function redirectTarget(
  next: string | null | undefined,
  fallback: string,
  context: RedirectContext = {},
): RedirectTarget {
  const resolved = resolveNext(next, fallback);
  const currentOrigin = context.currentOrigin;

  let url: string;
  if (resolved.startsWith("/")) {
    if (currentOrigin && shouldBounceToCanonical(hostOf(currentOrigin))) {
      url = `${canonicalAccountOrigin()}${resolved}`;
    } else {
      return { kind: "internal", path: resolved };
    }
  } else {
    url = rewriteLegacyOrigin(resolved);
  }

  if (context.tokens && isCrossOrigin(currentOrigin, url)) {
    url = withHandoff(url, context.tokens);
  }

  if (currentOrigin && sameOrigin(currentOrigin, url) && !url.includes("lumio_at=")) {
    const parsed = new URL(url, currentOrigin);
    return { kind: "internal", path: `${parsed.pathname}${parsed.search}` };
  }

  return { kind: "external", url };
}

export function goExternal(url: string): void {
  window.location.assign(url);
}

export function useAuthRedirect(next: string | null | undefined) {
  const navigate = useNavigate();

  return useCallback(() => {
    const target = redirectTarget(next, "/account", {
      currentOrigin: window.location.origin,
      tokens: sessionTokensForHandoff(),
    });
    if (target.kind === "internal") {
      navigate(target.path, { replace: true });
    } else {
      goExternal(target.url);
    }
  }, [navigate, next]);
}

function hostOf(origin: string): string {
  try {
    return new URL(origin).hostname;
  } catch {
    return "";
  }
}

function sameOrigin(origin: string, url: string): boolean {
  try {
    return new URL(url, origin).origin === new URL(origin).origin;
  } catch {
    return false;
  }
}

function isCrossOrigin(origin: string | undefined, url: string): boolean {
  if (!origin) return false;
  return !sameOrigin(origin, url);
}

/** 登录成功后不要把用户送回遗留门户，改写到规范账号主机。 */
function rewriteLegacyOrigin(url: string): string {
  return bounceToCanonicalUrl(url) ?? url;
}

/** 账号页之间互跳时保留 next，否则用户走一趟注册就丢了回跳目标。 */
export function withNext(path: string, next: string | null | undefined): string {
  return next ? `${path}?next=${encodeURIComponent(next)}` : path;
}

/**
 * CC 桌面端在控制面注册的回调形态（`cchaven/services/cchaven-control/migrations/0002_seed.sql`）：
 * 本机回环两种主机名，外加一个自定义 scheme 兜底。
 */
const LOOPBACK_HOSTS = ["127.0.0.1", "localhost"];
const CUSTOM_SCHEME_CALLBACK = "cchaven://auth/callback";

/**
 * 授权页的回跳地址校验。`redirect_uri` 来自查询串、`redirect_to` 来自控制面响应，
 * 两者都会被直接喂给浏览器导航，所以在跳之前必须逐条比对注册形态：
 * 放宽到「任意 http 地址」就等于把授权码送给任何能构造链接的人。
 */
export function isAllowedDesktopRedirect(uri: string | null | undefined): boolean {
  if (!uri) return false;

  let url: URL;
  try {
    url = new URL(uri);
  } catch {
    return false;
  }

  if (url.protocol === "http:") {
    return LOOPBACK_HOSTS.includes(url.hostname) && url.pathname === "/callback";
  }
  return `${url.protocol}//${url.host}${url.pathname}` === CUSTOM_SCHEME_CALLBACK;
}

/** 用户点「拒绝」时按 OAuth 契约回跳；地址不可信则不跳（桌面端会走等待超时）。 */
export function denyRedirectUrl(
  redirectUri: string | null | undefined,
  state: string,
): string | null {
  if (!isAllowedDesktopRedirect(redirectUri)) return null;

  const url = new URL(redirectUri as string);
  url.searchParams.set("error", "access_denied");
  url.searchParams.set("error_description", "你在浏览器中拒绝了本次授权。");
  if (state) url.searchParams.set("state", state);
  return url.toString();
}
