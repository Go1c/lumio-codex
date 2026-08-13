import { useState, type FormEvent } from "react";
import { Link } from "react-router-dom";

import { forgotPassword } from "@/api/endpoints";
import { TextField } from "@/components/fields";
import { Banner, Spinner, errorMessage } from "@/components/ui";
import { useCountdown } from "@/hooks/useCountdown";
import { useT } from "@/i18n";
import { ApiError } from "@/lib/api";
import { emailValid } from "@/lib/validation";

const RESUBMIT_COOLDOWN_SECONDS = 60;

/**
 * 4.8 忘记密码页。
 *
 * 提交后卡片整体切换为确认态，文案直接展示服务端下发的 6.2 节恒定回执
 * （无论邮箱是否注册都是同一句，防枚举）。
 */
export function ForgotPassword() {
  const t = useT();

  const [email, setEmail] = useState("");
  const [receipt, setReceipt] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [cooldown, startCooldown] = useCountdown(0);

  async function submit() {
    if (!emailValid(email) || busy || cooldown > 0) return;

    setBusy(true);
    setError("");
    try {
      const result = await forgotPassword(email.trim());
      setReceipt(result.message);
      startCooldown(RESUBMIT_COOLDOWN_SECONDS);
    } catch (err) {
      if (err instanceof ApiError && err.code === "rate_limited") {
        startCooldown(err.retryAfterSeconds ?? RESUBMIT_COOLDOWN_SECONDS);
      }
      setError(errorMessage(err, t("common.unknown_error")));
    } finally {
      setBusy(false);
    }
  }

  if (receipt) {
    return (
      <div className="auth-page">
        <div className="auth-card">
          <div className="success-check blue" aria-hidden="true">
            ✉
          </div>
          <h2>{t("forgot.sent_title")}</h2>
          <p className="sub">{receipt}</p>
          <button
            type="button"
            className="btn btn-secondary btn-block"
            disabled={cooldown > 0}
            onClick={() => void submit()}
          >
            {cooldown > 0 ? t("forgot.cooldown", { n: cooldown }) : t("forgot.submit")}
          </button>
          <div className="auth-links">
            <Link to="/login">{t("forgot.back_to_login")}</Link>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="auth-page">
      <div className="auth-card">
        <h2>{t("forgot.title")}</h2>
        <p className="sub">{t("forgot.subtitle")}</p>

        {error && (
          <Banner
            kind="error"
            action={
              <button type="button" className="btn btn-secondary" onClick={() => void submit()}>
                {t("common.retry")}
              </button>
            }
          >
            {error}
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
            <button
              type="submit"
              className="btn btn-primary btn-block"
              disabled={!emailValid(email) || busy || cooldown > 0}
            >
              {busy && <Spinner />}
              {busy
                ? t("forgot.submitting")
                : cooldown > 0
                  ? t("forgot.cooldown", { n: cooldown })
                  : t("forgot.submit")}
            </button>
          </fieldset>
        </form>

        <div className="auth-links">
          <Link to="/login">{t("forgot.back_to_login")}</Link>
        </div>
      </div>
    </div>
  );
}
