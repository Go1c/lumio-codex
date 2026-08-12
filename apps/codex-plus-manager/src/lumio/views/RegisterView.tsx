import { useEffect, useRef, useState } from "react";

import { lumioErrorCopy, lumioErrorLabel } from "../errors.ts";
import {
  emailSuffixError,
  formatEmailSuffixHint,
  isValidEmail,
  passwordStrength,
  registerFormError,
  sanitizeVerifyCode,
} from "../forms.ts";
import type { RegisterFormInput } from "../forms.ts";
import { LumioCommandError, registerAccount, sendVerifyCode, shellLabels } from "../invoke.ts";
import type { LumioAccountSummary, LumioServiceSettings } from "../types.ts";
import type { ToastTone } from "./Toast.tsx";

const RESEND_SECONDS = 60;

const INVITATION_REQUIRED_CODE = "AUTH_INVITATION_CODE_REQUIRED";
const INVITATION_INVALID_CODE = "AUTH_INVITATION_CODE_INVALID";

/**
 * Client-side field codes from `registerFormError` deliberately live outside
 * LUMIO_ERROR_COPY (that table is limited to the six server error domains), so
 * they need their own copy before falling back to the shared mapping.
 */
const FIELD_ERROR_COPY: Record<string, string> = {
  EMAIL_FORMAT_INVALID: "请输入有效的邮箱地址",
  PASSWORD_TOO_SHORT: "密码至少 8 位",
  PASSWORD_MISMATCH: "两次输入不一致",
  AGREEMENTS_NOT_ACCEPTED: "请先阅读并勾选全部协议",
};

const STRENGTH_LABEL: Record<ReturnType<typeof passwordStrength>, string> = {
  weak: "弱",
  medium: "中",
  strong: "强",
};

const STRENGTH_LEVEL: Record<ReturnType<typeof passwordStrength>, number> = {
  weak: 1,
  medium: 2,
  strong: 3,
};

function fieldErrorCopy(code: string): string {
  return FIELD_ERROR_COPY[code] ?? lumioErrorCopy(code);
}

function errorCodeOf(error: unknown): string {
  return error instanceof LumioCommandError ? error.errorCode : "UNKNOWN";
}

interface RegisterViewProps {
  settings: LumioServiceSettings;
  onAuthenticated: (account: LumioAccountSummary) => void;
  onTwoFactorRequired: () => void;
  onBack: () => void;
  pushToast: (input: string, tone?: ToastTone) => void;
}

