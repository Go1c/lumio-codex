import { LoginCard, type LoginCardProps } from "./LoginCard.tsx";

export type ChecklistStepStatus = "done" | "now" | "fail" | "pending";

export type InitStepKey = "connect" | "component" | "sync" | "install" | "login";

export type InitPhase = "connecting" | "component" | "syncing" | "installing" | "fail" | "login" | "done";

export type ChecklistStep = {
  status?: ChecklistStepStatus;
  detail?: string;
  when?: string;
};

export type ResumeStepKey = "connect" | "restore" | "align";

export const INIT_STEP_TITLES: Record<InitStepKey, string> = {
  connect: "连接服务器",
  component: "装同步组件",
  sync: "首次同步文件",
  install: "安装 Claude",
  login: "登录 Anthropic",
};

export const RESUME_STEP_TITLES: Record<ResumeStepKey, string> = {
  connect: "连上服务器",
  restore: "恢复上次的对话",
  align: "对齐文件",
};

const INIT_ORDER: InitStepKey[] = ["connect", "component", "sync", "install", "login"];
const RESUME_ORDER: ResumeStepKey[] = ["connect", "restore", "align"];

const CLI_DOWNLOAD_FAILED = "CLAUDE_CLI_DOWNLOAD_FAILED";
const CLI_DOWNLOAD_DETAIL = "服务器连不上官方下载地址。确认这台服务器能访问外网，或稍后再试。";
const SSH_TIMEOUT = "CLAUDE_SSH_TIMEOUT";
const SSH_TIMEOUT_DETAIL = "连接超时。检查这台服务器是否开机、网络是否可达。";
const DEFAULT_LOCAL_ROOT = "~/BestCodex/my-project";

function noop(): void {}
function noopCode(_code: string): void {}

export type InitChecklistProps = {
  phase: InitPhase;
  steps?: Partial<Record<InitStepKey, ChecklistStep>>;
  failedStep?: InitStepKey;
  progressPercent?: number;
  hostLabel?: string;
  claudeVersion?: string;
  filesDone?: number;
  filesTotal?: number;
  elapsedLabel?: string;
  downloadLabel?: string;
  installDetail?: string;
  localRoot?: string;
  failDetail?: string;
  errorCode?: string | null;
  onRetryInstall?: () => void;
  onRetrySync?: () => void;
  onRetryConnect?: () => void;
  onSwitchToCodex?: () => void;
  onOpenHelp?: () => void;
  onStartChat?: () => void;
  login?: Omit<LoginCardProps, "layout">;
};

