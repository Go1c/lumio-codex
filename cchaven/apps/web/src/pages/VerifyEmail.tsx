import { useEffect, useState } from "react";
import { Link, useNavigate, useSearchParams } from "react-router-dom";

import { resendVerificationCode, verifyEmail } from "@/api/endpoints";
import { CodeInput } from "@/components/CodeInput";
import { useToast } from "@/components/Toast";
import { Banner, Spinner, Truncated, errorMessage } from "@/components/ui";
import { useCountdown } from "@/hooks/useCountdown";
import { useT } from "@/i18n";
import { ApiError } from "@/lib/api";
import { useSession } from "@/state/session";

const RESEND_COOLDOWN_SECONDS = 60; // 6.2 节：60 秒重发冷却
const APP_DEEP_LINK = "cchaven://open";

/**
 * 4.6 邮箱验证页：6 格输入、填满自动提交、60 秒重发冷却。
 *
 * 五态：loading（提交中格子只读 + spinner）/ error（错误码文案）/
 * disabled（尝试耗尽或过期后格子禁用，仅重发可用）/ empty（缺邮箱参数）/ 无权限（不适用）。
 */
export function VerifyEmail() {
  const t = useT();
  const toast = useToast();
  const navigate = useNavigate();
  const [params] = useSearchParams();
  const { applySnapshot } = useSession();

  const email = params.get("email") ?? "";

  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState(false);
  const [errorText, setErrorText] = useState("");
  const [errorNonce, setErrorNonce] = useState(0);
  const [exhausted, setExhausted] = useState(false);
  const [cooldown, startCooldown] = useCountdown(RESEND_COOLDOWN_SECONDS);

  useEffect(() => {
    startCooldown(RESEND_COOLDOWN_SECONDS);
  }, [startCooldown]);

  if (!email) {
    return (
      <div className="auth-page">
        <div className="auth-card">
          <h2>{t("verify.title")}</h2>
          <Banner kind="warn">{t("verify.missing_email")}</Banner>
          <Link to="/signup" className="btn btn-primary btn-block">
            {t("verify.back_to_signup")}
          </Link>
        </div>
      </div>
    );
  }

  async function submitCode(code: string) {
    setBusy(true);
    setErrorText("");
    try {
      const snapshot = await verifyEmail(email, code);
      applySnapshot(snapshot);
      setDone(true);
    } catch (error) {
      setErrorNonce((nonce) => nonce + 1);
      if (error instanceof ApiError) {
        setErrorText(error.message);
        // code_expired 同时覆盖「过期」与「尝试次数耗尽」，两种情况都只能重新发送。
        if (error.code === "code_expired" || error.code === "code_attempts_exhausted") {
          setExhausted(true);
        }
      } else {
        setErrorText(errorMessage(error, t("common.unknown_error")));
      }
    } finally {
      setBusy(false);
    }
  }

  async function resend() {
    try {
      const result = await resendVerificationCode(email);
      startCooldown(result?.retry_after_seconds || RESEND_COOLDOWN_SECONDS);
      setExhausted(false);
      setErrorText("");
      toast(t("verify.resent_toast"));
    } catch (error) {
      if (error instanceof ApiError) {
        // 429 resend_cooldown 会带 retry_after_seconds，按服务端节奏重置倒计时。
        startCooldown(error.retryAfterSeconds ?? RESEND_COOLDOWN_SECONDS);
        toast(error.message);
      } else {
        toast(errorMessage(error, t("common.unknown_error")));
      }
    }
  }

  if (done) {
    return (
      <div className="auth-page">
        <div className="auth-card">
          <div className="success-check" aria-hidden="true">
            ✓
          </div>
          <h2>{t("verify.success_title")}</h2>
          <p className="sub">{t("verify.success_sub")}</p>
          <button
            type="button"
            className="btn btn-primary btn-block"
            onClick={() => navigate("/download")}
          >
            {t("verify.cta_download")}
          </button>
          <a href={APP_DEEP_LINK} className="btn btn-secondary btn-block" style={{ marginTop: 10 }}>
            {t("verify.cta_open_app")}
          </a>
        </div>
      </div>
    );
  }

  return (
    <div className="auth-page">
      <div className="auth-card">
        <h2>{t("verify.title")}</h2>
        <p className="sub">
          {t("verify.subtitle_prefix")} <Truncated text={email} max={30} />{" "}
          {t("verify.subtitle_suffix")}{" "}
          <Link to="/signup" style={{ fontSize: 13 }}>
            {t("verify.change_email")}
          </Link>
        </p>

        <CodeInput
          disabled={busy || exhausted}
          errorNonce={errorNonce}
          onComplete={(code) => void submitCode(code)}
        />

        {busy && (
          <p className="sub" style={{ marginTop: 10 }} role="status">
            <Spinner dark /> {t("verify.checking")}
          </p>
        )}

        {errorText && (
          <p className="form-error" role="alert">
            {errorText}
          </p>
        )}

        <div className="auth-links">
          {cooldown > 0 ? (
            <span>{t("verify.resend_countdown", { n: cooldown })}</span>
          ) : (
            <button type="button" className="btn btn-ghost" onClick={() => void resend()}>
              {t("verify.resend")}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
