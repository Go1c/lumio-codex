import { useState } from "react";
import { ApiError } from "../api/client";
import { ErrorBanner } from "../components/common";
import { t } from "../i18n";
import { AuthCard } from "./AuthCard";
import { useAuth } from "./AuthProvider";

const EMAIL_RE = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

/**
 * 管理员登录。失败文案一律用后端下发的 error.message（6.2 节固定文案，
 * 如「邮箱或密码不正确。」「尝试次数过多，请 {n} {unit}后再试。」），前端不重写。
 */
export function LoginPage() {
  const { login } = useAuth();
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [emailError, setEmailError] = useState("");
  const [passwordError, setPasswordError] = useState("");
  const [formError, setFormError] = useState("");
  const [submitting, setSubmitting] = useState(false);

  function validateEmail(value: string): string {
    if (!value.trim()) return t("login.emailRequired");
    if (!EMAIL_RE.test(value.trim())) return t("login.emailInvalid");
    return "";
  }

  async function onSubmit(event: React.FormEvent) {
    event.preventDefault();

    const nextEmailError = validateEmail(email);
    const nextPasswordError = password ? "" : t("login.passwordRequired");
    setEmailError(nextEmailError);
    setPasswordError(nextPasswordError);
    if (nextEmailError || nextPasswordError) return;

    setFormError("");
    setSubmitting(true);
    try {
      await login(email.trim(), password);
    } catch (error) {
      setFormError(error instanceof ApiError ? error.message : t("error.generic"));
      setSubmitting(false);
    }
  }

  return (
    <AuthCard title={t("login.title")} subtitle={t("login.subtitle")}>
      {formError && <ErrorBanner message={formError} />}
      <form onSubmit={onSubmit} noValidate>
        <fieldset disabled={submitting}>
          <div className="field">
            <label htmlFor="admin-email">{t("login.email")}</label>
            <input
              id="admin-email"
              type="email"
              autoComplete="username"
              value={email}
              className={emailError ? "invalid" : ""}
              aria-invalid={emailError ? true : undefined}
              aria-describedby={emailError ? "admin-email-error" : undefined}
              onBlur={() => setEmailError(validateEmail(email))}
              onChange={(event) => {
                setEmail(event.target.value);
                if (emailError) setEmailError("");
              }}
            />
            {emailError && (
              <div className="err" id="admin-email-error">
                {emailError}
              </div>
            )}
          </div>

          <div className="field">
            <label htmlFor="admin-password">{t("login.password")}</label>
            <input
              id="admin-password"
              type="password"
              autoComplete="current-password"
              value={password}
              className={passwordError ? "invalid" : ""}
              aria-invalid={passwordError ? true : undefined}
              aria-describedby={passwordError ? "admin-password-error" : undefined}
              onChange={(event) => {
                setPassword(event.target.value);
                if (passwordError) setPasswordError("");
              }}
            />
            {passwordError && (
              <div className="err" id="admin-password-error">
                {passwordError}
              </div>
            )}
          </div>

          <button type="submit" className="btn btn-primary">
            {submitting && <span className="spinner" />}
            {submitting ? t("login.submitting") : t("login.submit")}
          </button>
        </fieldset>
      </form>
    </AuthCard>
  );
}
