import { useCallback } from "react";
import { useNavigate } from "react-router-dom";

import { resolveNext } from "@lumio/ui";

export type RedirectTarget = { kind: "internal"; path: string } | { kind: "external"; url: string };

/** `next` 是用户可控输入，先过白名单再决定用前端路由还是整页跳转。 */
export function redirectTarget(next: string | null | undefined, fallback: string): RedirectTarget {
  const resolved = resolveNext(next, fallback);
  return resolved.startsWith("/")
    ? { kind: "internal", path: resolved }
    : { kind: "external", url: resolved };
}

export function goExternal(url: string): void {
  window.location.assign(url);
}

export function useAuthRedirect(next: string | null | undefined) {
  const navigate = useNavigate();

  return useCallback(() => {
    const target = redirectTarget(next, "/account");
    if (target.kind === "internal") {
      navigate(target.path, { replace: true });
    } else {
      goExternal(target.url);
    }
  }, [navigate, next]);
}

/** 账号页之间互跳时保留 next，否则用户走一趟注册就丢了回跳目标。 */
export function withNext(path: string, next: string | null | undefined): string {
  return next ? `${path}?next=${encodeURIComponent(next)}` : path;
}
