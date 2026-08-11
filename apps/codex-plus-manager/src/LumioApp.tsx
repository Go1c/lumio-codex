import {
  Activity,
  ArrowUpRight,
  Check,
  ChevronRight,
  CircleUserRound,
  CloudOff,
  Download,
  FileArchive,
  Home,
  Laptop,
  LockKeyhole,
  LogIn,
  RefreshCw,
  Rocket,
  RotateCcw,
  Settings,
  ShieldCheck,
  Sparkles,
  WalletCards,
} from "lucide-react";
import { useEffect, useReducer, useState } from "react";

import { loadLumioBootstrap, shellLabels } from "./lumio/invoke.ts";
import { initialLumioState, reduceLumioState } from "./lumio/state.ts";
import type { LumioState } from "./lumio/state.ts";
import type { LumioAccountSummary, LumioCodexApp, LumioPhase } from "./lumio/types.ts";

type View = "home" | "settings";

const phaseCopy: Record<LumioPhase, string> = {
  bootstrapping: "正在检查本机环境",
  "signed-out": "等待登录",
  authenticating: "正在验证账户",
  provisioning: "正在准备连接",
  "ready-online": "服务连接正常",
  "ready-offline": "使用本机缓存",
  "needs-repair": "需要检查配置",
};

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

  useEffect(() => {
    let active = true;
    void loadLumioBootstrap()
      .then((payload) => {
        if (active) dispatch({ type: "bootstrapped", payload });
      })
      .catch(() => {
        if (active) {
          dispatch({ type: "repair-required", errorCode: "BOOTSTRAP_FAILED" });
        }
      });
    return () => {
      active = false;
    };
  }, []);

  const account = state.bootstrap?.account ?? null;
  const codexApp = state.bootstrap?.codexApp ?? null;
  const online = state.phase === "ready-online";
  const offline = state.phase === "ready-offline";

  return (
    <div className="lumio-app">
      <div aria-hidden="true" className="lumio-aurora lumio-aurora-one" />
      <div aria-hidden="true" className="lumio-aurora lumio-aurora-two" />

      <header className="lumio-topbar">
        <button className="lumio-brand" onClick={() => setView("home")} type="button">
          <span className="lumio-logo-wrap">
            <img alt="" className="lumio-logo" src="/lumio-icon.png" />
          </span>
          <span>
            <strong>Lumio Codex</strong>
            <small>DESKTOP</small>
          </span>
        </button>

        <nav aria-label="主导航" className="lumio-nav">
          <button
            aria-current={view === "home" ? "page" : undefined}
            className={view === "home" ? "is-active" : ""}
            onClick={() => setView("home")}
            type="button"
          >
            <Home size={16} />
            首页
          </button>
          <button
            aria-current={view === "settings" ? "page" : undefined}
            className={view === "settings" ? "is-active" : ""}
            onClick={() => setView("settings")}
            type="button"
          >
            <Settings size={16} />
            设置
          </button>
        </nav>

        <div className={`lumio-phase${online ? " is-online" : offline ? " is-offline" : ""}`}>
          <span />
          {phaseCopy[state.phase]}
        </div>
      </header>

      <main className="lumio-main">
        {view === "home" ? (
          <HomeView
            account={account}
            codexApp={codexApp}
            phase={state.phase}
            state={state}
          />
        ) : (
          <SettingsView
            autoUpdateEnabled={state.autoUpdateEnabled}
            codexApp={codexApp}
            telemetryEnabled={state.telemetryEnabled}
          />
        )}
      </main>

      <footer className="lumio-footer">
        <span>内部测试渠道</span>
        <span className="lumio-footer-separator" />
        <span>官方应用需单独安装</span>
        {state.bootstrap ? (
          <span className="lumio-footer-version">v{state.bootstrap.version}</span>
        ) : null}
      </footer>
    </div>
  );
}

interface HomeViewProps {
  account: LumioAccountSummary | null;
  codexApp: LumioCodexApp | null;
  phase: LumioPhase;
  state: LumioState;
}

function HomeView({ account, codexApp, phase, state }: HomeViewProps) {
  if (phase === "bootstrapping") {
    return (
      <section aria-live="polite" className="lumio-loading">
        <span className="lumio-loading-mark">
          <RefreshCw size={24} />
        </span>
        <p>正在检测官方应用并读取本机状态…</p>
      </section>
    );
  }

  if (phase === "needs-repair") {
    return (
      <section className="lumio-repair-panel">
        <span className="lumio-panel-icon is-warning">
          <CloudOff size={24} />
        </span>
        <div>
          <p className="lumio-eyebrow">启动检查未完成</p>
          <h1>暂时无法读取 Lumio Codex 状态</h1>
          <p>请稍后重新打开应用。本机配置尚未被修改。</p>
          <code>{state.errorCode ?? "BOOTSTRAP_FAILED"}</code>
        </div>
      </section>
    );
  }

  if (account === null) {
    return <SignedOutHero codexApp={codexApp} />;
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
          <strong>等待服务端同步</strong>
          <small>模型目录将由 LumioAPI 提供</small>
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

function SignedOutHero({
  codexApp,
}: {
  codexApp: LumioCodexApp | null;
}) {
  return (
    <div className="lumio-signed-out">
      <section className="lumio-hero">
        <div className="lumio-hero-copy">
          <span className="lumio-kicker">
            <Sparkles size={14} />
            LumioAPI 原生桌面入口
          </span>
          <h1>
            让连接更简单，
            <span>让 Codex 保持原生。</span>
          </h1>
          <p>
            Lumio Codex 会自动完成账户连接和本机配置。后续模型切换仍在官方 Codex 中进行。
          </p>
          <button className="lumio-button is-primary is-large" disabled type="button">
            <LogIn size={18} />
            账户功能接入中
            <ArrowUpRight size={17} />
          </button>
          <small className="lumio-inline-note">
            <LockKeyhole size={14} />
            不需要手动填写底层连接信息
          </small>
        </div>

        <div className="lumio-orbit-card">
          <div className="lumio-orbit-glow" />
          <div className="lumio-orbit-logo">
            <img alt="Lumio" src="/lumio-icon.png" />
          </div>
          <div className="lumio-orbit-ring ring-one" />
          <div className="lumio-orbit-ring ring-two" />
          <span className="lumio-orbit-node node-one">
            <Check size={14} />
          </span>
          <span className="lumio-orbit-node node-two">
            <ShieldCheck size={14} />
          </span>
          <p>LUMIO × CODEX</p>
          <small>RESPONSES READY</small>
        </div>
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
          <ChevronRight size={17} />
        </article>
        <article>
          <span className={`lumio-status-icon${codexApp ? " is-success" : ""}`}>
            <Laptop size={19} />
          </span>
          <div>
            <small>官方应用</small>
            <strong>{codexApp ? "检测成功" : "等待手动选择"}</strong>
          </div>
          {codexApp ? <Check size={17} /> : <ChevronRight size={17} />}
        </article>
        <article>
          <span className="lumio-status-icon is-success">
            <Activity size={19} />
          </span>
          <div>
            <small>服务入口</small>
            <strong>api.lumio.games</strong>
          </div>
          <Check size={17} />
        </article>
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
        <h1>设置</h1>
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
            <p className="lumio-path-value">
              {codexApp?.path ?? "未自动检测到，可在功能接入后手动选择"}
            </p>
          </div>
          <button className="lumio-small-button" disabled type="button">
            <RefreshCw size={15} />
            重新检测
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
        当前为内部测试外壳；不可用的选项会保持禁用，不会修改本机配置。
      </p>
    </section>
  );
}
