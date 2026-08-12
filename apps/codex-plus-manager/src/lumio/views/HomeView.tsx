import { Activity, CheckCircle2, CloudOff, RefreshCw, Rocket, ShieldCheck, Sparkles, WalletCards } from "lucide-react";
import { useEffect, useState } from "react";

import { LumioCommandError, launchCodex, refreshAccount, shellLabels } from "../invoke.ts";
import type { LumioState } from "../state.ts";
import type { LumioAccountSummary } from "../types.ts";
import type { ToastTone } from "./Toast.tsx";

const RECONNECT_PROBE_MS = 30_000;
const RECONNECT_BANNER_MS = 3000;

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

interface HomeViewProps {
  state: LumioState;
  onRefreshed: (account: LumioAccountSummary, cachedAt: string) => void;
  onReconnected: (account: LumioAccountSummary, cachedAt: string) => void;
  onOpenSettings: () => void;
  pushToast: (input: string, tone?: ToastTone) => void;
}

export function HomeView({ state, onRefreshed, onReconnected, onOpenSettings, pushToast }: HomeViewProps) {
  const { account, actionNotes, actions, codexApp } = state;
  const offline = state.phase === "ready-offline";
  // 没有同步时间就没有同步过：账户数值此刻是启动时的占位而非缓存下来的真值，不能当余额渲染。
  const syncTimeUnknown = state.cachedAt === null;
  const [refreshing, setRefreshing] = useState(false);
  const [launching, setLaunching] = useState(false);
  const [reconnected, setReconnected] = useState(false);

  // Offline is a degraded normal state, so recovery is probed in the background
  // rather than asking the user to retry by hand (interaction spec §5.5).
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
          <button className="lumio-button is-secondary" disabled type="button">
            <WalletCards size={17} />
            {shellLabels.payment}
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

      <p className="lumio-settings-note">{actionNotes.pay}</p>
      {actions.canLaunch ? null : (
        <p className="lumio-settings-note">
          {actionNotes.launch}
          <button className="lumio-link-button" onClick={onOpenSettings} type="button">
            {shellLabels.settings}
          </button>
        </p>
      )}
    </div>
  );
}
