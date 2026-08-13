import { useState, type FormEvent } from "react";

import { cancelEmailChange, changePassword, confirmEmailChange, requestEmailChange } from "@/api/endpoints";
import { CodeInput } from "@/components/CodeInput";
import { PasswordField, TextField } from "@/components/fields";
import { useToast } from "@/components/Toast";
import { Banner, SectionCard, Spinner, errorMessage } from "@/components/ui";
import { useT } from "@/i18n";
import { emailValid, passwordValid } from "@/lib/validation";
import { useSession } from "@/state/session";

/**
 * 5.6「安全」：修改密码 + 两步式修改邮箱（新邮箱验证码 → 原子切换，流程中可取消）。
 * 五态：error（表单错误条）/ disabled（提交中禁用整个表单）/ loading（按钮 spinner）；
 * empty 与无权限不适用（本人数据）。
 */
export function SecuritySection() {
  const t = useT();
  const toast = useToast();
  const { user, patchUser } = useSession();

  const [currentPassword, setCurrentPassword] = useState("");
  const [newPassword, setNewPassword] = useState("");
  const [passwordBusy, setPasswordBusy] = useState(false);
  const [passwordError, setPasswordError] = useState("");

  const [newEmail, setNewEmail] = useState("");
  const [emailBusy, setEmailBusy] = useState(false);
  const [emailError, setEmailError] = useState("");
  const [pendingEmail, setPendingEmail] = useState("");
  const [codeErrorNonce, setCodeErrorNonce] = useState(0);

  async function submitPassword(event: FormEvent) {
    event.preventDefault();
    if (!currentPassword || !passwordValid(newPassword) || passwordBusy) return;

    setPasswordBusy(true);
    setPasswordError("");
    try {
      const result = await changePassword(currentPassword, newPassword);
      setCurrentPassword("");
      setNewPassword("");
      toast(result?.message ?? "密码已更新，其他设备已退出登录。");
    } catch (error) {
      setPasswordError(errorMessage(error, t("common.unknown_error")));
    } finally {
      setPasswordBusy(false);
    }
  }

  async function startEmailChange(event: FormEvent) {
    event.preventDefault();
    if (!emailValid(newEmail) || emailBusy) return;

    setEmailBusy(true);
    setEmailError("");
    try {
      await requestEmailChange(newEmail.trim());
      setPendingEmail(newEmail.trim());
    } catch (error) {
      setEmailError(errorMessage(error, t("common.unknown_error")));
    } finally {
      setEmailBusy(false);
    }
  }

  async function confirmEmail(code: string) {
    setEmailBusy(true);
    setEmailError("");
    try {
      patchUser(await confirmEmailChange(code));
      setPendingEmail("");
      setNewEmail("");
      toast(t("account.change_email_done"));
    } catch (error) {
      setCodeErrorNonce((nonce) => nonce + 1);
      setEmailError(errorMessage(error, t("common.unknown_error")));
    } finally {
      setEmailBusy(false);
    }
  }

  async function abortEmailChange() {
    setEmailBusy(true);
    try {
      await cancelEmailChange();
    } catch {
      // 取消失败不阻断：本地状态回到初始，后端的待验证码会自然过期。
    } finally {
      setPendingEmail("");
      setEmailError("");
      setEmailBusy(false);
    }
  }

  return (
    <SectionCard id="security" title={t("account.security")}>
      <form className="form-narrow" onSubmit={submitPassword} noValidate>
        <fieldset className="form-fieldset" disabled={passwordBusy}>
          {passwordError && <Banner kind="error">{passwordError}</Banner>}
          <TextField
            label={t("password.current")}
            type="password"
            autoComplete="current-password"
            value={currentPassword}
            onChange={(event) => setCurrentPassword(event.target.value)}
          />
          <PasswordField label={t("password.new")} value={newPassword} onChange={setNewPassword} />
          <button
            type="submit"
            className="btn btn-primary"
            disabled={!currentPassword || !passwordValid(newPassword) || passwordBusy}
          >
            {passwordBusy && <Spinner />}
            {passwordBusy ? t("account.change_password_busy") : t("account.change_password")}
          </button>
        </fieldset>
      </form>

      <hr className="section-divider" />

      <h4 className="subsection-title">{t("account.change_email")}</h4>
      {emailError && <Banner kind="error">{emailError}</Banner>}

      {pendingEmail ? (
        <div className="form-narrow">
          <p className="note" style={{ marginBottom: 10 }}>
            {t("account.change_email_sent", { email: pendingEmail })}
          </p>
          <CodeInput
            disabled={emailBusy}
            errorNonce={codeErrorNonce}
            onComplete={(code) => void confirmEmail(code)}
          />
          <button
            type="button"
            className="btn btn-ghost"
            style={{ marginTop: 12 }}
            onClick={() => void abortEmailChange()}
            disabled={emailBusy}
          >
            {t("account.change_email_cancel")}
          </button>
        </div>
      ) : (
        <form className="inline-form" onSubmit={startEmailChange} noValidate>
          <TextField
            label={t("account.change_email_new")}
            type="email"
            value={newEmail}
            placeholder={user?.email ?? "you@example.com"}
            onChange={(event) => setNewEmail(event.target.value)}
          />
          <button
            type="submit"
            className="btn btn-secondary profile-save"
            disabled={!emailValid(newEmail) || emailBusy}
          >
            {emailBusy && <Spinner dark />}
            {t("account.change_email_send")}
          </button>
        </form>
      )}
    </SectionCard>
  );
}
