import { useEffect, useRef, useState } from "react";
import type { ClipboardEvent as ReactClipboardEvent, KeyboardEvent as ReactKeyboardEvent } from "react";

import { lumioErrorLabel } from "../errors.ts";
import { LumioCommandError, openInBrowser, shellLabels, signIn, submitTwoFactor } from "../invoke.ts";
import type { LumioAccountSummary, LumioAuthResult, LumioServiceSettings } from "../types.ts";
import type { ToastTone } from "./Toast.tsx";

const OTP_LENGTH = 6;
const SHAKE_MS = 400;
const CONTACT_SUPPORT_CODES = ["AUTH_ACCOUNT_DISABLED", "AUTH_2FA_UNAVAILABLE"];

function errorCodeOf(error: unknown): string {
  return error instanceof LumioCommandError ? error.errorCode : "UNKNOWN";
}

interface LoginViewProps {
  settings: LumioServiceSettings;
  step: "login" | "two-factor";
  onAuthenticated: (account: LumioAccountSummary) => void;
  onTwoFactorRequired: () => void;
  onBackToPassword: () => void;
  onCreateAccount: () => void;
  pushToast: (input: string, tone?: ToastTone) => void;
}

export function LoginView({
  settings,
  step,
  onAuthenticated,
  onTwoFactorRequired,
  onBackToPassword,
  onCreateAccount,
  pushToast,
}: LoginViewProps) {
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [passwordVisible, setPasswordVisible] = useState(false);
  const [bannerCode, setBannerCode] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [forgotOpen, setForgotOpen] = useState(false);
  const passwordRef = useRef<HTMLInputElement>(null);

  const applyResult = (result: LumioAuthResult) => {
    if (result.requiresTwoFactor) {
      setBannerCode(null);
      onTwoFactorRequired();
      return;
    }
    if (result.account !== null) onAuthenticated(result.account);
  };

  const submit = () => {
    if (submitting || email.trim() === "" || password === "") return;
    setSubmitting(true);
    setBannerCode(null);
    void signIn(email.trim(), password)
      .then(applyResult)
      .catch((error: unknown) => {
        const code = errorCodeOf(error);
        setBannerCode(code);
        if (code === "AUTH_INVALID_CREDENTIALS") {
          // Keep the email, drop the password, and put the cursor back where the
          // user has to act (interaction spec §5.3).
          setPassword("");
          passwordRef.current?.focus();
        }
      })
      .finally(() => setSubmitting(false));
  };

  if (step === "two-factor") {
    return (
      <TwoFactorStep
        bannerCode={bannerCode}
        onAuthenticated={onAuthenticated}
        onBackToPassword={() => {
          setBannerCode(null);
          onBackToPassword();
        }}
        onFailed={setBannerCode}
        pushToast={pushToast}
        settings={settings}
      />
    );
  }

  return (
    <div className="lumio-auth-wrap">
      <section className="lumio-auth-card">
        <p className="lumio-eyebrow">欢迎回来</p>
        <h1>{shellLabels.signIn}</h1>
        <p className="lumio-auth-lead">登录后会自动完成官方 Codex 的连接准备，无需任何手动配置。</p>

        {bannerCode === null ? null : (
          <ErrorBanner code={bannerCode} pushToast={pushToast} settings={settings} />
        )}

        <form
          onSubmit={(event) => {
            event.preventDefault();
            submit();
          }}
        >
          <div className="lumio-field">
            <label htmlFor="lumio-login-email">邮箱</label>
            <input
              autoComplete="email"
              className="lumio-input"
              disabled={submitting}
              id="lumio-login-email"
              onChange={(event) => setEmail(event.target.value)}
              placeholder="you@example.com"
              type="email"
              value={email}
            />
          </div>

          <div className="lumio-field">
            <label htmlFor="lumio-login-password">密码</label>
            <div className="lumio-input-row">
              <input
                autoComplete="current-password"
                className="lumio-input"
                disabled={submitting}
                id="lumio-login-password"
                onChange={(event) => setPassword(event.target.value)}
                placeholder="输入密码"
                ref={passwordRef}
                type={passwordVisible ? "text" : "password"}
                value={password}
              />
              <button
                className="lumio-button is-secondary"
                onClick={() => setPasswordVisible((visible) => !visible)}
                type="button"
              >
                {passwordVisible ? "隐藏" : "显示"}
              </button>
            </div>
            <p className="lumio-field-hint is-end">
              <button className="lumio-link-button" onClick={() => setForgotOpen(true)} type="button">
                忘记密码？
              </button>
            </p>
          </div>

          <button
            className="lumio-button is-primary is-large is-block"
            disabled={submitting || email.trim() === "" || password === ""}
            type="submit"
          >
            {submitting ? "正在验证…" : shellLabels.signIn}
          </button>
        </form>

        <p className="lumio-auth-foot">
          <button className="lumio-link-button" onClick={onCreateAccount} type="button">
            没有账户？创建账户
          </button>
        </p>
      </section>

      {forgotOpen ? (
        <ForgotPasswordDialog
          onClose={() => setForgotOpen(false)}
          pushToast={pushToast}
          settings={settings}
        />
      ) : null}
    </div>
  );
}

