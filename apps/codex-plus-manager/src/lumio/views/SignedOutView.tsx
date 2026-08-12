import { Activity, Check, ChevronRight, CircleUserRound, Laptop, LockKeyhole, LogIn, UserPlus } from "lucide-react";

import { lumioErrorLabel } from "../errors.ts";
import { shellLabels } from "../invoke.ts";
import type { LumioActionNotes, LumioActions } from "../state.ts";
import type { LumioCodexApp } from "../types.ts";

const PROMISES = [
  { title: "不修改官方应用", detail: "官方 Codex 保持原样，本工具不替代、不注入。" },
  { title: "配置可一键恢复", detail: "写入前先备份，随时恢复到接管前的状态。" },
  { title: "凭据由系统保护", detail: "保存在钥匙串 / 凭据管理器，界面与日志不含明文。" },
];

interface SignedOutViewProps {
  actions: LumioActions;
  actionNotes: LumioActionNotes;
  codexApp: LumioCodexApp | null;
  errorCode: string | null;
  serviceAvailable: boolean;
  onSignIn: () => void;
  onCreateAccount: () => void;
  onOpenSettings: () => void;
}

export function SignedOutView({
  actions,
  actionNotes,
  codexApp,
  errorCode,
  serviceAvailable,
  onSignIn,
  onCreateAccount,
  onOpenSettings,
}: SignedOutViewProps) {
  return (
    <div className="lumio-signed-out">
      {serviceAvailable ? null : (
        <p className="lumio-banner is-warning" role="status">
          {lumioErrorLabel(errorCode ?? "SERVICE_UNAVAILABLE")}
        </p>
      )}

      <section className="lumio-hero">
        <div className="lumio-hero-copy">
          <span className="lumio-kicker">官方 Codex 快速接入工具</span>
          <h1>更快开始使用官方 Codex。</h1>
          <p>这个小工具只做一件事：帮你完成注册、登录和本机配置，省去手动安装配置的步骤。之后你使用的始终是官方 Codex 应用，一切保持原生。</p>

          <div className="lumio-hero-cta">
            <button
              className="lumio-button is-primary is-large"
              disabled={!actions.canSignIn}
              onClick={onSignIn}
              type="button"
            >
              <LogIn size={17} />
              {shellLabels.signIn}
            </button>
            <button
              className="lumio-button is-secondary is-large"
              disabled={!actions.canRegister}
              onClick={onCreateAccount}
              type="button"
            >
              <UserPlus size={17} />
              {shellLabels.createAccount}
            </button>
          </div>

          {actions.canSignIn ? null : <small className="lumio-inline-note">{actionNotes.signIn}</small>}
          {actions.canRegister || actionNotes.register === null ? null : (
            <small className="lumio-inline-note">{actionNotes.register}</small>
          )}

          <small className="lumio-inline-note">
            <LockKeyhole size={14} />
            不需要手动填写底层连接信息
          </small>
        </div>

        <aside className="lumio-quiet-card">
          <p className="lumio-quiet-title">我们的承诺</p>
          {PROMISES.map((promise) => (
            <div className="lumio-quiet-row" key={promise.title}>
              <span className="lumio-quiet-tick">
                <Check size={13} />
              </span>
              <span>
                <strong>{promise.title}</strong>
                <small>{promise.detail}</small>
              </span>
            </div>
          ))}
        </aside>
      </section>

      <section className="lumio-status-strip">
        <article>
          <span className="lumio-status-icon">
            <CircleUserRound size={19} />
          </span>
          <div>
            <small>{shellLabels.accountStatus}</small>
            <strong>尚未登录</strong>
          </div>
        </article>
        <button className="lumio-status-action" onClick={onOpenSettings} type="button">
          <span className={`lumio-status-icon${codexApp === null ? "" : " is-success"}`}>
            <Laptop size={19} />
          </span>
          <div>
            <small>官方应用</small>
            <strong>{codexApp === null ? "等待手动选择" : "检测成功"}</strong>
          </div>
          {codexApp === null ? <ChevronRight size={17} /> : <Check size={17} />}
        </button>
        <article>
          <span className={`lumio-status-icon${serviceAvailable ? " is-success" : " is-warning"}`}>
            <Activity size={19} />
          </span>
          <div>
            <small>服务入口</small>
            <strong>{serviceAvailable ? "连接正常" : "连接失败，30 秒后重试"}</strong>
          </div>
        </article>
      </section>
    </div>
  );
}
