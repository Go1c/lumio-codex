import { AlertTriangle, CheckCircle2, CloudOff, FolderOpen, RefreshCw, Rocket } from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";

import { greetingNameFromEmail } from "../greeting.ts";
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
import { destinationOptions } from "../install-destination.ts";
import { paymentUrl } from "../payment.ts";
import { isOfficialAppInstallInProgress, type LumioState } from "../state.ts";
import {
  downloadPercent,
  installFailureCopy,
  installProgressCopy,
  installStageLabel,
  toInstallProgress,
} from "../install-progress.ts";
import type {
  LumioAccountSummary,
  LumioCodexApp,
  LumioOfficialAppInstall,
} from "../types.ts";
import type { ToastTone } from "./Toast.tsx";

const RECONNECT_PROBE_MS = 30_000;
const RECONNECT_BANNER_MS = 3000;
const PAYMENT_POLL_MS = 10_000;
const INSTALL_POLL_MS = 400;
const INSTALL_AND_LAUNCH_COPY = "安装并启动官方 Codex";
const INSTALLING_COPY = "正在安装官方 Codex…";
const OFFLINE_NO_APP_NOTE = "安装官方应用需要网络";

function errorCodeOf(error: unknown): string {
  return error instanceof LumioCommandError ? error.errorCode : "UNKNOWN";
}

function formatBalance(balance: number): string {
  return new Intl.NumberFormat("zh-CN", {
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  }).format(balance);
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
  onRefreshed: (account: LumioAccountSummary, cachedAt: string) => void;
  onReconnected: (account: LumioAccountSummary, cachedAt: string) => void;
  onCodexAppChanged: (app: LumioCodexApp | null) => void;
  onInstallProgress: (status: LumioOfficialAppInstall) => void;
  onOpenSettings: () => void;
  pushToast: (input: string, tone?: ToastTone) => void;
}

