import { useCallback, useEffect, useState } from "react";

import {
  LumioApiError,
  fetchProfile,
  logout as logoutRequest,
  type AccountProfile,
} from "./client";
import {
  clearSession,
  hasSession,
  readRefreshToken,
  readSession,
  rotateSession,
} from "./session";

export interface SessionState {
  status: "loading" | "anonymous" | "authenticated";
  profile?: AccountProfile;
  accessToken?: string;
  reload: () => void;
  signOut: () => Promise<void>;
}

/**
 * 会话状态。Cookie 里有令牌就先按已登录渲染（避免账号入口先闪「登录」再跳「账户」），
 * 随后用 `/auth/me` 校验；校验不过就刷新一次，仍不过则清掉本地会话退回未登录。
 *
 * access cookie 到期被浏览器删除但 refresh cookie 还在时（1~2 小时后的主场景），
 * 先用 refresh 轮换出新令牌再拉资料——否则 30 天 refresh 形同虚设（QA W-1）。
 */
export function useSession(): SessionState {
  const [status, setStatus] = useState<SessionState["status"]>(() =>
    hasSession() ? "authenticated" : "anonymous",
  );
  const [profile, setProfile] = useState<AccountProfile | undefined>(undefined);
  const [accessToken, setAccessToken] = useState<string | undefined>(
    () => readSession()?.accessToken,
  );
  const [nonce, setNonce] = useState(0);

  const reload = useCallback(() => setNonce((n) => n + 1), []);

  useEffect(() => {
    let cancelled = false;

    async function resolveProfile(token: string): Promise<void> {
      try {
        const result = await fetchProfile(token);
        if (cancelled) return;
        setProfile(result);
        setAccessToken(token);
        setStatus("authenticated");
      } catch (error) {
        if (cancelled) return;
        const expired =
          error instanceof LumioApiError && error.code === "AUTH_SESSION_EXPIRED";
        if (!expired) {
          // 网络抖动不该把用户踢下线：保留本地会话，只是这轮拿不到资料。
          setStatus("authenticated");
          return;
        }
        throw error;
      }
    }

    /** 轮换令牌后用新的 access 重拉资料；任何一步失败都交给调用方的 catch。 */
    async function rotateThenResolve(): Promise<void> {
      const rotated = await rotateSession(true);
      if (cancelled || !rotated) throw new Error("refresh unavailable");
      const refreshed = readSession();
      if (!refreshed) throw new Error("session gone");
      await resolveProfile(refreshed.accessToken);
    }

    function forget(): void {
      clearSession();
      setProfile(undefined);
      setAccessToken(undefined);
      setStatus("anonymous");
    }

    void (async () => {
      const stored = readSession();
      if (!stored && !readRefreshToken()) {
        setStatus("anonymous");
        setProfile(undefined);
        setAccessToken(undefined);
        return;
      }

      try {
        if (stored) {
          try {
            await resolveProfile(stored.accessToken);
          } catch {
            // resolveProfile 只在服务端明确拒绝 access 时抛出：先轮换一次再重拉。
            await rotateThenResolve();
          }
        } else {
          await rotateThenResolve();
        }
      } catch {
        if (cancelled) return;
        forget();
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [nonce]);

  const signOut = useCallback(async () => {
    // access cookie 可能已到期被删掉，refresh cookie 仍在时照样要通知服务端撤销。
    const refreshToken = readSession()?.refreshToken ?? readRefreshToken();
    if (refreshToken) {
      // 服务端撤销失败也要让本地退出，否则用户会卡在「点了登出还显示已登录」。
      await logoutRequest(refreshToken).catch(() => undefined);
    }
    clearSession();
    setProfile(undefined);
    setAccessToken(undefined);
    setStatus("anonymous");
  }, []);

  return { status, profile, accessToken, reload, signOut };
}
