import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from "react";

import { getSession, logout as logoutRequest } from "@/api/endpoints";
import type { Entitlement, SessionSnapshot, UserView } from "@/api/types";
import { isApiError, onSessionExpired } from "@/lib/api";

type SessionStatus = "loading" | "authenticated" | "anonymous";

interface SessionContextValue {
  status: SessionStatus;
  user: UserView | undefined;
  entitlement: Entitlement | undefined;
  /** 登录 / 验证邮箱成功后同步会话，避免多跑一次 /auth/session。 */
  applySnapshot: (snapshot: SessionSnapshot) => void;
  patchUser: (user: UserView) => void;
  reload: () => Promise<void>;
  signOut: () => Promise<void>;
}

const SessionContext = createContext<SessionContextValue | null>(null);

export function SessionProvider({ children }: { children: ReactNode }) {
  const [status, setStatus] = useState<SessionStatus>("loading");
  const [user, setUser] = useState<UserView | undefined>();
  const [entitlement, setEntitlement] = useState<Entitlement | undefined>();

  const applySnapshot = useCallback((snapshot: SessionSnapshot) => {
    setUser(snapshot.user);
    setEntitlement(snapshot.entitlement);
    setStatus("authenticated");
  }, []);

  const clear = useCallback(() => {
    setUser(undefined);
    setEntitlement(undefined);
    setStatus("anonymous");
  }, []);

  const reload = useCallback(async () => {
    try {
      applySnapshot(await getSession());
    } catch (error) {
      // 未登录（401）是公开页的正常状态，不当作错误；其余异常也降级为匿名，
      // 页面自身的错误态会覆盖真正需要提示的场景。
      if (!isApiError(error)) {
        clear();
        return;
      }
      clear();
    }
  }, [applySnapshot, clear]);

  useEffect(() => {
    void reload();
  }, [reload]);

  useEffect(() => {
    onSessionExpired(() => clear());
    return () => onSessionExpired(null);
  }, [clear]);

  const signOut = useCallback(async () => {
    try {
      await logoutRequest();
    } finally {
      clear();
    }
  }, [clear]);

  const patchUser = useCallback((next: UserView) => setUser(next), []);

  const value = useMemo<SessionContextValue>(
    () => ({ status, user, entitlement, applySnapshot, patchUser, reload, signOut }),
    [status, user, entitlement, applySnapshot, patchUser, reload, signOut],
  );

  return <SessionContext.Provider value={value}>{children}</SessionContext.Provider>;
}

export function useSession(): SessionContextValue {
  const value = useContext(SessionContext);
  if (!value) throw new Error("useSession 必须在 SessionProvider 内使用");
  return value;
}
