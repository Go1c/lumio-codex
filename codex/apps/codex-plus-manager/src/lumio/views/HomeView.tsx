import {
  Activity,
  CheckCircle2,
  CloudOff,
  Download,
  RefreshCw,
  Rocket,
  ShieldCheck,
  Star,
  WalletCards,
} from "lucide-react";
import { useEffect, useState } from "react";

import {
  LumioCommandError,
  detectCodexApp,
  installOfficialApp,
  launchCodex,
  officialAppStatus,
  openInBrowser,
  refreshAccount,
  shellLabels,
} from "../invoke.ts";
import type { LumioOfficialAppInstallStatus } from "../invoke.ts";
import { paymentUrl } from "../payment.ts";
import { isOfficialAppInstallInProgress, type LumioState } from "../state.ts";
import type {
  LumioAccountSummary,
  LumioCodexApp,
  LumioOfficialAppInstall,
  LumioOfficialAppInstallPhase,
  LumioUpdateReminder,
} from "../types.ts";
import { LUMIO_OFFICIAL_APP_INSTALL_PHASES } from "../types.ts";
import type { ToastTone } from "./Toast.tsx";

const RECONNECT_PROBE_MS = 30_000;
const RECONNECT_BANNER_MS = 3000;
const PAYMENT_POLL_MS = 10_000;
const INSTALL_POLL_MS = 400;
const INSTALL_AND_LAUNCH_COPY = "安装并启动官方 Codex";
const INSTALLING_COPY = "正在安装官方 Codex…";
const OFFLINE_NO_APP_NOTE = "安装官方应用需要网络";
const NO_APP_SUBCOPY = "将为你安装官方 Codex";

function errorCodeOf(error: unknown): string {
  return error instanceof LumioCommandError ? error.errorCode : "UNKNOWN";
}

function formatBalance(balance: number): string {
  return new Intl.NumberFormat("zh-CN", {
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  }).format(balance);
}

function asInstallPhase(phase: string): LumioOfficialAppInstallPhase {
  return (LUMIO_OFFICIAL_APP_INSTALL_PHASES as readonly string[]).includes(phase)
    ? (phase as LumioOfficialAppInstallPhase)
    : "failed";
}

function toInstallProgress(status: LumioOfficialAppInstallStatus): LumioOfficialAppInstall {
  return {
    phase: asInstallPhase(status.phase),
    stage: status.stage,
    errorCode: status.errorCode,
    path: status.installedPath,
  };
}

function installStageLabel(stage: string | null, phase: string): string | null {
  const key = (stage ?? phase).toLowerCase();
  if (key.includes("download")) return "下载";
  if (key.includes("verify")) return "校验";
  if (key.includes("install") || key.includes("detect") || key.includes("plan")) return "安装";
  return null;
}

function wait(ms: number): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, ms);
  });
}

function formatSyncTime(iso: string | null): string {
  if (iso === null) return "未知时间";
  const parsed = new Date(iso);
  if (Number.isNaN(parsed.getTime())) return "未知时间";
  return new Intl.DateTimeFormat("zh-CN", { hour: "2-digit", minute: "2-digit" }).format(parsed);
}

interface HomeViewProps {
  state: LumioState;
  updateReminder: LumioUpdateReminder | null;
  onRefreshed: (account: LumioAccountSummary, cachedAt: string) => void;
  onReconnected: (account: LumioAccountSummary, cachedAt: string) => void;
  onCodexAppChanged: (app: LumioCodexApp) => void;
  onInstallProgress: (status: LumioOfficialAppInstall) => void;
  onOpenSettings: () => void;
  onDismissUpdate: () => void;
  pushToast: (input: string, tone?: ToastTone) => void;
}

