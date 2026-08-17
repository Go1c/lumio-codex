import { useState, type FormEvent } from "react";
import { Link, useSearchParams } from "react-router-dom";

import { login, loginTwoFactor } from "@lumio/auth";
import { Banner, PasswordField, Spinner, TextField } from "@lumio/ui";

import { TwoFactorStep } from "@/components/TwoFactorStep";
import { messageOf, useAuthOutcome } from "@/lib/authFlow";
import { withNext } from "@/lib/redirect";

export function Login() {
  const [params] = useSearchParams();
  const next = params.get("next");
  const { challenge, apply } = useAuthOutcome(next);

  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

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

  function submit(event: FormEvent) {
    event.preventDefault();
    void run(async () => apply(await login(email.trim(), password)));
  }

  if (challenge) {
    return (
      <div className="auth-page">
        <TwoFactorStep
          maskedEmail={challenge.maskedEmail}
          error={error ?? undefined}
          busy={busy}
          onSubmit={(code) =>
            void run(async () => apply(await loginTwoFactor(challenge.tempToken, code)))
          }
        />
      </div>
    );
  }

  return (
    <div className="auth-page">
      <div className="auth-card">
        <h2>登录</h2>
        {error && <Banner kind="error">{error}</Banner>}
        <form onSubmit={submit}>
          <fieldset className="form-fieldset" disabled={busy}>
            <TextField
              label="邮箱"
              type="email"
              value={email}
              autoComplete="email"
              required
              onChange={(event) => setEmail(event.target.value)}
            />
            <PasswordField
              value={password}
              onChange={setPassword}
              autoComplete="current-password"
              showRules={false}
              required
            />
            <button type="submit" className="btn btn-primary btn-block">
              {busy && <Spinner />}
              登录
            </button>
          </fieldset>
        </form>
        <p className="auth-links">
          还没有账号？<Link to={withNext("/signup", next)}>创建账号</Link>
        </p>
      </div>
    </div>
  );
}
