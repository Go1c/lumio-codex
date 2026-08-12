import { useEffect, useState, type FormEvent } from "react";
import { Link, useNavigate, useSearchParams } from "react-router-dom";

import { inspectResetToken, resetPassword } from "@/api/endpoints";
import type { ResetTokenInspection } from "@/api/types";
import { PasswordField, TextField } from "@/components/fields";
import { Banner, Skeleton, Spinner, errorMessage } from "@/components/ui";
import { useResource } from "@/hooks/useResource";
import { useT } from "@/i18n";
import { ApiError } from "@/lib/api";
import { passwordValid } from "@/lib/validation";

const REDIRECT_DELAY_MS = 3000;

/** 4.8 重设密码页：token 校验中显示骨架，失效给「重新申请链接」，成功 3 秒后跳登录。 */
export function ResetPassword() {
  const t = useT();
  const navigate = useNavigate();
  const [params] = useSearchParams();
  const token = params.get("token") ?? "";

  const inspection = useResource<ResetTokenInspection>(
    (signal) => inspectResetToken(token, signal),
    [token],
    { enabled: token !== "" },
  );

  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [doneMessage, setDoneMessage] = useState("");

  useEffect(() => {
    if (!doneMessage) return;
    const timer = setTimeout(() => navigate("/login"), REDIRECT_DELAY_MS);
    return () => clearTimeout(timer);
  }, [doneMessage, navigate]);

  const invalidToken =
    token === "" ||
    (inspection.status === "error" &&
      (!(inspection.error instanceof ApiError) || inspection.error.code === "reset_link_invalid"));

  async function submit() {
    if (!passwordValid(password) || password !== confirm || busy) return;

    setBusy(true);
    setError("");
    try {
      const result = await resetPassword(token, password);
      setDoneMessage(result?.message ?? "");
    } catch (err) {
      setError(errorMessage(err, t("common.unknown_error")));
    } finally {
      setBusy(false);
    }
  }

  if (doneMessage) {
    return (
      <div className="auth-page">
        <div className="auth-card">
          <div className="success-check" aria-hidden="true">
            ✓
          </div>
          <h2>{t("reset.done_title")}</h2>
          <p className="sub" role="status">
            {doneMessage}
          </p>
          <p className="sub">{t("reset.done_sub")}</p>
          <Link to="/login" className="btn btn-primary btn-block">
            {t("forgot.back_to_login")}
          </Link>
        </div>
      </div>
    );
  }

  if (token !== "" && inspection.status === "loading") {
    return (
      <div className="auth-page">
        <div className="auth-card" aria-busy="true">
          <span className="sr-only">{t("common.loading")}</span>
          <Skeleton height={22} width={200} />
          <div style={{ height: 14 }} />
          <Skeleton height={44} />
          <div style={{ height: 10 }} />
          <Skeleton height={44} />
        </div>
      </div>
    );
  }

  if (invalidToken) {
    return (
      <div className="auth-page">
        <div className="auth-card">
          <h2>{t("reset.invalid_title")}</h2>
          <p className="sub">
            {inspection.error instanceof ApiError ? inspection.error.message : "该链接已过期或已被使用。"}
          </p>
          <Link to="/forgot-password" className="btn btn-primary btn-block">
            {t("reset.invalid_cta")}
          </Link>
        </div>
      </div>
    );
  }

  if (inspection.status === "error") {
    return (
      <div className="auth-page">
        <div className="auth-card">
          <Banner
            kind="error"
            action={
              <button type="button" className="btn btn-secondary" onClick={inspection.reload}>
                {t("common.retry")}
              </button>
            }
          >
            {errorMessage(inspection.error, t("common.unknown_error"))}
          </Banner>
        </div>
      </div>
    );
  }

  const mismatch = confirm.length > 0 && confirm !== password;

  return (
    <div className="auth-page">
      <div className="auth-card">
        <h2>{t("reset.title")}</h2>
        <p className="sub">
          {t("reset.subtitle")}
          {inspection.data?.email_masked ? <> （{inspection.data.email_masked}）</> : null}
        </p>

        {error && <Banner kind="error">{error}</Banner>}

        <form
          onSubmit={(event: FormEvent) => {
            event.preventDefault();
            void submit();
          }}
          noValidate
        >
          <fieldset className="form-fieldset" disabled={busy}>
            <PasswordField label={t("password.new")} value={password} onChange={setPassword} />
            <TextField
              label={t("password.confirm")}
              type="password"
              value={confirm}
              autoComplete="new-password"
              onChange={(event) => setConfirm(event.target.value)}
              error={mismatch ? t("password.mismatch") : undefined}
            />
            <button
              type="submit"
              className="btn btn-primary btn-block"
              disabled={!passwordValid(password) || mismatch || confirm.length === 0 || busy}
            >
              {busy && <Spinner />}
              {busy ? t("reset.submitting") : t("reset.submit")}
            </button>
          </fieldset>
        </form>
      </div>
    </div>
  );
}
