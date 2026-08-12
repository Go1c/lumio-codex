import { useCallback, useEffect, useState } from "react";

import { LumioApiError, errorText, fetchPublicSettings, type PublicSettings } from "@lumio/auth";

interface State {
  status: "loading" | "ready" | "error";
  data?: PublicSettings;
  error?: string;
}

/** 注册页的开关全部来自 `settings/public`，页面不写死任何注册策略。 */
export function usePublicSettings() {
  const [state, setState] = useState<State>({ status: "loading" });
  const [nonce, setNonce] = useState(0);

  const reload = useCallback(() => setNonce((value) => value + 1), []);

  useEffect(() => {
    let cancelled = false;
    setState({ status: "loading" });

    fetchPublicSettings()
      .then((data) => {
        if (!cancelled) setState({ status: "ready", data });
      })
      .catch((error: unknown) => {
        if (cancelled) return;
        const message =
          error instanceof LumioApiError ? error.message : errorText("SERVICE_UNAVAILABLE");
        setState({ status: "error", error: message });
      });

    return () => {
      cancelled = true;
    };
  }, [nonce]);

  return { ...state, reload };
}
