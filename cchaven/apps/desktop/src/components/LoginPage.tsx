import { useEffect, useRef, useState } from "react";
import { t } from "../i18n";
import { EVENTS, toApiError } from "../lib/api";
import { formatDate } from "../lib/format";
import { useApi } from "../state/ApiProvider";
import { useToast } from "../state/ToastProvider";
import { Banner, Spinner } from "./ui";
import type { LoginFailure, SessionView } from "../lib/types";

type Phase = "idle" | "waiting" | "failed";

/**
 * 5.1 APP 登录页 — three states, no email or password field anywhere.
 *
 * `canUseOffline` mirrors the spec's rule that the 「离线使用」 escape hatch only
 * appears when there is something cached to look at.
 */
export function LoginPage({
  onSignedIn,
  onUseOffline,
  canUseOffline,
  initialMessage,
}: {
  onSignedIn: (session: SessionView) => void;
  onUseOffline: () => void;
  canUseOffline: boolean;
  initialMessage?: string | null;
}) {
  const api = useApi();
  const { toast } = useToast();
  const [phase, setPhase] = useState<Phase>("idle");
  const [error, setError] = useState<string | null>(initialMessage ?? null);
  const [isNetworkError, setIsNetworkError] = useState(false);
  const [manualCode, setManualCode] = useState("");
  const [busy, setBusy] = useState(false);
  const mounted = useRef(true);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  useEffect(() => {
    let disposeCompleted: (() => void) | undefined;
    let disposeFailed: (() => void) | undefined;

    void api.on<SessionView>(EVENTS.loginCompleted, (session) => {
      if (!mounted.current) return;
      setPhase("idle");
      announceActivation(session);
      onSignedIn(session);
    }).then((dispose) => {
      disposeCompleted = dispose;
    });

    void api.on<LoginFailure>(EVENTS.loginFailed, (failure) => {
      if (!mounted.current) return;
      setPhase("failed");
      setError(failure.message);
      setIsNetworkError(failure.network);
    }).then((dispose) => {
      disposeFailed = dispose;
    });

    return () => {
      disposeCompleted?.();
      disposeFailed?.();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [api]);

  function announceActivation(session: SessionView) {
    // 5.1 邀请试用发放：带邀请归因的账号首次授权成功时弹出祝贺 toast。
    if (session.activation?.trialGranted) {
      toast(t("login.trialGranted", { date: formatDate(session.activation.trialExpiresAt) }));
    } else if (session.activation?.trialDeniedReuse) {
      toast(t("fixed.trialReuse"));
    }
  }

  async function start() {
    setError(null);
    setIsNetworkError(false);
    setPhase("waiting");
    try {
      await api.beginLogin();
    } catch (caught) {
      const failure = toApiError(caught);
      setPhase("failed");
      setError(failure.message);
      setIsNetworkError(failure.code === "network");
    }
  }

  async function reopen() {
    try {
      await api.reopenBrowser();
    } catch (caught) {
      setError(toApiError(caught).message);
    }
  }

  async function cancel() {
    await api.cancelLogin();
    setPhase("idle");
    setError(null);
  }

  async function submitManualCode() {
    setBusy(true);
    setError(null);
    try {
      const session = await api.submitManualCode(manualCode);
      announceActivation(session);
      onSignedIn(session);
    } catch (caught) {
      setError(toApiError(caught).message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="auth-page">
      <div className="auth-card">
        <div className="logo">
          <span className="mark" aria-hidden="true" />
          {t("brand.name")} <span className="latin">{t("brand.latin")}</span>
        </div>

        {phase === "idle" && (
          <>
            <h2>{t("login.title")}</h2>
            <p className="sub">点击下方按钮，在浏览器中登录并授权本应用。</p>
            {error && (
              <Banner
                tone="error"
                action={
                  <button type="button" className="btn btn-secondary btn-sm" onClick={start}>
                    {t("common.retry")}
                  </button>
                }
              >
                {error}
              </Banner>
            )}
            <button type="button" className="btn btn-primary" onClick={start}>
              {t("login.button")}
            </button>
            {canUseOffline && (
              <button type="button" className="btn btn-ghost" onClick={onUseOffline}>
                {t("login.offlineEnter")}
              </button>
            )}
            <div className="terms">{t("login.explain")}</div>
          </>
        )}

        {phase === "waiting" && (
          <>
            <h2>{t("login.waitingTitle")}</h2>
            <p className="sub">{t("login.waitingBody")}</p>
            <div style={{ margin: "6px 0 22px" }}>
              <Spinner dark />
            </div>
            <div className="row-actions">
              <button type="button" className="btn btn-secondary" onClick={reopen}>
                {t("login.reopen")}
              </button>
              <button type="button" className="btn btn-ghost" onClick={cancel}>
                {t("common.cancel")}
              </button>
            </div>
          </>
        )}

        {phase === "failed" && (
          <>
            <h2>{t("login.failedTitle")}</h2>
            <Banner tone="error">{error ?? t("login.timeout")}</Banner>
            <button type="button" className="btn btn-primary" onClick={start}>
              {t("common.retry")}
            </button>
            {isNetworkError && canUseOffline && (
              <button type="button" className="btn btn-ghost" onClick={onUseOffline}>
                {t("login.offlineEnter")}
              </button>
            )}
            <div className="field" style={{ marginTop: 18 }}>
              <label htmlFor="manual-code">{t("login.manualHint")}</label>
              <input
                id="manual-code"
                value={manualCode}
                placeholder={t("login.manualPlaceholder")}
                onChange={(event) => setManualCode(event.target.value)}
              />
            </div>
            <button
              type="button"
              className="btn btn-secondary"
              onClick={submitManualCode}
              disabled={busy || manualCode.trim().length === 0}
            >
              {busy && <Spinner dark />}
              {t("login.manualSubmit")}
            </button>
          </>
        )}
      </div>
    </div>
  );
}