export function InitChecklist({
  phase,
  steps,
  failedStep,
  progressPercent,
  hostLabel,
  claudeVersion,
  filesDone,
  filesTotal,
  elapsedLabel,
  downloadLabel,
  installDetail,
  localRoot,
  failDetail,
  errorCode,
  onRetryInstall,
  onRetrySync,
  onRetryConnect,
  onSwitchToCodex,
  onOpenHelp,
  onStartChat,
  login,
}: InitChecklistProps) {
  const resolvedFail = failedStep ?? inferFailedStep(steps) ?? "install";
  const resolved = INIT_ORDER.map((key, index) => {
    const override = steps?.[key];
    const status = override?.status ?? defaultInitStatus(phase, resolvedFail, index);
    return {
      key,
      title: INIT_STEP_TITLES[key],
      status,
      detail:
        override?.detail ??
        defaultInitDetail(key, status, {
          phase,
          hostLabel,
          claudeVersion,
          filesDone,
          filesTotal,
          elapsedLabel,
          downloadLabel,
          installDetail,
        }),
      when: override?.when ?? defaultWhen(key, status, phase),
    };
  });
  const header = initHeader(phase, resolvedFail, claudeVersion, filesTotal);
  const progress = initProgress(phase, progressPercent, filesDone, filesTotal);
  const failText = failBoxText(resolvedFail, errorCode, failDetail);
  const failCode = failBoxCode(resolvedFail, errorCode);
  const local = localRoot ?? DEFAULT_LOCAL_ROOT;

  const retry = () => {
    if (resolvedFail === "sync") onRetrySync?.();
    else if (resolvedFail === "connect" || resolvedFail === "component") onRetryConnect?.();
    else onRetryInstall?.();
  };

  const preparing = phase === "connecting" || phase === "component" || phase === "syncing";
  const primary =
    phase === "login"
      ? null
      : phase === "done"
        ? { label: "开始对话", disabled: false, busy: false, onClick: onStartChat }
        : phase === "fail"
          ? { label: "重试这一步", disabled: false, busy: false, onClick: retry }
          : phase === "installing"
            ? { label: "正在安装…", disabled: true, busy: true, onClick: undefined }
            : { label: "正在准备…", disabled: true, busy: true, onClick: undefined };

  return (
    <div className="lumio-claude-init-pane">
      <div className="lumio-claude-ws-card lumio-claude-init">
        <header>
          <h3>{header.title}</h3>
          {header.pill ? <span className={`lumio-claude-init-pill ${header.pillClass}`}>{header.pill}</span> : null}
          {header.note ? <span className="lumio-claude-init-note">{header.note}</span> : null}
        </header>
        {header.meta ? <p className="lumio-claude-init-meta">{header.meta}</p> : null}
        {phase === "fail" && (failText || failCode) ? (
          <div className="lumio-claude-fail" role="alert">
            {failText}
            {failCode ? <span className="lumio-claude-fail-code">{failCode}</span> : null}
          </div>
        ) : null}
        {progress ? (
          <div className={`lumio-claude-progress${progress.indeterminate ? " is-indeterminate" : ""}`}>
            <i style={{ width: progress.width }} />
          </div>
        ) : null}
        <div className="lumio-claude-init-steps">
          {resolved.map((step) => (
            <div
              aria-current={step.status === "now" ? "step" : undefined}
              className={`lumio-claude-init-step is-${step.status}`}
              key={step.key}
            >
              <i />
              <div>
                <b>{step.title}</b>
                {step.detail ? <span className="d">{step.detail}</span> : null}
              </div>
              <span className="lumio-claude-init-when">{step.when}</span>
            </div>
          ))}
        </div>
        {primary ? (
          <div className="lumio-claude-actions">
            <button
              aria-busy={primary.busy || undefined}
              className={`lumio-button is-primary${primary.busy ? " is-busy" : ""}`}
              disabled={primary.disabled}
              onClick={primary.onClick}
              type="button"
            >
              {primary.busy ? <i aria-hidden="true" className="lumio-button-spinner" /> : null}
              {primary.label}
            </button>
            {preparing && onSwitchToCodex ? (
              <button className="lumio-button is-secondary" onClick={onSwitchToCodex} type="button">
                切到 Codex（这边继续跑）
              </button>
            ) : null}
            {phase === "fail" && onOpenHelp ? (
              <button className="lumio-button is-secondary" onClick={onOpenHelp} type="button">
                打开帮助页
              </button>
            ) : null}
          </div>
        ) : null}
        {phase === "login" ? (
          <LoginCard
            claudeVersion={login?.claudeVersion ?? claudeVersion}
            layout="embedded"
            loginUrl={login?.loginUrl ?? null}
            onCopyLink={login?.onCopyLink ?? noop}
            onOpenBrowser={login?.onOpenBrowser ?? noop}
            onSubmitCode={login?.onSubmitCode ?? noopCode}
          />
        ) : null}
        {phase === "installing" ? (
          <p className="lumio-claude-init-quiet">下载和安装都在服务器上跑，不占用这台电脑。</p>
        ) : null}
        {phase === "fail" && resolvedFail === "install" ? (
          <p className="lumio-claude-init-quiet">
            文件已经同步好了。这步失败不影响你在本机 {local} 里改文件。
          </p>
        ) : null}
        {phase === "done" ? (
          <p className="lumio-claude-init-quiet">默认不自动进 Claude。点了「开始对话」才进。</p>
        ) : null}
      </div>
    </div>
  );
}

export function ResumeProgress({
  projectName,
  hostLabel,
  steps,
  onPeek,
}: {
  projectName: string;
  hostLabel?: string;
  steps?: Partial<Record<ResumeStepKey, ChecklistStep>>;
  onPeek?: () => void;
}) {
  const resolved = RESUME_ORDER.map((key, index) => {
    const override = steps?.[key];
    const status = override?.status ?? (index === 0 ? "now" : "pending");
    return {
      key,
      title: RESUME_STEP_TITLES[key],
      status,
      when: override?.when ?? defaultWhen(key, status, "syncing"),
    };
  });

  return (
    <div className="lumio-claude-shell-center">
      <div className="lumio-claude-ws-card lumio-claude-resume-card">
        <header>
          <h3>正在连接 {projectName}</h3>
          {hostLabel ? <span className="lumio-claude-init-note">{hostLabel}</span> : null}
        </header>
        <p className="lumio-claude-init-meta">不用你再填什么，几秒就好。</p>
        <div className="lumio-claude-progress is-indeterminate">
          <i />
        </div>
        <div className="lumio-claude-init-steps">
          {resolved.map((step) => (
            <div
              aria-current={step.status === "now" ? "step" : undefined}
              className={`lumio-claude-init-step is-${step.status}`}
              key={step.key}
            >
              <i />
              <div>
                <b>{step.title}</b>
              </div>
              <span className="lumio-claude-init-when">{step.when}</span>
            </div>
          ))}
        </div>
        <div className="lumio-claude-actions">
          <button className="lumio-button is-primary" disabled={!onPeek} onClick={onPeek} type="button">
            进去看看
          </button>
        </div>
      </div>
    </div>
  );
}

