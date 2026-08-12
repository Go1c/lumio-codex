import { Home, RefreshCw, Settings } from "lucide-react";
import { useCallback, useEffect, useReducer, useRef, useState } from "react";

import { lumioErrorLabel } from "./lumio/errors.ts";
import {
  LumioCommandError,
  SESSION_EXPIRED_ERROR_CODE,
  checkTakeover,
  loadLumioBootstrap,
  loadPublicSettings,
  onSessionExpired,
  shellLabels,
  signOut,
} from "./lumio/invoke.ts";
import { initialLumioState, reduceLumioState } from "./lumio/state.ts";
import type { LumioEvent, ProvisioningStepId } from "./lumio/state.ts";
import type {
  LumioAccountSummary,
  LumioBootstrap,
  LumioCodexApp,
  LumioPhase,
} from "./lumio/types.ts";
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

const CONFIG_CONFLICT_CODE = "CODEX_CONFIG_CONFLICT";

function errorCodeOf(error: unknown): string {
  return error instanceof LumioCommandError ? error.errorCode : "UNKNOWN";
}

/**
 * 本机凭据是否可用于免登录续跑。命令层还没带 `credentialStatus` 的旧 payload 只能靠
 * 缓存账户判断，带了就以它为准——凭据失效时不该把用户当作已登录继续推进。
 */
function hasLocalCredentials(bootstrap: LumioBootstrap): boolean {
  const status = bootstrap.credentialStatus;
  return bootstrap.account !== null && (status === undefined || status === "present");
}

/**
 * 启动编排：bootstrap 本身回答不了「下一站是哪」。它只说本机有没有凭据，
 * 而 provisioning 会写本机配置，所以必须先把两件事定下来再决定阶段：
 *
 * 1. 本机接管记录是否与 `~/.codex` 现状一致——冲突就进修复页，绝不让 `write-config`
 *    有机会把用户在别处改过的字段静默盖回去。
 * 2. 服务是否可达——不可达但本机凭据与接管都在，就进离线首页，让用户仍能启动官方 Codex。
 *
 * 返回一串事件而不是边算边 dispatch：中间阶段一旦上屏，ProvisioningView 会立刻开跑，
 * 而我们此刻已经知道它不该跑。
 */
async function planStartup(): Promise<LumioEvent[]> {
  let bootstrap: LumioBootstrap;
  try {
    bootstrap = await loadLumioBootstrap();
  } catch (error: unknown) {
    return [{ type: "repair-required", errorCode: errorCodeOf(error) }];
  }

  const booted: LumioEvent = { type: "bootstrapped", payload: bootstrap };
  if (!hasLocalCredentials(bootstrap)) return [booted];

  // 检测本身失败（拿不到状态目录之类）不等于配置冲突，按未接管处理，交给 provisioning 如实报错。
  const health = await checkTakeover().catch(() => null);
  if (health !== null && health.health === "conflicted") {
    return [booted, { type: "repair-required", errorCode: health.errorCode ?? CONFIG_CONFLICT_CODE }];
  }

  try {
    const settings = await loadPublicSettings();
    return [booted, { type: "service-settings-loaded", settings }];
  } catch (error: unknown) {
    const unavailable: LumioEvent = { type: "service-unavailable", errorCode: errorCodeOf(error) };
    // 没接管过就没有可用的本机配置可离线启动；此时如实走 provisioning 失败态，
    // 而不是端出一个「本机就绪」的假象。
    if (health === null || health.health !== "healthy") return [booted, unavailable];
    // 这台机器上最近一次成功同步的时间没有任何命令能提供，就让它保持未知。
    return [booted, unavailable, { type: "offline-ready", cachedAt: null }];
  }
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
    void planStartup().then((events) => {
      // 一次性刷出：编排算出来的中间阶段不该被渲染出来。
      if (!active) return;
      for (const event of events) dispatch(event);
    });
    return () => {
      active = false;
    };
  }, []);

  // The public settings gate every entry-point button, so an unreachable
  // service must keep retrying instead of leaving the surface permanently dark.
  const startupPending = state.phase === "bootstrapping";
  useEffect(() => {
    if (state.serviceAvailable || startupPending) return;
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
    // 启动编排在 bootstrapping 阶段自己探活一次，这里同时轮询会和它的判定赛跑。
  }, [startupPending, state.serviceAvailable]);

  // 过期的会话在哪个命令上暴露都一样：提示一次，回到登录入口，绝不继续展示陈旧数据。
  useEffect(() => {
    onSessionExpired(() => {
      pushToast(lumioErrorLabel(SESSION_EXPIRED_ERROR_CODE));
      dispatch({ type: "session-expired", errorCode: SESSION_EXPIRED_ERROR_CODE });
      // 服务不可达时登录表单没有可用的服务端规则，停在未登录首页由它解释原因。
      if (stateRef.current.service !== null && stateRef.current.serviceAvailable) {
        dispatch({ type: "auth-step-changed", step: "login" });
      }
      setView("home");
    });
    return () => onSessionExpired(null);
  }, [pushToast]);

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
  const onSignOutRequested = useCallback(() => {
    setView("home");
    onDeferred();
  }, [onDeferred]);
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
            onSignOut={onSignOutRequested}
            pushToast={pushToast}
            signedIn={state.account !== null}
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
