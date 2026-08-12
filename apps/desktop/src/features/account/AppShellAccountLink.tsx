/**
 * App Shell account entry (R-00007 wiring).
 * Lightweight link/status surface; full pages live in account-web.
 */
import { useEffect, useState } from "react";
import { loadAccessFromMemory, loadRefreshToken } from "./secure-storage";

export default function AppShellAccountLink() {
  const [signedIn, setSignedIn] = useState(false);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      const access = loadAccessFromMemory();
      const refresh = await loadRefreshToken();
      if (!cancelled) setSignedIn(Boolean(access || refresh));
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <div className="app-shell-account" data-testid="app-shell-account">
      <a href="account-web://login" aria-label="Account">
        {signedIn ? "Account" : "Sign in"}
      </a>
    </div>
  );
}
