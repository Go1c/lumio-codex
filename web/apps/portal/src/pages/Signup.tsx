import { useState, type FormEvent } from "react";
import { Link, useSearchParams } from "react-router-dom";

import { loginTwoFactor, register, sendVerifyCode, type PublicSettings } from "@lumio/auth";
import { Banner, ErrorBlock, LoadingBlock, PasswordField, Spinner, TextField } from "@lumio/ui";

import { TwoFactorStep } from "@/components/TwoFactorStep";
import { useCountdown } from "@/hooks/useCountdown";
import { usePublicSettings } from "@/hooks/usePublicSettings";
import { messageOf, useAuthOutcome } from "@/lib/authFlow";
import { readAffiliateRef } from "@/lib/affiliateRef";
import { withNext } from "@/lib/redirect";

export function Signup() {
  const settings = usePublicSettings();

  return (
    <div className="auth-page">
      <div className="auth-card wide">
        <h2>创建账号</h2>
        {settings.status === "loading" && <LoadingBlock label="读取注册设置…" />}
        {settings.status === "error" && (
          <ErrorBlock message={settings.error ?? ""} onRetry={settings.reload} />
        )}
        {settings.status === "ready" && settings.data && <SignupForm settings={settings.data} />}
      </div>
    </div>
  );
}

function SignupForm({ settings }: { settings: PublicSettings }) {
  const [params] = useSearchParams();
  const next = params.get("next");
  const { challenge, apply } = useAuthOutcome(next);

  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [code, setCode] = useState("");
  const [affCode] = useState(() => readAffiliateRef(params));
  const [invitationCode, setInvitationCode] = useState(
    () => (params.get("invite") ?? "").trim() || (affCode ? affCode.toUpperCase() : ""),
  );
  const [agreed, setAgreed] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [remaining, startCountdown] = useCountdown();

  const whitelist = settings.emailSuffixWhitelist;

  if (!settings.registrationEnabled) {
    return <Banner kind="warn">当前未开放注册。如需账号，请联系客服。</Banner>;
  }

  async function run(action: () => Promise<void>) {
    setBusy(true);
    setError(null);
    try {
      await action();
    } catch (failure) {
      setError(messageOf(failure));
    } finally {
      setBusy(false);
    }
  }

  function localError(): string | null {
    const value = email.trim().toLowerCase();
    if (whitelist.length > 0 && !whitelist.some((suffix) => value.endsWith(suffix.toLowerCase()))) {
      return `仅支持以下邮箱后缀注册：${whitelist.join("、")}`;
    }
    if (settings.emailVerifyEnabled && !code.trim()) {
      return "请先获取并填写邮箱验证码。";
    }
    if (settings.agreementEnabled && !agreed) {
      return "请先阅读并同意下方协议。";
    }
    return null;
  }

  function submit(event: FormEvent) {
    event.preventDefault();
    const invalid = localError();
    if (invalid) {
      setError(invalid);
      return;
    }
    void run(async () =>
      apply(
        await register({
          email: email.trim(),
          password,
          verifyCode: code.trim() || undefined,
          invitationCode: invitationCode.trim() || undefined,
          affCode: affCode || undefined,
        }),
      ),
    );
  }

  if (challenge) {
    return (
      <TwoFactorStep
        maskedEmail={challenge.maskedEmail}
        error={error ?? undefined}
        busy={busy}
        onSubmit={(totp) =>
          void run(async () => apply(await loginTwoFactor(challenge.tempToken, totp)))
        }
      />
    );
  }

  return (
    <>
      {error && <Banner kind="error">{error}</Banner>}
      {affCode && (
        <Banner kind="info">已接受好友邀请（{affCode.toUpperCase()}），注册后自动绑定邀请关系。</Banner>
      )}
      <form onSubmit={submit}>
        <fieldset className="form-fieldset" disabled={busy}>
          <TextField
            label="邮箱"
            type="email"
            value={email}
            autoComplete="email"
            required
            hint={whitelist.length > 0 ? `仅支持：${whitelist.join("、")}` : undefined}
            onChange={(event) => setEmail(event.target.value)}
          />

          {settings.emailVerifyEnabled && (
            <div className="field-row">
              <TextField
                label="邮箱验证码"
                value={code}
                inputMode="numeric"
                autoComplete="one-time-code"
                required
                onChange={(event) => setCode(event.target.value)}
              />
              <button
                type="button"
                className="btn btn-secondary"
                style={{ marginTop: 26 }}
                disabled={!email.trim() || remaining > 0}
                onClick={() =>
                  void run(async () => startCountdown(await sendVerifyCode(email.trim())))
                }
              >
                {remaining > 0 ? `${remaining} 秒后可重发` : "发送验证码"}
              </button>
            </div>
          )}

          <PasswordField value={password} onChange={setPassword} required />

          <TextField
            label="邀请码（选填）"
            value={invitationCode}
            onChange={(event) => setInvitationCode(event.target.value)}
          />

          {settings.agreementEnabled && (
            <label className="checkbox-field">
              <input
                type="checkbox"
                checked={agreed}
                onChange={(event) => setAgreed(event.target.checked)}
              />
              <span>
                我已阅读并同意
                {settings.agreementDocuments.map((doc) => (
                  <span key={doc.id}>《{doc.title}》</span>
                ))}
              </span>
            </label>
          )}

          <button type="submit" className="btn btn-primary btn-block">
            {busy && <Spinner />}
            创建账号
          </button>
        </fieldset>
      </form>
      <p className="auth-links">
        已有账号？<Link to={withNext("/login", next)}>登录</Link>
      </p>
    </>
  );
}
