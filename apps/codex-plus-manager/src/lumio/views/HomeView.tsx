import {
  Activity,
  CheckCircle2,
  CloudOff,
  Download,
  RefreshCw,
  Rocket,
  ShieldCheck,
  Sparkles,
  WalletCards,
} from "lucide-react";
import { useEffect, useState } from "react";

import {
  LumioCommandError,
  launchCodex,
  openInBrowser,
  refreshAccount,
  shellLabels,
} from "../invoke.ts";
import type { LumioState } from "../state.ts";
import type { LumioAccountSummary, LumioUpdateReminder } from "../types.ts";
import type { ToastTone } from "./Toast.tsx";

const RECONNECT_PROBE_MS = 30_000;
const RECONNECT_BANNER_MS = 3000;
const PAYMENT_POLL_MS = 10_000;

function errorCodeOf(error: unknown): string {
  return error instanceof LumioCommandError ? error.errorCode : "UNKNOWN";
}

function formatBalance(balance: number): string {
  return new Intl.NumberFormat("zh-CN", {
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  }).format(balance);
}

function formatSyncTime(iso: string | null): string {
  if (iso === null) return "未知时间";
  const parsed = new Date(iso);
  if (Number.isNaN(parsed.getTime())) return "未知时间";
  return new Intl.DateTimeFormat("zh-CN", { hour: "2-digit", minute: "2-digit" }).format(parsed);
}

function paymentUrl(state: LumioState): string | null {
  const site = state.service?.siteBaseUrl?.replace(/\/$/, "");
  const path = state.service?.paymentPath ?? "/payment";
  if (!site) return null;
  return `${site}${path.startsWith("/") ? path : `/${path}`}`;
}

interface HomeViewProps {
  state: LumioState;
  updateReminder: LumioUpdateReminder | null;
  onRefreshed: (account: LumioAccountSummary, cachedAt: string) => void;
  onReconnected: (account: LumioAccountSummary, cachedAt: string) => void;
  onOpenSettings: () => void;
  onDismissUpdate: () => void;
  pushToast: (input: string, tone?: ToastTone) => void;
}

export function HomeView({
  state,
  updateReminder,
  onRefreshed,
  onReconnected,
  onOpenSettings,
  onDismissUpdate,
  pushToast,
}: HomeViewProps) {
  const { account, actionNotes, actions, codexApp } = state;
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

  if (account === null) {
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
            <Sparkles size={20} />
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
              ? "可在设置中重新检测或手动选择"
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
            disabled={!actions.canLaunch || launching}
            onClick={launch}
            type="button"
          >
            <Rocket size={17} />
            {launching ? "正在启动…" : shellLabels.launch}
          </button>
        </div>
      </section>

      {actions.canPay ? null : <p className="lumio-settings-note">{actionNotes.pay}</p>}
      {actions.canLaunch ? null : (
        <p className="lumio-settings-note">
          {actionNotes.launch}
          <button className="lumio-link-button" onClick={onOpenSettings} type="button">
            {shellLabels.settings}
          </button>
        </p>
      )}

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
