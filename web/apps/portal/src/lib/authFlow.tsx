import { useCallback, useState } from "react";

import { LumioApiError, errorText, writeSession, type AuthOutcome } from "@lumio/auth";

import { useAuthRedirect } from "@/lib/redirect";
import { usePortalSession } from "@/state/session";

export interface TwoFactorChallenge {
  tempToken: string;
  maskedEmail: string;
}

/** 登录与注册共用的收尾逻辑：拿到令牌就写会话并回跳，遇到 2FA 挑战则转入第二步。 */
export function useAuthOutcome(next: string | null | undefined) {
  const session = usePortalSession();
  const redirect = useAuthRedirect(next);
  const [challenge, setChallenge] = useState<TwoFactorChallenge | null>(null);

  const apply = useCallback(
    (outcome: AuthOutcome) => {
      if (outcome.kind === "2fa") {
        setChallenge({ tempToken: outcome.tempToken, maskedEmail: outcome.maskedEmail });
        return;
      }
      writeSession(outcome.tokens);
      session.reload();
      redirect();
    },
    [redirect, session],
  );

  return { challenge, apply };
}

export function messageOf(error: unknown): string {
  return error instanceof LumioApiError ? error.message : errorText("SERVICE_UNAVAILABLE");
}
