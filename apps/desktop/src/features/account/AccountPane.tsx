/**
 * Desktop account feature surface.
 * Hosts browser-callback / device-binding + secure refresh storage boundary.
 * Full form flows live in account-web; desktop reuses the same screen models via props.
 */

import { useMemo, useState } from "react";
import {
  applySessionTokens,
  buildDeviceBinding,
  clearDesktopSession,
  desktopRefreshPlacement,
  parseBrowserCallback,
  sanitizeAccountLog,
  type DeviceBinding,
} from "./secure-storage";

export type AccountPaneProps = {
  deviceId: string;
  redirectBase?: string;
  /** Invoked when user opens system browser for sign-in. */
  onOpenBrowser?: (url: string) => void;
  /** Optional: inject tokens after external login completes. */
  onSessionReady?: (tokens: { accessToken: string; refreshToken: string }) => void;
};

export type AccountPaneStatus =
  | "idle"
  | "waiting_browser"
  | "success"
  | "error"
  | "offline";

/**
 * Pure helper exported for unit tests — maps callback URL → next status + tokens.
 */
export function resolveBrowserCallback(url: string): {
  status: AccountPaneStatus;
  message: string;
  code?: string;
  state?: string;
} {
  const parsed = parseBrowserCallback(url);
  if (parsed.error) {
    return {
      status: "error",
      message: parsed.error === "invalid_callback" ? "Invalid callback URL." : parsed.error,
    };
  }
  if (!parsed.code || !parsed.state) {
    return { status: "error", message: "Missing code or state in callback." };
  }
  return {
    status: "success",
    message: "Sign-in completed. Tokens stored securely.",
    code: parsed.code,
    state: parsed.state,
  };
}

export function AccountPane({
  deviceId,
  redirectBase = "fns://",
  onOpenBrowser,
  onSessionReady,
}: AccountPaneProps) {
  const [status, setStatus] = useState<AccountPaneStatus>("idle");
  const [message, setMessage] = useState<string | null>(null);
  const [binding, setBinding] = useState<DeviceBinding | null>(null);

  const placement = useMemo(() => desktopRefreshPlacement(), []);

  function startBrowserSignIn() {
    const b = buildDeviceBinding(deviceId, redirectBase);
    setBinding(b);
    setStatus("waiting_browser");
    setMessage("Complete sign-in in your browser, then return here.");
    onOpenBrowser?.(b.callbackUrl);
  }

  async function handleCallbackUrl(url: string) {
    const resolved = resolveBrowserCallback(url);
    if (resolved.status !== "success" || !resolved.code) {
      setStatus("error");
      setMessage(resolved.message);
      return;
    }
    if (binding && resolved.state !== binding.state) {
      setStatus("error");
      setMessage("Callback state mismatch. Start sign-in again.");
      return;
    }
    // Mock exchange: production would POST code to control-plane.
    const tokens = {
      accessToken: `at_desktop_${resolved.code}`,
      refreshToken: `rt_desktop_${resolved.code}`,
    };
    await applySessionTokens(tokens);
    setStatus("success");
    setMessage(resolved.message);
    onSessionReady?.(tokens);
  }

  async function signOut() {
    await clearDesktopSession();
    setStatus("idle");
    setMessage(null);
    setBinding(null);
  }

  return (
    <section className="account-pane" data-refresh-placement={placement} data-status={status}>
      <h2>Account</h2>
      <p className="account-pane__hint">
        Desktop refresh tokens use OS secure storage ({placement}), never LocalStorage or URL.
      </p>

      {message ? (
        <p role="alert" className={`account-pane__msg account-pane__msg--${status}`}>
          {message}
        </p>
      ) : null}

      <div className="account-pane__actions">
        <button type="button" onClick={startBrowserSignIn} disabled={status === "waiting_browser"}>
          Sign in with browser
        </button>
        <button type="button" onClick={() => void signOut()}>
          Sign out
        </button>
      </div>

      {/* Hidden test/dev hook for deep-link injection */}
      <label className="account-pane__callback">
        Callback URL
        <input
          type="url"
          name="callbackUrl"
          autoComplete="off"
          placeholder="fns://account/callback?code=…&state=…"
          onBlur={(e) => {
            const v = e.target.value.trim();
            if (v) void handleCallbackUrl(v);
          }}
        />
      </label>
    </section>
  );
}

/** Safe log wrapper for desktop account diagnostics. */
export function accountDiag(fields: Record<string, unknown>): Record<string, unknown> {
  return sanitizeAccountLog(fields);
}

export default AccountPane;
