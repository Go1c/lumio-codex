import { createContext, useCallback, useContext, useEffect, useMemo, useState } from "react";
import type { ReactNode } from "react";
import { ApiError } from "../api/client";
import * as api from "../api/endpoints";
import type { AdminMe } from "../api/types";

/**
 * 后台的会话状态机：
 *
 *   loading ─► anonymous ──login──► mfa_challenge ──totp──► ready
 *                  │                                        ▲
 *                  └──login（尚未启用 2FA）──► enroll ───────┘
 *
 * 「半会话」（登录成功但未过两步验证）访问任何业务接口都会拿到 401 mfa_required，
 * 此时 handleApiError 会把整个应用退回 mfa_challenge，业务页面不会渲染出来。
 */
export type AuthStatus = "loading" | "anonymous" | "mfa_challenge" | "enroll" | "ready" | "forbidden";

interface AuthContextValue {
  status: AuthStatus;
  me: AdminMe | null;
  login: (email: string, password: string) => Promise<void>;
  verifyTotp: (code: string) => Promise<void>;
  completeEnrollment: (code: string) => Promise<void>;
  logout: () => Promise<void>;
  backToLogin: () => void;
  /** 业务页面在 catch 里调用；返回 true 表示错误已被会话层接管，页面无需再展示错误条。 */
  handleApiError: (error: unknown) => boolean;
}

const AuthContext = createContext<AuthContextValue | null>(null);

export function AuthProvider({ children }: { children: ReactNode }) {
  const [status, setStatus] = useState<AuthStatus>("loading");
  const [me, setMe] = useState<AdminMe | null>(null);

  const applyMe = useCallback((admin: AdminMe) => {
    setMe(admin);
    // 首次登录尚未启用两步验证时强制引导，不给跳过入口。
    setStatus(admin.totp_enabled ? "ready" : "enroll");
  }, []);

  const refresh = useCallback(async () => {
    try {
      applyMe(await api.fetchMe());
    } catch (error) {
      if (error instanceof ApiError && error.isMfaRequired) {
        setStatus("mfa_challenge");
        return;
      }
      if (error instanceof ApiError && error.isForbidden) {
        setStatus("forbidden");
        return;
      }
      setMe(null);
      setStatus("anonymous");
    }
  }, [applyMe]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const login = useCallback(
    async (email: string, password: string) => {
      const result = await api.login(email, password);
      if (result.mfa_required) {
        setStatus("mfa_challenge");
        return;
      }
      await refresh();
    },
    [refresh],
  );

  const verifyTotp = useCallback(
    async (code: string) => {
      await api.verifyLoginTotp(code);
      await refresh();
    },
    [refresh],
  );

  const completeEnrollment = useCallback(
    async (code: string) => {
      await api.enableTotp(code);
      await refresh();
    },
    [refresh],
  );

  const logout = useCallback(async () => {
    try {
      await api.logout();
    } finally {
      setMe(null);
      setStatus("anonymous");
    }
  }, []);

  const backToLogin = useCallback(() => {
    setMe(null);
    setStatus("anonymous");
  }, []);

  const handleApiError = useCallback((error: unknown): boolean => {
    if (!(error instanceof ApiError)) return false;
    if (error.isMfaRequired) {
      setStatus("mfa_challenge");
      return true;
    }
    if (error.isForbidden) {
      setStatus("forbidden");
      return true;
    }
    if (error.isUnauthenticated) {
      setMe(null);
      setStatus("anonymous");
      return true;
    }
    return false;
  }, []);

  const value = useMemo<AuthContextValue>(
    () => ({ status, me, login, verifyTotp, completeEnrollment, logout, backToLogin, handleApiError }),
    [status, me, login, verifyTotp, completeEnrollment, logout, backToLogin, handleApiError],
  );

  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}

export function useAuth(): AuthContextValue {
  const ctx = useContext(AuthContext);
  if (!ctx) throw new Error("useAuth 必须在 AuthProvider 内使用");
  return ctx;
}
