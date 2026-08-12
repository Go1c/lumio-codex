import {
  Activity,
  Download,
  FileArchive,
  Home,
  Laptop,
  RefreshCw,
  Rocket,
  RotateCcw,
  Settings,
  ShieldCheck,
  Sparkles,
  WalletCards,
} from "lucide-react";
import { useCallback, useEffect, useReducer, useState } from "react";

import { lumioErrorLabel } from "./lumio/errors.ts";
import { LumioCommandError, loadLumioBootstrap, loadPublicSettings, shellLabels } from "./lumio/invoke.ts";
import { initialLumioState, reduceLumioState } from "./lumio/state.ts";
import type { LumioState } from "./lumio/state.ts";
import type { LumioAccountSummary, LumioCodexApp, LumioPhase } from "./lumio/types.ts";
import { LoginView } from "./lumio/views/LoginView.tsx";
import { RegisterView } from "./lumio/views/RegisterView.tsx";
import { SignedOutView } from "./lumio/views/SignedOutView.tsx";
import { ToastHost, useToasts } from "./lumio/views/Toast.tsx";

type View = "home" | "settings";

const SERVICE_RETRY_MS = 30_000;

const phaseCopy: Record<LumioPhase, string> = {
  bootstrapping: "正在检查本机环境",
  "signed-out": "等待登录",
  authenticating: "正在验证账户",
  provisioning: "正在准备连接",
  "ready-online": "服务连接正常",
  "ready-offline": "使用本机缓存",
  "needs-repair": "需要检查配置",
};

function errorCodeOf(error: unknown): string {
  return error instanceof LumioCommandError ? error.errorCode : "UNKNOWN";
}

function formatBalance(balance: number): string {
  return new Intl.NumberFormat("zh-CN", {
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  }).format(balance);
}

function Toggle({ checked, label }: { checked: boolean; label: string }) {
  return (
    <button
      aria-checked={checked}
      aria-label={label}
      className={`lumio-toggle${checked ? " is-on" : ""}`}
      disabled
      role="switch"
      type="button"
    >
      <span />
    </button>
  );
}

