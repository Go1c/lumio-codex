import { useCallback, useEffect, useState } from "react";
import { QRCodeSVG } from "qrcode.react";
import { ApiError } from "../api/client";
import * as api from "../api/endpoints";
import type { TotpEnrollment } from "../api/types";
import { ErrorBanner } from "../components/common";
import { useToast } from "../components/ToastProvider";
import { t } from "../i18n";
import { AuthCard } from "./AuthCard";
import { useAuth } from "./AuthProvider";

/**
 * 首次登录强制启用两步验证：setup 拿到 otpauth URI 渲染二维码，
 * 再用 App 里的首个验证码调 enable 完成启用。没有跳过入口。
 */
export function TotpEnroll() {
  const { completeEnrollment, logout } = useAuth();
  const { toast } = useToast();
  const [enrollment, setEnrollment] = useState<TotpEnrollment | null>(null);
  const [loadError, setLoadError] = useState("");
  const [code, setCode] = useState("");
  const [fieldError, setFieldError] = useState("");
  const [formError, setFormError] = useState("");
  const [submitting, setSubmitting] = useState(false);

  const startSetup = useCallback(async () => {
    setLoadError("");
    try {
      setEnrollment(await api.setupTotp());
    } catch (error) {
      setLoadError(error instanceof ApiError ? error.message : t("error.generic"));
    }
  }, []);

  useEffect(() => {
    void startSetup();
  }, [startSetup]);

  async function onSubmit(event: React.FormEvent) {
    event.preventDefault();
    if (code.trim().length !== 6) {
      setFieldError(t("mfa.codeRequired"));
      return;
    }

    setFormError("");
    setSubmitting(true);
    try {
      await completeEnrollment(code.trim());
      toast(t("mfa.enrolled"));
    } catch (error) {
      setFormError(error instanceof ApiError ? error.message : t("error.generic"));
      setSubmitting(false);
    }
  }

  return (
    <AuthCard title={t("mfa.enrollTitle")} subtitle={t("mfa.enrollSubtitle")}>
      {loadError && <ErrorBanner message={loadError} onRetry={() => void startSetup()} />}
      {formError && <ErrorBanner message={formError} />}

      {enrollment && (
        <>
          <div className="totp-qr">
            <QRCodeSVG value={enrollment.uri} size={168} title={t("mfa.qrAlt")} />
          </div>
          <div className="totp-secret">
            <span className="totp-secret-label">{t("mfa.secretLabel")}</span>
            <code>{enrollment.secret}</code>
          </div>

          <form onSubmit={onSubmit} noValidate>
            <fieldset disabled={submitting}>
              <div className="field">
                <label htmlFor="enroll-code">{t("mfa.enrollConfirm")}</label>
                <input
                  id="enroll-code"
                  inputMode="numeric"
                  autoComplete="one-time-code"
                  maxLength={6}
                  value={code}
                  className={`code-input ${fieldError ? "invalid" : ""}`}
                  aria-invalid={fieldError ? true : undefined}
                  aria-describedby={fieldError ? "enroll-code-error" : undefined}
                  onChange={(event) => {
                    setCode(event.target.value.replace(/\D/g, ""));
                    if (fieldError) setFieldError("");
                  }}
                />
                {fieldError && (
                  <div className="err" id="enroll-code-error">
                    {fieldError}
                  </div>
                )}
              </div>
              <button type="submit" className="btn btn-primary">
                {submitting && <span className="spinner" />}
                {submitting ? t("mfa.enrollSubmitting") : t("mfa.enrollSubmit")}
              </button>
            </fieldset>
          </form>
        </>
      )}

      <div className="auth-links">
        <button type="button" className="btn btn-ghost" onClick={() => void logout()}>
          {t("nav.logout")}
        </button>
      </div>
    </AuthCard>
  );
}
