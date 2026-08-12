import { Home, RefreshCw, Settings } from "lucide-react";
import { useCallback, useEffect, useReducer, useRef, useState } from "react";

import {
  LumioCommandError,
  loadLumioBootstrap,
  loadPublicSettings,
  shellLabels,
  signOut,
} from "./lumio/invoke.ts";
import { initialLumioState, reduceLumioState } from "./lumio/state.ts";
import type { ProvisioningStepId } from "./lumio/state.ts";
import type { LumioAccountSummary, LumioCodexApp, LumioPhase } from "./lumio/types.ts";
import { HomeView } from "./lumio/views/HomeView.tsx";
import { LoginView } from "./lumio/views/LoginView.tsx";
import { ProvisioningView } from "./lumio/views/ProvisioningView.tsx";
import { RegisterView } from "./lumio/views/RegisterView.tsx";
import { RepairView } from "./lumio/views/RepairView.tsx";
import { SettingsView } from "./lumio/views/SettingsView.tsx";
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

export function LumioApp() {
  const [state, dispatch] = useReducer(reduceLumioState, undefined, initialLumioState);
  const [view, setView] = useState<View>("home");
  const { toasts, pushToast, dismiss } = useToasts();
  // Read by callbacks that must keep a stable identity across renders.
  const stateRef = useRef(state);
  stateRef.current = state;

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

  // Stable identities: ProvisioningView keys its run loop on these callbacks.
  const onStepStarted = useCallback(
    (step: ProvisioningStepId) => dispatch({ type: "provisioning-step-started", step }),
    [],
  );
  const onStepCompleted = useCallback(
    (step: ProvisioningStepId) => dispatch({ type: "provisioning-step-completed", step }),
    [],
  );
  const onStepFailed = useCallback(
    (step: ProvisioningStepId, errorCode: string) =>
      dispatch({ type: "provisioning-step-failed", step, errorCode }),
    [],
  );
  const onProvisioned = useCallback(() => {
    const current = stateRef.current;
    if (current.account === null) return;
    dispatch({
      type: "online-ready",
      account: current.account,
      cachedAt: new Date().toISOString(),
      codexApp: current.codexApp,
      defaultModel: current.defaultModel,
    });
  }, []);
  const onDeferred = useCallback(() => {
    void signOut()
      .catch(() => undefined)
      .finally(() => dispatch({ type: "signed-out" }));
  }, []);
  const onRefreshed = useCallback(
    (account: LumioAccountSummary, cachedAt: string) =>
      dispatch({ type: "account-refreshed", account, cachedAt }),
    [],
  );
  const onReconnected = useCallback((account: LumioAccountSummary, cachedAt: string) => {
    dispatch({
      type: "online-ready",
      account,
      cachedAt,
      codexApp: stateRef.current.codexApp,
      defaultModel: stateRef.current.defaultModel,
    });
  }, []);
  // A repaired takeover has no cached account until the surface reloads, so a
  // clean health check restarts provisioning instead of faking a ready home.
  const onRepaired = useCallback(() => {
    const current = stateRef.current;
    if (current.account === null) {
      dispatch({ type: "signed-out" });
      return;
    }
    dispatch({ type: "authenticated", account: current.account });
  }, []);
  const onCodexAppChanged = useCallback((app: LumioCodexApp) => {
    const current = stateRef.current;
    if (current.account === null || current.phase !== "ready-online") return;
    dispatch({
      type: "online-ready",
      account: current.account,
      cachedAt: current.cachedAt ?? new Date().toISOString(),
      codexApp: app,
      defaultModel: current.defaultModel,
    });
  }, []);

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
          <RepairView
            errorCode={state.errorCode}
            onResolved={onRepaired}
            onSignOut={onDeferred}
            pushToast={pushToast}
          />
        ) : view === "settings" ? (
          <SettingsView
            autoUpdateEnabled={state.autoUpdateEnabled}
            codexApp={state.codexApp}
            onCodexAppChanged={onCodexAppChanged}
            pushToast={pushToast}
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
        ) : state.phase === "provisioning" ? (
          <ProvisioningView
            email={state.account?.email ?? null}
            onCompleted={onProvisioned}
            onDeferred={onDeferred}
            onStepCompleted={onStepCompleted}
            onStepFailed={onStepFailed}
            onStepStarted={onStepStarted}
            provisioning={state.provisioning}
          />
        ) : state.phase === "authenticating" ? (
          <section aria-live="polite" className="lumio-loading">
            <span className="lumio-loading-mark">
              <RefreshCw size={24} />
            </span>
            <p>{phaseCopy[state.phase]}…</p>
          </section>
        ) : (
          <HomeView
            onOpenSettings={openSettings}
            onReconnected={onReconnected}
            onRefreshed={onRefreshed}
            pushToast={pushToast}
            state={state}
          />
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