export function LumioApp() {
  const [state, dispatch] = useReducer(reduceLumioState, undefined, initialLumioState);
  const [view, setView] = useState<View>("home");
  const { toasts, pushToast, dismiss } = useToasts();

  useEffect(() => {
    let active = true;
    void loadLumioBootstrap()
      .then((payload) => {
        if (active) dispatch({ type: "bootstrapped", payload });
      })
      .catch((error: unknown) => {
        if (active) dispatch({ type: "repair-required", errorCode: errorCodeOf(error) });
      });
    return () => {
      active = false;
    };
  }, []);

  // The public settings gate every entry-point button, so an unreachable
  // service must keep retrying instead of leaving the surface permanently dark.
  useEffect(() => {
    if (state.serviceAvailable) return;
    let active = true;

    const load = () => {
      void loadPublicSettings()
        .then((settings) => {
          if (active) dispatch({ type: "service-settings-loaded", settings });
        })
        .catch((error: unknown) => {
          if (active) dispatch({ type: "service-unavailable", errorCode: errorCodeOf(error) });
        });
    };

    load();
    const timer = setInterval(load, SERVICE_RETRY_MS);
    return () => {
      active = false;
      clearInterval(timer);
    };
  }, [state.serviceAvailable]);

  const openSettings = useCallback(() => setView("settings"), []);
  const online = state.phase === "ready-online";
  const offline = state.phase === "ready-offline";
  const ready = online || offline;
  // Stage pages own the whole main area: leaving them mid-flight would strand
  // the account in a half-configured state (interaction spec §4).
  const navLocked = !ready && state.phase !== "signed-out";

  return (
    <div className="lumio-app">
      <header className="lumio-topbar">
        <span className="lumio-brand">
          <span className="lumio-logo-wrap">
            <img alt="" className="lumio-logo" src="/lumio-icon.png" />
          </span>
          <span>
            <strong>Lumio Codex</strong>
            <small>DESKTOP</small>
          </span>
        </span>

        <nav aria-label="主导航" className={`lumio-nav${navLocked ? " is-locked" : ""}`}>
          <button
            aria-current={view === "home" ? "page" : undefined}
            className={view === "home" ? "is-active" : ""}
            disabled={navLocked}
            onClick={() => setView("home")}
            type="button"
          >
            <Home size={16} />
            {shellLabels.home}
          </button>
          <button
            aria-current={view === "settings" ? "page" : undefined}
            className={view === "settings" ? "is-active" : ""}
            disabled={navLocked}
            onClick={() => setView("settings")}
            type="button"
          >
            <Settings size={16} />
            {shellLabels.settings}
          </button>
        </nav>

        <div className={`lumio-phase${online ? " is-online" : offline ? " is-offline" : ""}`}>
          <span />
          {phaseCopy[state.phase]}
        </div>
      </header>

      <main className="lumio-main">
        {state.phase === "bootstrapping" ? (
          <section aria-live="polite" className="lumio-loading">
            <span className="lumio-loading-mark">
              <RefreshCw size={24} />
            </span>
            <p>正在检测官方应用并读取本机状态…</p>
          </section>
        ) : state.phase === "needs-repair" ? (
          <BootstrapFailurePanel errorCode={state.errorCode} />
        ) : view === "settings" ? (
          <SettingsView
            autoUpdateEnabled={state.autoUpdateEnabled}
            codexApp={state.codexApp}
            telemetryEnabled={state.telemetryEnabled}
          />
        ) : state.phase === "signed-out" ? (
          <SignedOutView
            actionNotes={state.actionNotes}
            actions={state.actions}
            codexApp={state.codexApp}
            errorCode={state.errorCode}
            onCreateAccount={() => dispatch({ type: "auth-step-changed", step: "register" })}
            onOpenSettings={openSettings}
            onSignIn={() => dispatch({ type: "auth-step-changed", step: "login" })}
            serviceAvailable={state.serviceAvailable}
          />
        ) : state.phase === "authenticating" && state.service !== null ? (
          state.authStep === "register" ? (
            <RegisterView
              onAuthenticated={(account) => dispatch({ type: "authenticated", account })}
              onBack={() => dispatch({ type: "auth-step-changed", step: "login" })}
              onTwoFactorRequired={() => dispatch({ type: "two-factor-required" })}
              pushToast={pushToast}
              settings={state.service}
            />
          ) : (
            <LoginView
              onAuthenticated={(account) => dispatch({ type: "authenticated", account })}
              onBackToPassword={() => dispatch({ type: "auth-step-changed", step: "login" })}
              onCreateAccount={() => dispatch({ type: "auth-step-changed", step: "register" })}
              onTwoFactorRequired={() => dispatch({ type: "two-factor-required" })}
              pushToast={pushToast}
              settings={state.service}
              step={state.authStep === "two-factor" ? "two-factor" : "login"}
            />
          )
        ) : state.phase === "authenticating" || state.phase === "provisioning" ? (
          <section aria-live="polite" className="lumio-loading">
            <span className="lumio-loading-mark">
              <RefreshCw size={24} />
            </span>
            <p>{phaseCopy[state.phase]}…</p>
          </section>
        ) : (
          <HomeView account={state.account} codexApp={state.codexApp} phase={state.phase} state={state} />
        )}
      </main>

      <footer className="lumio-footer">
        <span>内部测试渠道</span>
        <span className="lumio-footer-separator" />
        <span>官方应用需单独安装</span>
        {state.bootstrap ? <span className="lumio-footer-version">v{state.bootstrap.version}</span> : null}
      </footer>

      <ToastHost onDismiss={dismiss} toasts={toasts} />
    </div>
  );
}

function BootstrapFailurePanel({ errorCode }: { errorCode: string | null }) {
  return (
    <section className="lumio-repair-panel">
      <span className="lumio-panel-icon is-warning">
        <ShieldCheck size={24} />
      </span>
      <div>
        <p className="lumio-eyebrow">启动检查未完成</p>
        <h1>需要检查配置</h1>
        <p>本机配置尚未被修改。</p>
        <code>{lumioErrorLabel(errorCode)}</code>
      </div>
    </section>
  );
}

interface HomeViewProps {
  account: LumioAccountSummary | null;
  codexApp: LumioCodexApp | null;
  phase: LumioPhase;
  state: LumioState;
}