export function RegisterView({
  settings,
  onAuthenticated,
  onTwoFactorRequired,
  onBack,
  pushToast,
}: RegisterViewProps) {
  const [email, setEmail] = useState("");
  const [verifyCode, setVerifyCode] = useState("");
  const [invitationCode, setInvitationCode] = useState("");
  // 服务端没下发开关时，错误码是唯一能证明「这台服务端要邀请码」的信号（交互规格 §7）。
  const [invitationDemanded, setInvitationDemanded] = useState(false);
  const invitationField = useRef<HTMLInputElement>(null);
  const [password, setPassword] = useState("");
  const [confirmPassword, setConfirmPassword] = useState("");
  const [acceptedDocumentIds, setAcceptedDocumentIds] = useState<string[]>([]);
  const [expandedDocumentId, setExpandedDocumentId] = useState<string | null>(null);
  const [emailError, setEmailError] = useState<string | null>(null);
  const [confirmError, setConfirmError] = useState<string | null>(null);
  const [bannerCode, setBannerCode] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [countdown, setCountdown] = useState(0);
  const [sending, setSending] = useState(false);

  useEffect(() => {
    if (countdown <= 0) return;
    const timer = setTimeout(() => setCountdown((current) => current - 1), 1000);
    return () => clearTimeout(timer);
  }, [countdown]);

  const invitationVisible = settings.invitationCodeEnabled === true || invitationDemanded;

  useEffect(() => {
    if (!invitationVisible) return;
    if (bannerCode !== INVITATION_REQUIRED_CODE && bannerCode !== INVITATION_INVALID_CODE) return;
    invitationField.current?.focus();
  }, [bannerCode, invitationVisible]);

  if (!settings.registrationEnabled) {
    return (
      <div className="lumio-auth-wrap">
        <section className="lumio-auth-card">
          <p className="lumio-eyebrow">{shellLabels.createAccount}</p>
          <h1>注册暂未开放</h1>
          <p className="lumio-auth-lead">
            我们正在分批开放注册名额，请稍后再来。如你已有账户，可以直接登录。
          </p>
          <button className="lumio-button is-primary is-large is-block" onClick={onBack} type="button">
            返回登录
          </button>
        </section>
      </div>
    );
  }

  const input: RegisterFormInput = {
    email,
    verifyCode,
    password,
    confirmPassword,
    acceptedDocumentIds,
  };
  const formError = registerFormError(input, settings);
  const strength = passwordStrength(password);
  const suffixHint = formatEmailSuffixHint(settings.emailSuffixWhitelist);
  const canSendCode = isValidEmail(email) && countdown === 0 && !sending && !submitting;

  const validateEmail = () => {
    if (email.trim() === "") {
      setEmailError(null);
      return;
    }
    if (!isValidEmail(email)) {
      setEmailError(FIELD_ERROR_COPY.EMAIL_FORMAT_INVALID);
      return;
    }
    const suffix = emailSuffixError(email, settings.emailSuffixWhitelist);
    setEmailError(suffix === null ? null : lumioErrorCopy(suffix));
  };

  const toggleDocument = (id: string) => {
    setAcceptedDocumentIds((current) =>
      current.includes(id) ? current.filter((entry) => entry !== id) : [...current, id],
    );
  };

  const requestVerifyCode = () => {
    setSending(true);
    void sendVerifyCode(email.trim())
      .then((result) => {
        setCountdown(result.countdown > 0 ? result.countdown : RESEND_SECONDS);
        pushToast("验证码已发送，请查看邮箱", "success");
      })
      .catch((error: unknown) => pushToast(errorCodeOf(error)))
      .finally(() => setSending(false));
  };

  const submit = () => {
    if (formError !== null || submitting) return;
    setSubmitting(true);
    setBannerCode(null);
    void registerAccount({
      email: email.trim(),
      password,
      verifyCode,
      acceptedRevision: settings.agreementRevision,
      invitationCode: invitationCode.trim(),
    })
      .then((result) => {
        if (result.requiresTwoFactor) {
          onTwoFactorRequired();
          return;
        }
        if (result.account !== null) onAuthenticated(result.account);
      })
      .catch((error: unknown) => {
        const code = errorCodeOf(error);
        setBannerCode(code);
        if (code === INVITATION_REQUIRED_CODE) setInvitationDemanded(true);
        if (code === INVITATION_INVALID_CODE) setInvitationCode("");
      })
      .finally(() => setSubmitting(false));
  };

  return (
    <div className="lumio-auth-wrap">
      <section className="lumio-auth-card">
        <p className="lumio-eyebrow">{shellLabels.createAccount}</p>
        <h1>用邮箱开始</h1>
        <p className="lumio-auth-lead">
          验证邮箱后设置密码即可完成注册，随后会自动完成官方 Codex 的连接准备。
        </p>

        {bannerCode === null ? null : (
          <p className="lumio-banner" role="alert">
            {lumioErrorLabel(bannerCode)}
          </p>
        )}

        <form
          onSubmit={(event) => {
            event.preventDefault();
            submit();
          }}
        >
          <div className={`lumio-field${emailError === null ? "" : " has-error"}`}>
            <label htmlFor="lumio-register-email">邮箱</label>
            <input
              autoComplete="email"
              className="lumio-input"
              disabled={submitting}
              id="lumio-register-email"
              onBlur={validateEmail}
              onChange={(event) => {
                setEmail(event.target.value);
                setEmailError(null);
              }}
              placeholder="you@example.com"
              type="email"
              value={email}
            />
            {suffixHint === null ? null : <p className="lumio-field-hint">{suffixHint}</p>}
            {emailError === null ? null : <p className="lumio-field-error">{emailError}</p>}
          </div>

          {settings.emailVerifyEnabled ? (
            <div className="lumio-field">
              <label htmlFor="lumio-register-code">验证码</label>
              <div className="lumio-input-row">
                <input
                  className="lumio-input"
                  disabled={submitting}
                  id="lumio-register-code"
                  inputMode="numeric"
                  maxLength={6}
                  onChange={(event) => setVerifyCode(sanitizeVerifyCode(event.target.value))}
                  placeholder="6 位数字"
                  type="text"
                  value={verifyCode}
                />
                <button
                  className="lumio-button is-secondary"
                  disabled={!canSendCode}
                  onClick={requestVerifyCode}
                  type="button"
                >
                  {countdown > 0 ? `重新发送 (${countdown}s)` : "发送验证码"}
                </button>
              </div>
            </div>
          ) : null}

          {invitationVisible ? (
            <div className="lumio-field">
              <label htmlFor="lumio-register-invitation">邀请码</label>
              <input
                className="lumio-input"
                disabled={submitting}
                id="lumio-register-invitation"
                onChange={(event) => setInvitationCode(event.target.value)}
                placeholder="填写你收到的邀请码"
                ref={invitationField}
                type="text"
                value={invitationCode}
              />
              <p className="lumio-field-hint">这台服务端目前需要邀请码才能注册</p>
            </div>
          ) : null}

          <div className="lumio-field">
            <label htmlFor="lumio-register-password">设置密码</label>
            <input
              autoComplete="new-password"
              className="lumio-input"
              disabled={submitting}
              id="lumio-register-password"
              onChange={(event) => setPassword(event.target.value)}
              placeholder="至少 8 位"
              type="password"
              value={password}
            />
            <div className="lumio-strength" data-level={password === "" ? 0 : STRENGTH_LEVEL[strength]}>
              <span className="lumio-strength-bars">
                <i />
                <i />
                <i />
              </span>
              <small>{password === "" ? "密码强度" : STRENGTH_LABEL[strength]}</small>
            </div>
          </div>

          <div className={`lumio-field${confirmError === null ? "" : " has-error"}`}>
            <label htmlFor="lumio-register-confirm">确认密码</label>
            <input
              autoComplete="new-password"
              className="lumio-input"
              disabled={submitting}
              id="lumio-register-confirm"
              onBlur={() =>
                setConfirmError(
                  confirmPassword !== "" && confirmPassword !== password
                    ? FIELD_ERROR_COPY.PASSWORD_MISMATCH
                    : null,
                )
              }
              onChange={(event) => {
                setConfirmPassword(event.target.value);
                setConfirmError(null);
              }}
              placeholder="再输入一次"
              type="password"
              value={confirmPassword}
            />
            {confirmError === null ? null : <p className="lumio-field-error">{confirmError}</p>}
          </div>

          {settings.agreementEnabled ? (
            <div className="lumio-agreements">
              {settings.agreementDocuments.map((agreement) => (
                <div className="lumio-agree-row" key={agreement.id}>
                  <label>
                    <input
                      checked={acceptedDocumentIds.includes(agreement.id)}
                      disabled={submitting}
                      onChange={() => toggleDocument(agreement.id)}
                      type="checkbox"
                    />
                    <span>
                      我已阅读并同意
                      <button
                        className="lumio-link-button"
                        onClick={() =>
                          setExpandedDocumentId((current) =>
                            current === agreement.id ? null : agreement.id,
                          )
                        }
                        type="button"
                      >
                        {agreement.title}
                      </button>
                      <small className="lumio-agree-version">{settings.agreementRevision}</small>
                    </span>
                  </label>
                  {expandedDocumentId === agreement.id ? (
                    <p className="lumio-agree-body">{agreement.contentMd}</p>
                  ) : null}
                </div>
              ))}
            </div>
          ) : null}

          {formError === null || submitting ? null : (
            <p className="lumio-field-error lumio-form-error">{fieldErrorCopy(formError)}</p>
          )}

          <button
            className="lumio-button is-primary is-large is-block"
            disabled={formError !== null || submitting}
            type="submit"
          >
            {submitting ? "正在创建账户…" : shellLabels.createAccount}
          </button>
        </form>

        <p className="lumio-auth-foot">
          <button className="lumio-link-button" onClick={onBack} type="button">
            已有账户？去登录
          </button>
        </p>
      </section>
    </div>
  );
}
