import { useState } from "react";
import { ApiError } from "../api/client";
import { ErrorBanner } from "../components/common";
import { t } from "../i18n";
import { AuthCard } from "./AuthCard";
import { useAuth } from "./AuthProvider";

/** 半会话补交 TOTP。此前的会话只能用于本接口，访问业务接口会被 401 mfa_required 拦回这里。 */
export function TotpChallenge() {
  const { verifyTotp, backToLogin } = useAuth();
  const [code, setCode] = useState("");
  const [fieldError, setFieldError] = useState("");
  const [formError, setFormError] = useState("");
  const [submitting, setSubmitting] = useState(false);

  async function onSubmit(event: React.FormEvent) {
    event.preventDefault();
    if (code.trim().length !== 6) {
      setFieldError(t("mfa.codeRequired"));
      return;
    }

    setFormError("");
    setSubmitting(true);
    try {
      await verifyTotp(code.trim());
    } catch (error) {
      setFormError(error instanceof ApiError ? error.message : t("error.generic"));
      setSubmitting(false);
    }
  }

  return (
    <AuthCard title={t("mfa.challengeTitle")} subtitle={t("mfa.challengeSubtitle")}>
      {formError && <ErrorBanner message={formError} />}
      <form onSubmit={onSubmit} noValidate>
        <fieldset disabled={submitting}>
          <div className="field">
            <label htmlFor="totp-code">{t("mfa.codeLabel")}</label>
            <input
              id="totp-code"
              inputMode="numeric"
              autoComplete="one-time-code"
              maxLength={6}
              value={code}
              className={`code-input ${fieldError ? "invalid" : ""}`}
              aria-invalid={fieldError ? true : undefined}
              aria-describedby={fieldError ? "totp-code-error" : undefined}
              onChange={(event) => {
                setCode(event.target.value.replace(/\D/g, ""));
                if (fieldError) setFieldError("");
              }}
            />
            {fieldError && (
              <div className="err" id="totp-code-error">
                {fieldError}
              </div>
            )}
          </div>
          <button type="submit" className="btn btn-primary">
            {submitting && <span className="spinner" />}
            {submitting ? t("mfa.submitting") : t("mfa.submit")}
          </button>
        </fieldset>
      </form>
      <div className="auth-links">
        <button type="button" className="btn btn-ghost" onClick={backToLogin}>
          {t("mfa.backToLogin")}
        </button>
      </div>
    </AuthCard>
  );
}