function HomeView({ account, codexApp, phase, state }: HomeViewProps) {
  if (account === null) {
    return (
      <section aria-live="polite" className="lumio-loading">
        <p>正在读取账户信息…</p>
      </section>
    );
  }

  return (
    <div className="lumio-dashboard">
      <section className="lumio-welcome-row">
        <div>
          <p className="lumio-eyebrow">{shellLabels.accountStatus} · 欢迎回来</p>
          <h1>你的 Lumio 连接中心</h1>
          <p>{account.email}</p>
        </div>
        <span className="lumio-secure-chip">
          <ShieldCheck size={16} />
          凭据由系统保护
        </span>
      </section>

      <div className="lumio-metric-grid">
        <article className="lumio-card lumio-balance-card">
          <span className="lumio-card-icon">
            <WalletCards size={20} />
          </span>
          <p>{shellLabels.balanceAndPlan}</p>
          <strong>{formatBalance(account.balance)}</strong>
          <small>{account.planLabel ?? "当前没有生效套餐"}</small>
        </article>
        <article className="lumio-card">
          <span className="lumio-card-icon">
            <Activity size={20} />
          </span>
          <p>{shellLabels.connectionStatus}</p>
          <strong>{phase === "ready-online" ? "在线" : "本机就绪"}</strong>
          <small>{phaseCopy[phase]}</small>
        </article>
        <article className="lumio-card">
          <span className="lumio-card-icon">
            <Sparkles size={20} />
          </span>
          <p>{shellLabels.defaultModel}</p>
          <strong>{state.defaultModel ?? "等待服务端同步"}</strong>
          <small>模型由服务端管理</small>
        </article>
      </div>

      <section className="lumio-action-panel">
        <div>
          <p className="lumio-eyebrow">官方 Codex</p>
          <h2>{codexApp ? "已检测到官方应用" : "尚未检测到官方应用"}</h2>
          <p>{codexApp?.version ? `版本 ${codexApp.version}` : "可在设置中查看检测状态"}</p>
        </div>
        <div className="lumio-actions">
          <button className="lumio-button is-secondary" disabled={!state.actions.canPay} type="button">
            <WalletCards size={17} />
            {shellLabels.payment}
          </button>
          <button className="lumio-button is-primary" disabled={!state.actions.canLaunch} type="button">
            <Rocket size={17} />
            {shellLabels.launch}
          </button>
        </div>
      </section>
    </div>
  );
}

interface SettingsViewProps {
  autoUpdateEnabled: boolean;
  codexApp: LumioCodexApp | null;
  telemetryEnabled: boolean;
}

function SettingsView({ autoUpdateEnabled, codexApp, telemetryEnabled }: SettingsViewProps) {
  return (
    <section className="lumio-settings-page">
      <div className="lumio-page-heading">
        <p className="lumio-eyebrow">桌面偏好</p>
        <h1>{shellLabels.settings}</h1>
        <p>这里只保留 Lumio Codex 运行所需的本机选项。</p>
      </div>

      <div className="lumio-settings-list">
        <article className="lumio-setting-row">
          <span className="lumio-setting-icon">
            <Rocket size={19} />
          </span>
          <div>
            <strong>{shellLabels.launchAtLogin}</strong>
            <p>登录电脑后自动准备 Lumio Codex</p>
          </div>
          <Toggle checked={false} label={shellLabels.launchAtLogin} />
        </article>

        <article className="lumio-setting-row">
          <span className="lumio-setting-icon">
            <Download size={19} />
          </span>
          <div>
            <strong>{shellLabels.automaticUpdates}</strong>
            <p>仅安装经过校验且适用于当前平台的版本</p>
          </div>
          <Toggle checked={autoUpdateEnabled} label={shellLabels.automaticUpdates} />
        </article>

        <article className="lumio-setting-row is-path-row">
          <span className="lumio-setting-icon">
            <Laptop size={19} />
          </span>
          <div>
            <strong>{shellLabels.officialAppPath}</strong>
            <p className="lumio-path-value">{codexApp?.path ?? "未自动检测到，可在功能接入后手动选择"}</p>
          </div>
          <button className="lumio-small-button" disabled type="button">
            <RefreshCw size={15} />
            {shellLabels.recheck}
          </button>
        </article>

        <article className="lumio-setting-row">
          <span className="lumio-setting-icon">
            <Activity size={19} />
          </span>
          <div>
            <strong>{shellLabels.telemetry}</strong>
            <p>默认关闭；开启后也只发送版本、平台、阶段和脱敏错误码</p>
          </div>
          <Toggle checked={telemetryEnabled} label={shellLabels.telemetry} />
        </article>

        <article className="lumio-setting-row">
          <span className="lumio-setting-icon">
            <FileArchive size={19} />
          </span>
          <div>
            <strong>{shellLabels.exportLogs}</strong>
            <p>导出前会再次扫描并移除敏感内容</p>
          </div>
          <button className="lumio-small-button" disabled type="button">
            导出
          </button>
        </article>

        <article className="lumio-setting-row">
          <span className="lumio-setting-icon is-warning">
            <RotateCcw size={19} />
          </span>
          <div>
            <strong>{shellLabels.restoreConfiguration}</strong>
            <p>撤销 Lumio 管理的字段并保留其他本机设置</p>
          </div>
          <button className="lumio-small-button is-warning" disabled type="button">
            恢复
          </button>
        </article>
      </div>

      <p className="lumio-settings-note">
        <ShieldCheck size={15} />
        不可用的选项会保持禁用，不会修改本机配置。
      </p>
    </section>
  );
}