export function HomeView({
  state,
  updateReminder,
  onRefreshed,
  onReconnected,
  onCodexAppChanged,
  onInstallProgress,
  onOpenSettings,
  onDismissUpdate,
  pushToast,
}: HomeViewProps) {
  const { account, actionNotes, actions, codexApp, officialAppInstall } = state;
  const offline = state.phase === "ready-offline";
  // 没有同步时间就没有同步过：账户数值此刻是启动时的占位而非缓存下来的真值，不能当余额渲染。
  const syncTimeUnknown = state.cachedAt === null;
  const [refreshing, setRefreshing] = useState(false);
  const [launching, setLaunching] = useState(false);
  const [paying, setPaying] = useState(false);
  const [paymentOpen, setPaymentOpen] = useState(false);
  const [paymentBalanceUpdated, setPaymentBalanceUpdated] = useState(false);
  const [reconnected, setReconnected] = useState(false);
  const openingBalance = account?.balance ?? null;

  useEffect(() => {
    if (!offline) return;
    let active = true;
    const probe = () => {
      void refreshAccount()
        .then((fresh) => {
          if (!active) return;
          setReconnected(true);
          onReconnected(fresh, new Date().toISOString());
        })
        .catch(() => undefined);
    };
    const timer = setInterval(probe, RECONNECT_PROBE_MS);
    return () => {
      active = false;
      clearInterval(timer);
    };
  }, [offline, onReconnected]);

  useEffect(() => {
    if (!reconnected) return;
    const timer = setTimeout(() => setReconnected(false), RECONNECT_BANNER_MS);
    return () => clearTimeout(timer);
  }, [reconnected]);

  useEffect(() => {
    if (!paymentOpen || paymentBalanceUpdated || openingBalance === null) return;
    let active = true;
    const poll = () => {
      void refreshAccount()
        .then((fresh) => {
          if (!active) return;
          onRefreshed(fresh, new Date().toISOString());
          if (fresh.balance !== openingBalance) {
            setPaymentBalanceUpdated(true);
          }
        })
        .catch(() => undefined);
    };
    const timer = setInterval(poll, PAYMENT_POLL_MS);
    return () => {
      active = false;
      clearInterval(timer);
    };
  }, [paymentOpen, paymentBalanceUpdated, openingBalance, onRefreshed]);

  useEffect(() => {
    if (!paymentBalanceUpdated) return;
    const timer = setTimeout(() => {
      setPaymentOpen(false);
      setPaymentBalanceUpdated(false);
    }, 2000);
    return () => clearTimeout(timer);
  }, [paymentBalanceUpdated]);

  if (!account) {
    return (
      <section aria-live="polite" className="lumio-loading">
        <p>正在读取账户信息…</p>
      </section>
    );
  }

  const refresh = () => {
    setRefreshing(true);
    void refreshAccount()
      .then((fresh) => onRefreshed(fresh, new Date().toISOString()))
      .catch(() => pushToast("刷新失败，仍显示上次数据"))
      .finally(() => setRefreshing(false));
  };

  const launch = () => {
    setLaunching(true);
    void launchCodex()
      .then(() => pushToast("官方 Codex 已启动", "success"))
      .catch((error: unknown) => pushToast(errorCodeOf(error)))
      .finally(() => setLaunching(false));
  };

  const adoptInstalledApp = async (installedPath: string | null): Promise<LumioCodexApp | null> => {
    const detected = await detectCodexApp();
    if (detected) return detected;
    if (installedPath) {
      return { path: installedPath, version: null, source: "automatic" };
    }
    return null;
  };

  const finishSuccessfulInstall = async (installedPath: string | null) => {
    const app = await adoptInstalledApp(installedPath);
    if (app === null) {
      pushToast("CODEX_APP_NOT_FOUND");
      return;
    }
    onCodexAppChanged(app);
    await launchCodex();
    pushToast("官方 Codex 已启动", "success");
  };

  const applyInstallStatus = async (status: LumioOfficialAppInstallStatus): Promise<boolean> => {
    onInstallProgress(toInstallProgress(status));
    const alreadyInstalled = status.started === false && Boolean(status.installedPath);
    if (status.phase === "succeeded" || alreadyInstalled) {
      await finishSuccessfulInstall(status.installedPath);
      return true;
    }
    if (status.phase === "failed" || status.phase === "cancelled") {
      pushToast(status.errorCode ?? "CODEX_APP_INSTALL_FAILED");
      return true;
    }
    return false;
  };

  const installThenLaunch = () => {
    setLaunching(true);
    void (async () => {
      try {
        const started = await installOfficialApp();
        if (await applyInstallStatus(started)) return;
        for (;;) {
          await wait(INSTALL_POLL_MS);
          const status = await officialAppStatus();
          if (await applyInstallStatus(status)) return;
        }
      } catch (error: unknown) {
        const code = errorCodeOf(error);
        onInstallProgress({ phase: "failed", stage: null, errorCode: code });
        pushToast(code);
      } finally {
        setLaunching(false);
      }
    })();
  };

  const onPrimaryClick = () => {
    if (codexApp) {
      launch();
      return;
    }
    installThenLaunch();
  };

  const primaryDisabled = !actions.canLaunch || launching || isOfficialAppInstallInProgress(officialAppInstall);
  const stageLabel = installStageLabel(officialAppInstall.stage, officialAppInstall.phase);
  const primaryLabel = isOfficialAppInstallInProgress(officialAppInstall)
    ? `${INSTALLING_COPY}${stageLabel ? ` ${stageLabel}` : ""}`
    : launching
      ? "正在启动…"
      : codexApp
        ? shellLabels.launch
        : INSTALL_AND_LAUNCH_COPY;

  const openPayment = () => {
    const url = paymentUrl(state);
    if (url === null) {
      pushToast("PAYMENT_HANDOFF_CREATE_FAILED");
      return;
    }
    setPaying(true);
    void openInBrowser(url)
      .then(() => {
        setPaymentOpen(true);
        setPaymentBalanceUpdated(false);
      })
      .catch((error: unknown) => pushToast(errorCodeOf(error)))
      .finally(() => setPaying(false));
  };

  const openUpdatePage = () => {
    const url = updateReminder?.downloadUrl;
    if (!url) return;
    void openInBrowser(url).catch((error: unknown) => pushToast(errorCodeOf(error)));
  };

  return (
    <div className="lumio-dashboard">
      <section className="lumio-welcome-row">
        <div>
          <p className="lumio-eyebrow">{shellLabels.accountStatus}</p>
          <h1>欢迎回来</h1>
          <p>{account.email}</p>
        </div>
        <span className="lumio-secure-chip">
          <ShieldCheck size={16} />
          凭据由系统保护
        </span>
      </section>

      {updateReminder?.updateAvailable ? (
        <p className="lumio-notice is-update" role="status">
          <Download size={15} />
          <span>
            发现新版本 {updateReminder.latestVersion ?? ""}（当前 v{updateReminder.currentVersion}）
          </span>
          <button className="lumio-small-button" onClick={openUpdatePage} type="button">
            查看更新
          </button>
          <button className="lumio-link-button" onClick={onDismissUpdate} type="button">
            稍后
          </button>
        </p>
      ) : null}

      {reconnected ? (
        <p className="lumio-notice is-success" role="status">
          <CheckCircle2 size={15} />
          已重新连接，数据已自动刷新。
        </p>
      ) : offline ? (
        <p className="lumio-notice" role="status">
          <CloudOff size={15} />
          {syncTimeUnknown
            ? "无法连接服务，正在使用本机缓存，上次同步时间未知。你仍可以启动官方 Codex。"
            : `无法连接服务，正在使用 ${formatSyncTime(state.cachedAt)} 的本机缓存。你仍可以启动官方 Codex。`}
        </p>
      ) : null}

      <div className="lumio-metric-grid">
        <article className={`lumio-card lumio-balance-card${offline ? " is-cached" : ""}`}>
          <span className="lumio-card-icon">
            <WalletCards size={20} />
          </span>
          <p>{shellLabels.balanceAndPlan}</p>
          <strong>{syncTimeUnknown ? "未知" : formatBalance(account.balance)}</strong>
          <small>
            {offline ? (
              <span className="lumio-tag is-warning">{syncTimeUnknown ? "尚未同步" : "缓存值"}</span>
            ) : null}
            {syncTimeUnknown ? "恢复网络后自动更新" : (account.planLabel ?? "当前没有生效套餐")}
          </small>
        </article>

        <article className="lumio-card">
          <span className="lumio-card-icon">
            <Activity size={20} />
          </span>
          <p>{shellLabels.connectionStatus}</p>
          <strong>{offline ? "本机就绪" : "在线"}</strong>
          <small>
            {offline
              ? "使用本机缓存"
              : syncTimeUnknown
                ? "尚未同步"
                : `上次同步 ${formatSyncTime(state.cachedAt)}`}
            <button
              className="lumio-small-button is-inline"
              disabled={!actions.canRefresh || refreshing}
              onClick={refresh}
              title={actions.canRefresh ? undefined : (actionNotes.refresh ?? undefined)}
              type="button"
            >
              <RefreshCw size={12} />
              {refreshing ? "刷新中…" : "刷新"}
            </button>
          </small>
          {actions.canRefresh ? null : <small className="lumio-card-note">{actionNotes.refresh}</small>}
        </article>

        <article className="lumio-card">
          <span className="lumio-card-icon">
            <Star size={20} />
          </span>
          <p>{shellLabels.defaultModel}</p>
          <strong>{state.defaultModel ?? "等待服务端同步"}</strong>
          <small>
            <span className="lumio-tag is-success">已配置</span>
            由服务端管理
          </small>
        </article>
      </div>

      <section className="lumio-action-panel">
        <div>
          <p className="lumio-eyebrow">官方 Codex</p>
          <h2>{codexApp === null ? "尚未检测到官方应用" : "已检测到官方应用"}</h2>
          <p>
            {codexApp === null
              ? NO_APP_SUBCOPY
              : `${codexApp.version === null ? "已就绪" : `版本 ${codexApp.version}`} · ${codexApp.path}`}
          </p>
        </div>
        <div className="lumio-actions">
          <button
            className="lumio-button is-secondary"
            disabled={!actions.canPay || paying}
            onClick={openPayment}
            title={actions.canPay ? undefined : (actionNotes.pay ?? undefined)}
            type="button"
          >
            <WalletCards size={17} />
            {paying ? "正在打开…" : shellLabels.payment}
          </button>
          <button
            className="lumio-button is-primary"
            disabled={primaryDisabled}
            onClick={onPrimaryClick}
            type="button"
          >
            <Rocket size={17} />
            {primaryLabel}
          </button>
        </div>
      </section>

      {actions.canPay ? null : <p className="lumio-settings-note">{actionNotes.pay}</p>}
      {!actions.canLaunch && actionNotes.launch ? (
        <p className="lumio-settings-note">
          {actionNotes.launch}
          <button className="lumio-link-button" onClick={onOpenSettings} type="button">
            {shellLabels.settings}
          </button>
        </p>
      ) : null}

      {paymentOpen ? (
        <div aria-modal="true" className="lumio-modal-backdrop" role="dialog">
          <div className="lumio-modal">
            {paymentBalanceUpdated ? (
              <>
                <h3>余额已更新</h3>
                <p>当前余额 {formatBalance(account.balance)}。</p>
              </>
            ) : (
              <>
                <h3>已在浏览器中打开支付页面</h3>
                <p>完成支付后回到这里，余额会自动更新。</p>
                <div className="lumio-modal-actions">
                  <button
                    className="lumio-button is-secondary"
                    onClick={() => {
                      setPaymentOpen(false);
                      setPaymentBalanceUpdated(false);
                    }}
                    type="button"
                  >
                    关闭
                  </button>
                  <button className="lumio-button is-primary" onClick={openPayment} type="button">
                    重新打开浏览器
                  </button>
                </div>
              </>
            )}
          </div>
        </div>
      ) : null}
    </div>
  );
}