export function OfflineCard({
  host,
  localRoot,
  failDetail,
  errorCode,
  onRetryConnect,
  onViewLocalFiles,
  onDismiss,
}: {
  host: string;
  localRoot?: string;
  failDetail?: string;
  errorCode?: string | null;
  onRetryConnect?: () => void;
  onViewLocalFiles?: () => void;
  onDismiss?: () => void;
}) {
  const local = localRoot ?? DEFAULT_LOCAL_ROOT;
  const code = errorCode ?? SSH_TIMEOUT;
  const detail = failDetail ?? (code === SSH_TIMEOUT ? SSH_TIMEOUT_DETAIL : undefined);

  return (
    <div className="lumio-claude-shell-center">
      <div className="lumio-claude-ws-card lumio-claude-offline-card">
        <header>
          <h3>连不上这台服务器</h3>
          {onDismiss ? (
            <button
              aria-label="关闭"
              className="lumio-claude-init-note"
              onClick={onDismiss}
              type="button"
            >
              离线
            </button>
          ) : (
            <span className="lumio-claude-init-note">离线</span>
          )}
        </header>
        <p className="lumio-claude-init-meta">
          现在连不上 {host}。本机 {local} 里的文件照常能改，恢复连接后自动对齐，不会静默覆盖谁。
        </p>
        <div className="lumio-claude-fail" role="alert">
          {detail}
          <span className="lumio-claude-fail-code">{code}</span>
        </div>
        <div className="lumio-claude-actions">
          <button className="lumio-button is-primary" onClick={onRetryConnect} type="button">
            重试连接
          </button>
          <button className="lumio-button is-secondary" disabled={!onViewLocalFiles} onClick={onViewLocalFiles} type="button">
            看本机文件
          </button>
        </div>
      </div>
    </div>
  );
}

export function PickProjectHint() {
  return (
    <div className="lumio-claude-shell-center">
      <div className="lumio-claude-pick">
        <span className="lumio-claude-icon" aria-hidden="true">
          <img alt="" src="/lumio-icon.png" />
        </span>
        <h3>挑一个项目</h3>
        <p>
          左边点一下就自动连上那台服务器，上次的对话还在。
          <br />
          每个项目就是一台服务器加一个文件夹。
        </p>
      </div>
    </div>
  );
}

function inferFailedStep(steps?: Partial<Record<InitStepKey, ChecklistStep>>): InitStepKey | undefined {
  return INIT_ORDER.find((key) => steps?.[key]?.status === "fail");
}

function defaultInitStatus(phase: InitPhase, failedStep: InitStepKey, index: number): ChecklistStepStatus {
  const now = phaseNowIndex(phase, failedStep);
  if (phase === "fail") {
    if (index < now) return "done";
    if (index === now) return "fail";
    return "pending";
  }
  if (index < now) return "done";
  if (index === now) return "now";
  return "pending";
}

function phaseNowIndex(phase: InitPhase, failedStep: InitStepKey): number {
  switch (phase) {
    case "connecting":
      return 0;
    case "component":
      return 1;
    case "syncing":
      return 2;
    case "installing":
      return 3;
    case "login":
      return 4;
    case "done":
      return 5;
    case "fail":
      return Math.max(0, INIT_ORDER.indexOf(failedStep));
  }
}

function defaultWhen(key: string, status: ChecklistStepStatus, phase: InitPhase): string {
  if (status === "now") return key === "login" ? "等你" : "进行中";
  if (status === "fail") return "失败";
  if (status === "pending" && phase === "fail" && key === "login") return "等这步好了再来";
  if (status === "pending") return "待办";
  return "";
}

