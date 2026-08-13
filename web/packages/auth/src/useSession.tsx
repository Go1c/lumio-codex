import { useCallback, useEffect, useState } from "react";

import {
  LumioApiError,
  fetchProfile,
  logout as logoutRequest,
  refreshTokens,
  type AccountProfile,
} from "./client";
import { clearSession, readSession, writeSession } from "./session";

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
 */
export function useSession(): SessionState {
  const [status, setStatus] = useState<SessionState["status"]>(() =>
    readSession() ? "authenticated" : "anonymous",
  );
  const [profile, setProfile] = useState<AccountProfile | undefined>(undefined);
  const [accessToken, setAccessToken] = useState<string | undefined>(
    () => readSession()?.accessToken,
  );
  const [nonce, setNonce] = useState(0);

  const reload = useCallback(() => setNonce((n) => n + 1), []);

  useEffect(() => {
    const stored = readSession();
    if (!stored) {
      setStatus("anonymous");
      setProfile(undefined);
      setAccessToken(undefined);
      return;
    }

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

    void (async () => {
      try {
        await resolveProfile(stored.accessToken);
      } catch {
        try {
          if (!stored.refreshToken) throw new Error("no refresh token");
          const refreshed = await refreshTokens(stored.refreshToken);
          if (cancelled) return;
          writeSession(refreshed);
          await resolveProfile(refreshed.accessToken);
        } catch {
          if (cancelled) return;
          clearSession();
          setProfile(undefined);
          setAccessToken(undefined);
          setStatus("anonymous");
        }
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [nonce]);

  const signOut = useCallback(async () => {
    const stored = readSession();
    if (stored?.refreshToken) {
      // 服务端撤销失败也要让本地退出，否则用户会卡在「点了登出还显示已登录」。
      await logoutRequest(stored.refreshToken).catch(() => undefined);
    }
    clearSession();
    setProfile(undefined);
    setAccessToken(undefined);
    setStatus("anonymous");
  }, []);

  return { status, profile, accessToken, reload, signOut };
}
