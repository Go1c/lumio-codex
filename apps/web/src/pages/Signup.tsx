import { useState, type FormEvent } from "react";
import { Link, useNavigate } from "react-router-dom";

import { register } from "@/api/endpoints";
import { PasswordField, TextField } from "@/components/fields";
import { useToast } from "@/components/Toast";
import { Banner, Spinner, errorMessage } from "@/components/ui";
import { useT } from "@/i18n";
import { ApiError } from "@/lib/api";
import { emailValid, passwordValid } from "@/lib/validation";
import { useInviteAttribution } from "@/state/inviteAttribution";

type EmailError = "" | "format" | "taken";

/** 4.5 注册页：邀请横幅 + 邮箱 blur 校验 + 密码强度与规则清单。 */
export function Signup() {
  const t = useT();
  const toast = useToast();
  const navigate = useNavigate();
  // 归因未确定或接口失败时 attribution 为 null，横幅整块不渲染——宁可不显示，也不给错误承诺。
  const attribution = useInviteAttribution();

  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [emailError, setEmailError] = useState<EmailError>("");
  const [formError, setFormError] = useState("");
  const [busy, setBusy] = useState(false);

  const canSubmit = emailValid(email) && passwordValid(password) && !busy;

  async function submit() {
    if (!canSubmit) return;

    setBusy(true);
    setFormError("");
    setEmailError("");

    try {
      await register(email.trim(), password);
      navigate(`/verify-email?email=${encodeURIComponent(email.trim())}`);
    } catch (error) {
      if (error instanceof ApiError && error.code === "email_taken") {
        setEmailError("taken");
      } else if (error instanceof ApiError && error.code === "email_invalid") {
        setEmailError("format");
      } else if (error instanceof ApiError && error.code === "rate_limited") {
        // 4.5 节：频率限制用 toast，文案直接取服务端下发的 6.2 节模板。
        toast(error.message);
      } else {
        setFormError(errorMessage(error, t("common.unknown_error")));
      }
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="auth-page">
      <div className="auth-card">
        <h2>{t("signup.title")}</h2>
        <p className="sub">{t("signup.subtitle")}</p>

        {attribution?.attributed && (
          <Banner kind="ok">{t("signup.invite_banner", { inviter: attribution.inviter || "朋友" })}</Banner>
        )}

        {formError && (
          <Banner
            kind="error"
            action={
              <button type="button" className="btn btn-secondary" onClick={() => void submit()}>
                {t("common.retry")}
              </button>
            }
          >
            {formError}
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
              onChange={(event) => {
                setEmail(event.target.value);
                setEmailError("");
              }}
              onBlur={() => setEmailError(email && !emailValid(email) ? "format" : "")}
              error={
                emailError === "format" ? (
                  t("signup.email_invalid")
                ) : emailError === "taken" ? (
                  <>
                    该邮箱已注册。<Link to="/login">{t("signup.email_taken_login")}</Link> 或{" "}
                    <Link to="/forgot-password">{t("signup.email_taken_forgot")}</Link>。
                  </>
                ) : undefined
              }
            />

            <PasswordField value={password} onChange={setPassword} />

            <button type="submit" className="btn btn-primary btn-block" disabled={!canSubmit}>
              {busy && <Spinner />}
              {busy ? t("signup.submitting") : t("signup.submit")}
            </button>
          </fieldset>
        </form>

        <div className="auth-links">
          {t("signup.have_account")}
          <Link to="/login">{t("signup.login")}</Link>
        </div>
        <div className="terms">{t("signup.terms")}</div>
      </div>
    </div>
  );
}
