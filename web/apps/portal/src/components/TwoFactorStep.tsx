import { useState, type FormEvent } from "react";

import { Banner, Spinner, TextField } from "@lumio/ui";

/** 2FA 是登录成功响应的一个分支，不是错误：独立一步收 TOTP 验证码。 */
export function TwoFactorStep({
  maskedEmail,
  error,
  busy,
  onSubmit,
}: {
  maskedEmail: string;
  error?: string;
  busy: boolean;
  onSubmit: (code: string) => void;
}) {
  const [code, setCode] = useState("");

  function submit(event: FormEvent) {
    event.preventDefault();
    onSubmit(code.trim());
  }

  return (
    <div className="auth-card">
      <h2>两步验证</h2>
      <p className="sub">
        请输入验证器中 {maskedEmail} 对应的 6 位动态码。
      </p>
      {error && <Banner kind="error">{error}</Banner>}
      <form onSubmit={submit}>
        <fieldset className="form-fieldset" disabled={busy}>
          <TextField
            label="两步验证码"
            value={code}
            inputMode="numeric"
            autoComplete="one-time-code"
            maxLength={6}
            required
            onChange={(event) => setCode(event.target.value)}
          />
          <button type="submit" className="btn btn-primary btn-block">
            {busy && <Spinner />}
            验证并登录
          </button>
        </fieldset>
      </form>
    </div>
  );
}