function defaultInitDetail(
  key: InitStepKey,
  status: ChecklistStepStatus,
  ctx: {
    phase: InitPhase;
    hostLabel?: string;
    claudeVersion?: string;
    filesDone?: number;
    filesTotal?: number;
    elapsedLabel?: string;
    downloadLabel?: string;
    installDetail?: string;
  },
): string | undefined {
  switch (key) {
    case "connect":
      return ctx.hostLabel;
    case "component":
      return status === "pending" ? undefined : "本机与服务器双向同步已就位";
    case "sync":
      if (status === "now" && ctx.filesDone != null && ctx.filesTotal != null) {
        return `${ctx.filesDone} / ${ctx.filesTotal} 个文件${ctx.elapsedLabel ? ` · 已用 ${ctx.elapsedLabel}` : ""}`;
      }
      if (ctx.filesTotal != null && ctx.filesTotal > 0) return `${ctx.filesTotal} 个文件`;
      return undefined;
    case "install":
      if (ctx.installDetail) return ctx.installDetail;
      if (status === "fail") return "下载中断";
      if (status === "pending") return "取官方当前最新版";
      if (status === "now") {
        const head = ctx.claudeVersion ? `取到最新版 ${ctx.claudeVersion}` : "取官方当前最新版";
        const download = ctx.downloadLabel ? ` · 正在下载 ${ctx.downloadLabel}` : "";
        return `${head}${download} · 校验后装到 ~/.local/bin`;
      }
      if (status === "done" && ctx.claudeVersion) {
        return ctx.phase === "login" ? `${ctx.claudeVersion} · 已校验` : ctx.claudeVersion;
      }
      return undefined;
    case "login":
      if (status === "done") return "已授权";
      if (status === "now") return "点下面的按钮，浏览器里授权";
      if (ctx.phase === "fail") return undefined;
      return "用你的 Claude 账号授权一次";
  }
}

function initHeader(
  phase: InitPhase,
  failedStep: InitStepKey,
  claudeVersion: string | undefined,
  filesTotal: number | undefined,
): { title: string; note?: string; pill?: string; pillClass: string; meta?: string } {
  const version = claudeVersion ? ` ${claudeVersion}` : "";
  switch (phase) {
    case "connecting":
      return {
        title: "正在把这个项目准备好",
        note: "第 1 / 5 步",
        pillClass: "",
        meta: "全自动，不用你敲命令。装完就能直接用。",
      };
    case "component":
      return {
        title: "正在把这个项目准备好",
        note: "第 2 / 5 步",
        pillClass: "",
        meta: "全自动，不用你敲命令。装完就能直接用。",
      };
    case "syncing":
      return {
        title: "正在把这个项目准备好",
        note: "第 3 / 5 步",
        pillClass: "",
        meta: "全自动，不用你敲命令。装完就能直接用。",
      };
    case "installing":
      return {
        title: "正在安装 Claude",
        note: "第 4 / 5 步",
        pillClass: "",
        meta: `从官方渠道取当前最新版${version}，装在这台服务器上。你不用自己敲安装命令。`,
      };
    case "fail":
      if (failedStep === "install") {
        return {
          title: "没能装上 Claude",
          pill: "需要处理",
          pillClass: "is-warn",
          meta: "前面三步都成了。卡在这台服务器取不到官方安装包。",
        };
      }
      return {
        title: INIT_STEP_TITLES[failedStep],
        pill: "需要处理",
        pillClass: "is-warn",
      };
    case "login":
      return {
        title: "最后一步：登录 Anthropic",
        note: "第 5 / 5 步",
        pillClass: "",
        meta: `Claude${version} 已经装好。用你的 Claude 账号授权一次，之后不用再登。`,
      };
    case "done":
      return {
        title: "准备好了",
        pill: "5 / 5",
        pillClass: "is-ok",
        meta: `Claude${version} 已装好并登录${filesTotal != null ? `，${filesTotal} 个文件已同步` : ""}。中间这块马上换成终端。`,
      };
  }
}

function initProgress(
  phase: InitPhase,
  progressPercent: number | undefined,
  filesDone: number | undefined,
  filesTotal: number | undefined,
): { indeterminate: boolean; width: string } | null {
  if (phase === "fail" || phase === "login" || phase === "done") return null;
  if (progressPercent != null) return { indeterminate: false, width: `${progressPercent}%` };
  if (phase === "syncing" && filesTotal != null && filesTotal > 0) {
    return { indeterminate: false, width: `${Math.max(6, Math.round(((filesDone ?? 0) / filesTotal) * 100))}%` };
  }
  return { indeterminate: true, width: "36%" };
}

function failBoxText(
  failedStep: InitStepKey,
  errorCode: string | null | undefined,
  failDetail: string | undefined,
): string {
  if (failDetail) return failDetail;
  if (failedStep === "install" && (errorCode == null || errorCode === CLI_DOWNLOAD_FAILED)) {
    return CLI_DOWNLOAD_DETAIL;
  }
  return "";
}

function failBoxCode(failedStep: InitStepKey, errorCode: string | null | undefined): string {
  if (errorCode) return errorCode;
  if (failedStep === "install") return CLI_DOWNLOAD_FAILED;
  return "";
}
