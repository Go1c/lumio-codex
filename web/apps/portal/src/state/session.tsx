import { createContext, useContext, type ReactNode } from "react";

import { useSession, type SessionState } from "@lumio/auth";

const SessionContext = createContext<SessionState | null>(null);

/** 整个门户共用一份会话，避免每个页面各拉一次 `/auth/me`、登出后状态不同步。 */
export function SessionProvider({ children }: { children: ReactNode }) {
  const session = useSession();
  return <SessionContext.Provider value={session}>{children}</SessionContext.Provider>;
}

export function usePortalSession(): SessionState {
  const session = useContext(SessionContext);
  if (!session) throw new Error("usePortalSession 必须在 SessionProvider 内使用");
  return session;
}