function ErrorBanner({
  code,
  settings,
  pushToast,
}: {
  code: string;
  settings: LumioServiceSettings;
  pushToast: (input: string, tone?: ToastTone) => void;
}) {
  return (
    <p className="lumio-banner" role="alert">
      {lumioErrorLabel(code)}
      {CONTACT_SUPPORT_CODES.includes(code) ? (
        <button
          className="lumio-link-button"
          onClick={() => {
            void openInBrowser(`${settings.siteBaseUrl}/support`).catch((error: unknown) =>
              pushToast(errorCodeOf(error)),
            );
          }}
          type="button"
        >
          联系支持
        </button>
      ) : null}
    </p>
  );
}

function ForgotPasswordDialog({
  settings,
  onClose,
  pushToast,
}: {
  settings: LumioServiceSettings;
  onClose: () => void;
  pushToast: (input: string, tone?: ToastTone) => void;
}) {
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onClose]);

  return (
    <div aria-modal="true" className="lumio-modal-backdrop" role="dialog">
      <div className="lumio-modal">
        <h3>密码重置在网页端完成</h3>
        <p>出于安全考虑，密码重置在浏览器中进行。我们会打开重置页面，完成后回到这里重新登录即可。</p>
        {settings.passwordResetEnabled ? null : <p className="lumio-field-error">密码重置暂未开放</p>}
        <div className="lumio-modal-actions">
          <button className="lumio-button is-secondary" onClick={onClose} type="button">
            取消
          </button>
          <button
            className="lumio-button is-primary"
            disabled={!settings.passwordResetEnabled}
            onClick={() => {
              onClose();
              void openInBrowser(`${settings.siteBaseUrl}/reset-password`).catch((error: unknown) =>
                pushToast(errorCodeOf(error)),
              );
            }}
            type="button"
          >
            在浏览器中打开
          </button>
        </div>
      </div>
    </div>
  );
}

function TwoFactorStep({
  settings,
  bannerCode,
  onAuthenticated,
  onBackToPassword,
  onFailed,
  pushToast,
}: {
  settings: LumioServiceSettings;
  bannerCode: string | null;
  onAuthenticated: (account: LumioAccountSummary) => void;
  onBackToPassword: () => void;
  onFailed: (code: string) => void;
  pushToast: (input: string, tone?: ToastTone) => void;
}) {
  const [digits, setDigits] = useState<string[]>(() => Array.from({ length: OTP_LENGTH }, () => ""));
  const [shaking, setShaking] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const inputs = useRef<(HTMLInputElement | null)[]>([]);

  useEffect(() => {
    inputs.current[0]?.focus();
  }, []);

  const verify = (code: string) => {
    setSubmitting(true);
    void submitTwoFactor(code)
      .then((result) => {
        if (result.account !== null) onAuthenticated(result.account);
      })
      .catch((error: unknown) => {
        onFailed(errorCodeOf(error));
        setDigits(Array.from({ length: OTP_LENGTH }, () => ""));
        setShaking(true);
        setTimeout(() => {
          setShaking(false);
          inputs.current[0]?.focus();
        }, SHAKE_MS);
      })
      .finally(() => setSubmitting(false));
  };

  const writeDigits = (next: string[]) => {
    setDigits(next);
    const code = next.join("");
    if (code.length === OTP_LENGTH && !next.includes("")) verify(code);
  };

  const onChangeDigit = (index: number, raw: string) => {
    const value = raw.replace(/\D/g, "").slice(-1);
    const next = [...digits];
    next[index] = value;
    writeDigits(next);
    if (value !== "" && index < OTP_LENGTH - 1) inputs.current[index + 1]?.focus();
  };

  const onKeyDownDigit = (index: number, event: ReactKeyboardEvent<HTMLInputElement>) => {
    if (event.key !== "Backspace" || digits[index] !== "" || index === 0) return;
    event.preventDefault();
    const next = [...digits];
    next[index - 1] = "";
    setDigits(next);
    inputs.current[index - 1]?.focus();
  };

  const onPasteDigits = (event: ReactClipboardEvent<HTMLInputElement>) => {
    const pasted = event.clipboardData.getData("text").replace(/\D/g, "").slice(0, OTP_LENGTH);
    if (pasted === "") return;
    event.preventDefault();
    const next = Array.from({ length: OTP_LENGTH }, (_unused, index) => pasted[index] ?? "");
    writeDigits(next);
    inputs.current[Math.min(pasted.length, OTP_LENGTH - 1)]?.focus();
  };

  return (
    <div className="lumio-auth-wrap">
      <section className="lumio-auth-card">
        <p className="lumio-eyebrow">两步验证</p>
        <h1>输入两步验证码</h1>
        <p className="lumio-auth-lead">打开你的验证器应用查看动态码。</p>

        {bannerCode === null ? null : (
          <ErrorBanner code={bannerCode} pushToast={pushToast} settings={settings} />
        )}

        <div className={`lumio-otp${shaking ? " is-error" : ""}`}>
          {digits.map((digit, index) => (
            <input
              aria-label={`第 ${index + 1} 位`}
              disabled={submitting}
              inputMode="numeric"
              key={index}
              maxLength={1}
              onChange={(event) => onChangeDigit(index, event.target.value)}
              onKeyDown={(event) => onKeyDownDigit(index, event)}
              onPaste={onPasteDigits}
              ref={(element) => {
                inputs.current[index] = element;
              }}
              type="text"
              value={digit}
            />
          ))}
        </div>

        <p className="lumio-auth-foot">
          <button className="lumio-link-button" onClick={onBackToPassword} type="button">
            返回重新登录
          </button>
        </p>
      </section>
    </div>
  );
}
