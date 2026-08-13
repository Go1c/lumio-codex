import { useState, type FormEvent } from "react";
import { Link, useNavigate, useSearchParams } from "react-router-dom";

import { login as loginRequest, resendVerificationCode } from "@/api/endpoints";
import { TextField } from "@/components/fields";
import { Banner, Spinner, errorMessage } from "@/components/ui";
import { useCountdown } from "@/hooks/useCountdown";
import { useT } from "@/i18n";
import { ApiError } from "@/lib/api";
import { emailValid } from "@/lib/validation";
import { useSession } from "@/state/session";

type LoginFailure = "" | "credentials" | "unverified" | "locked" | "disabled" | "other";

/** 只接受站内相对路径，避免 `next` 被用作开放重定向。 */
function safeNext(raw: string | null): string | null {
  if (!raw) return null;
  if (!raw.startsWith("/") || raw.startsWith("//")) return null;
  return raw;
}

/** 4.7 登录页：凭据错误 / 邮箱未验证 / 账号锁定 / 账号停用 四种失败分支。 */
export function Login() {
  const t = useT();
  const navigate = useNavigate();
  const [params] = useSearchParams();
  const { applySnapshot } = useSession();

  const next = safeNext(params.get("next"));

  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [failure, setFailure] = useState<LoginFailure>("");
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState(false);
  const [lockRemaining, startLockCountdown] = useCountdown(0);

  const locked = failure === "locked" && lockRemaining > 0;
  const canSubmit = emailValid(email) && password.length > 0 && !busy && !locked && failure !== "disabled";

  async function submit() {
    if (!canSubmit) return;

    setBusy(true);
    setFailure("");
    setMessage("");

    try {
      const snapshot = await loginRequest(email.trim(), password);
      applySnapshot(snapshot);
      navigate(next ?? "/account");
    } catch (error) {
      if (!(error instanceof ApiError)) {
        setFailure("other");
        setMessage(errorMessage(error, t("common.unknown_error")));
        return;
      }

      setMessage(error.message);
      switch (error.code) {
        case "invalid_credentials":
          setFailure("credentials");
          setPassword("");
          document.getElementById("login-password")?.focus();
          break;
        case "email_unverified":
          setFailure("unverified");
          break;
        case "account_locked":
        case "rate_limited":
          setFailure("locked");
          startLockCountdown(error.retryAfterSeconds ?? 900);
          break;
        case "account_disabled":
          setFailure("disabled");
          break;
        default:
          setFailure("other");
      }
    } finally {
      setBusy(false);
    }
  }

  async function resendVerification() {
    try {
      await resendVerificationCode(email.trim());
    } finally {
      navigate(`/verify-email?email=${encodeURIComponent(email.trim())}`);
    }
  }

  return (
    <div className="auth-page">
      <div className="auth-card">
        <h2>{t("login.title")}</h2>
        <p className="sub">{t("login.subtitle")}</p>

        {failure === "unverified" && (
          <Banner
            kind="warn"
            action={
              <button type="button" className="btn btn-secondary" onClick={() => void resendVerification()}>
                {t("login.resend_verification")}
              </button>
            }
          >
            {message}
          </Banner>
        )}

        {failure === "locked" && (
          <Banner kind="error">
            {message}
            {lockRemaining > 0 && <>（{t("login.locked_countdown", { n: lockRemaining })}）</>}
          </Banner>
        )}

        {failure === "disabled" && <Banner kind="error">{message}</Banner>}

        {failure === "other" && (
          <Banner
            kind="error"
            action={
              <button type="button" className="btn btn-secondary" onClick={() => void submit()}>
                {t("common.retry")}
              </button>
            }
          >
            {message}
          </Banner>
        )}

        <form
          onSubmit={(event: FormEvent) => {
            event.preventDefault();
            void submit();
          }}
          noValidate
        >
          <fieldset className="form-fieldset" disabled={busy}>
            <TextField
              label={t("signup.email")}
              type="email"
              value={email}
              autoComplete="email"
              placeholder="you@example.com"
              onChange={(event) => setEmail(event.target.value)}
            />

            <div className="field">
              {/* 「忘记密码？」放在 label 外，避免污染密码输入框的可访问名称。 */}
              <div className="label-row">
                <label htmlFor="login-password">{t("password.label")}</label>
                <Link to="/forgot-password">{t("login.forgot")}</Link>
              </div>
              <input
                id="login-password"
                type="password"
                value={password}
                autoComplete="current-password"
                placeholder="••••••••"
                className={failure === "credentials" ? "invalid" : ""}
                aria-invalid={failure === "credentials" || undefined}
                aria-describedby={failure === "credentials" ? "login-password-error" : undefined}
                onChange={(event) => {
                  setPassword(event.target.value);
                  if (failure === "credentials") setFailure("");
                }}
              />
              {failure === "credentials" && (
                <div className="err" id="login-password-error">
                  {message}
                </div>
              )}
            </div>

            <button type="submit" className="btn btn-primary btn-block" disabled={!canSubmit}>
              {busy && <Spinner />}
              {busy ? t("login.submitting") : t("login.submit")}
            </button>
          </fieldset>
        </form>

        <div className="auth-links">
          {t("login.new_user")}
          <Link to="/signup">{t("login.create_account")}</Link>
        </div>
      </div>
    </div>
  );
}
