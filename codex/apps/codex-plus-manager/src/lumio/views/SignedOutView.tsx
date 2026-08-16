import { lumioErrorLabel } from "../errors.ts";
import { shellLabels } from "../invoke.ts";
import type { LumioActionNotes, LumioActions } from "../state.ts";

interface SignedOutViewProps {
  actions: LumioActions;
  actionNotes: LumioActionNotes;
  errorCode: string | null;
  serviceAvailable: boolean;
  onSignIn: () => void;
  onCreateAccount: () => void;
  onOpenHelp: () => void;
}

export function SignedOutView({
  actions,
  actionNotes,
  errorCode,
  serviceAvailable,
  onSignIn,
  onCreateAccount,
  onOpenHelp,
}: SignedOutViewProps) {
  return (
    <div className="lumio-signed-out">
      {serviceAvailable ? null : (
        <p className="lumio-banner is-warning" role="status">
          {lumioErrorLabel(errorCode ?? "SERVICE_UNAVAILABLE")}
        </p>
      )}

      <section className="lumio-welcome">
        <span className="lumio-app-icon is-hero" aria-hidden="true">
          <img alt="" src="/lumio-icon.png" />
        </span>
        <h1>BestCodex</h1>
        <p>一个启动器。官方 Codex，以及跑在你自己服务器上的 Claude。</p>

        <div className="lumio-welcome-cta">
          <button
            className="lumio-button is-primary is-large is-block"
            disabled={!actions.canSignIn}
            onClick={onSignIn}
            type="button"
          >
            {shellLabels.signIn}
          </button>
          <button
            className="lumio-button is-secondary is-large is-block"
            disabled={!actions.canRegister}
            onClick={onCreateAccount}
            type="button"
          >
            {shellLabels.createAccount}
          </button>
        </div>

        {actions.canSignIn ? null : <small className="lumio-inline-note">{actionNotes.signIn}</small>}
        {actions.canRegister || actionNotes.register === null ? null : (
          <small className="lumio-inline-note">{actionNotes.register}</small>
        )}

        <p className="lumio-welcome-help">
          <button className="lumio-link-button" onClick={onOpenHelp} type="button">
            需要帮助？安装与登录说明
          </button>
        </p>
        <p className="lumio-welcome-foot">
          凭据只保存在这台电脑上、仅你本人可读，界面与日志不含明文。
        </p>
      </section>
    </div>
  );
}