export function HomeView({
  state,
  onRefreshed,
  onReconnected,
  onCodexAppChanged,
  onInstallProgress,
  onOpenSettings,
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
      .catch(async (error: unknown) => {
        const code = errorCodeOf(error);
        pushToast(code);
        if (code !== "CODEX_APP_NOT_FOUND") return;
        // 应用被删/移后首页还留着安装时的旧状态（D-25）：重新检测一次，
        // 没了就翻回「安装并启动官方 Codex」引导，别让用户困在
        // 只会反复报错的「启动」上。
        const detected = await detectCodexApp().catch(() => null);
        onCodexAppChanged(detected);
      })
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

  // D-23：首次安装先问装哪里（标准路线 or 自选目录），选择权交给用户。
  const [destinationOpen, setDestinationOpen] = useState(false);

  const chooseDirectory = (): Promise<string | null> => {
    return open({ directory: true, multiple: false, title: "选择安装目录" }).then((picked) =>
      typeof picked === "string" ? picked : null,
    );
  };

  const installThenLaunch = (destination: string | null) => {
    setDestinationOpen(false);
    setLaunching(true);
    void (async () => {
      try {
        const started = await installOfficialApp(destination);
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
    setDestinationOpen(true);
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
  const installInProgress = isOfficialAppInstallInProgress(officialAppInstall);
  const installEnded =
    officialAppInstall.phase === "failed" || officialAppInstall.phase === "cancelled";
  const downloadPercentValue =
    officialAppInstall.phase === "downloading"
      ? downloadPercent(
          officialAppInstall.bytesDownloaded ?? null,
          officialAppInstall.bytesTotal ?? null,
        )
      : null;
  const progressCopy = installProgressCopy(officialAppInstall, downloadPercentValue);
  const failureCopy = installEnded ? installFailureCopy(officialAppInstall) : null;
  const greetingName = greetingNameFromEmail(account.email);
  const offlineMissing = offline && codexApp === null && !installInProgress;

  const cardTitle = installInProgress
    ? "正在安装官方 Codex"
    : installEnded
      ? "安装没有完成"
      : offlineMissing
        ? "尚未安装官方 Codex"
        : offline
          ? "离线可用"
          : codexApp
            ? "Codex 已就绪"
            : "尚未安装官方 Codex";
  const cardPill = installInProgress
    ? (stageLabel ?? "安装")
    : installEnded
      ? "可重试"
      : offlineMissing
        ? "需要网络"
        : offline
          ? "缓存"
          : codexApp
            ? "可启动"
            : "未安装";
  const cardMeta = installInProgress
    ? progressCopy
    : offlineMissing
      ? OFFLINE_NO_APP_NOTE
      : offline
        ? "本机配置还在。充值需要网络，启动不需要。"
        : codexApp
          ? "本机配置已写好。启动后进入官方 Codex，这个窗口留在这里。"
          : "装好后会自动启动。安装过程可以切到 Claude，回来进度还在。";

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

  return (
    <div className="lumio-dashboard">
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

      <div className="lumio-home-top">
        <div>
          <h1>你好，{greetingName}</h1>
          <p className="lumio-balance-line">
            {account.email}
            {" · "}
            余额{" "}
            <strong>{syncTimeUnknown ? "未知" : formatBalance(account.balance)}</strong>
            {offline ? (
              <span className="lumio-tag is-warning">{syncTimeUnknown ? "尚未同步" : "缓存值"}</span>
            ) : null}
            <button
              className="lumio-link-button"
              disabled={!actions.canPay || paying}
              onClick={openPayment}
              title={actions.canPay ? undefined : (actionNotes.pay ?? undefined)}
              type="button"
            >
              {paying ? "正在打开…" : shellLabels.payment}
            </button>
            <button
              className="lumio-link-button"
              disabled={!actions.canRefresh || refreshing}
              onClick={refresh}
              title={actions.canRefresh ? undefined : (actionNotes.refresh ?? undefined)}
              type="button"
            >
              <RefreshCw size={12} />
              {refreshing ? "刷新中…" : "刷新余额"}
            </button>
          </p>
        </div>
        <span className={`lumio-chip${offline ? " is-offline" : ""}`}>
          <i />
          {offline ? "离线" : "服务正常"}
        </span>
      </div>

      <section className="lumio-launch-card">
        <span className="lumio-app-icon is-card" aria-hidden="true">
          <img alt="" src="/lumio-icon.png" />
        </span>
        <p className="lumio-eyebrow">官方应用</p>
        <header>
          <h2>{cardTitle}</h2>
          <span className={`lumio-pill${offline || installEnded || offlineMissing ? " is-warn" : " is-ok"}`}>
            {cardPill}
          </span>
        </header>
        <p className="lumio-launch-meta">{cardMeta}</p>

        <div className="lumio-actions">
          <button
            className="lumio-button is-primary"
            disabled={primaryDisabled}
            onClick={onPrimaryClick}
            title={!actions.canLaunch ? (actionNotes.launch ?? undefined) : undefined}
            type="button"
          >
            <Rocket size={17} />
            {primaryLabel}
          </button>
        </div>

        {installInProgress ? (
          <div className="lumio-install-progress">
            <div
              aria-label={progressCopy}
              aria-valuemax={100}
              aria-valuemin={0}
              aria-valuenow={downloadPercentValue ?? undefined}
              className="lumio-install-progress-track"
              role="progressbar"
            >
              <div
                className={`lumio-install-progress-fill${
                  downloadPercentValue === null ? " is-indeterminate" : ""
                }`}
                style={{ width: `${downloadPercentValue ?? 30}%` }}
              />
            </div>
            <p>{progressCopy}</p>
          </div>
        ) : null}

        {installEnded && failureCopy !== null ? (
          // D-20：失败原因常驻面板（toast 只有 4 秒），主按钮即重试入口。
          <p className="lumio-install-failure" role="alert">
            <AlertTriangle size={14} />
            {failureCopy}
          </p>
        ) : null}

        {codexApp ? <p className="lumio-launch-path">{codexApp.path}</p> : null}

        {actions.canPay ? null : <p className="lumio-settings-note">{actionNotes.pay}</p>}
        {!actions.canLaunch && actionNotes.launch ? (
          <p className="lumio-settings-note">
            {actionNotes.launch}
            <button className="lumio-link-button" onClick={onOpenSettings} type="button">
              {shellLabels.settings}
            </button>
          </p>
        ) : null}

        {destinationOpen ? (
          <div className="lumio-sheet-back">
            <div aria-modal="true" className="lumio-sheet" role="dialog">
              <h3>选择安装位置</h3>
              <p>先选位置，再开始下载。取消不会开始安装。</p>
              <div className="lumio-place-list">
                {destinationOptions(state.bootstrap?.platform ?? "").map((option) => (
                  <button
                    className="lumio-place"
                    key={option.id}
                    onClick={() => {
                      if (option.id === "standard") {
                        installThenLaunch(null);
                        return;
                      }
                      void chooseDirectory().then((dir) => {
                        if (dir !== null) installThenLaunch(dir);
                      });
                    }}
                    type="button"
                  >
                    <FolderOpen size={16} />
                    <span>
                      <b>{option.label}</b>
                      {option.note === null ? null : <small>{option.note}</small>}
                    </span>
                  </button>
                ))}
              </div>
              <button
                className="lumio-button is-secondary is-block"
                onClick={() => setDestinationOpen(false)}
                type="button"
              >
                取消
              </button>
            </div>
          </div>
        ) : null}
      </section>

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
