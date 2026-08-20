import { useEffect, useState } from "react";

import {
  activateClaudeProject,
  beginHostLogin,
  closeClaudeProjectChat,
  completeHostLogin,
  continueClaudeInit,
  refreshClaudeConflicts,
  refreshClaudeFiles,
} from "../../claude/session.ts";
import { dispatchClaude, getClaudeState } from "../../claude/store.ts";
import type { ClaudeCliInstallStatus, ClaudeLoginStatus, ClaudeProject, ClaudeState } from "../../claude/types.ts";
import { HELP_URL } from "../../help.ts";
import { openInBrowser } from "../../invoke.ts";
import { FileExplorer } from "./FileExplorer.tsx";
import {
  InitChecklist,
  OfflineCard,
  PickProjectHint,
  ResumeProgress,
  type ChecklistStep,
  type InitPhase,
  type InitStepKey,
  type ResumeStepKey,
} from "./InitChecklist.tsx";
import { LoginCard } from "./LoginCard.tsx";
import { ProjectRail } from "./ProjectRail.tsx";
import { SessionTabs } from "./SessionTabs.tsx";
import { StatusBar } from "./StatusBar.tsx";
import { StatusDrawer } from "./StatusDrawer.tsx";
import { TerminalPane } from "./TerminalPane.tsx";

const EMPTY_FILES: ClaudeState["filesByProject"][string] = [];
const EMPTY_SESSIONS: ClaudeState["sessionsByProject"][string] = [];

export function ClaudeHome({
  state,
  onConnect,
  onBackToCodex,
}: {
  state: ClaudeState;
  onConnect: () => void;
  onBackToCodex?: () => void;
}) {
  const active = state.projects.find((project) => project.id === state.activeProjectId) ?? null;
  const sync = active ? state.syncByProject[active.id] : null;
  const files = active ? (state.filesByProject[active.id] ?? EMPTY_FILES) : EMPTY_FILES;
  const sessions = active ? (state.sessionsByProject[active.id] ?? EMPTY_SESSIONS) : EMPTY_SESSIONS;
  const activeSessionId = active ? (state.activeSessionByProject[active.id] ?? sessions[0]?.id ?? null) : null;
  const phase = active ? state.workspacePhaseByProject[active.id] : undefined;
  const login = active ? state.loginByHost[active.host] : undefined;
  const cli = active ? state.cliByHost[active.host] : undefined;
  const loginExpired = phase === "ready" && login?.phase === "expired";
  const [askingId, setAskingId] = useState<string | null>(null);

  useEffect(() => {
    if (!active) return;
    void activateClaudeProject(active.id);
  }, [active?.id]);

  useEffect(() => {
    if (!active) return;
    void refreshClaudeFiles(active.id);
    void refreshClaudeConflicts(active.id);
  }, [active?.id]);

  useEffect(() => {
    if (!active || phase !== "ready") return;
    const current = getClaudeState().sessionsByProject[active.id] ?? [];
    if (current.length === 0) {
      dispatchClaude({ type: "open-session", projectId: active.id, sessionId: crypto.randomUUID() });
    }
  }, [active?.id, phase]);

  useEffect(() => {
    if (!active || !loginExpired) return;
    if (login?.loginUrl) return;
    void beginHostLogin(active.id);
  }, [active?.id, loginExpired, login?.loginUrl]);

  const selectProject = (projectId: string) => {
    if (projectId === state.activeProjectId) return;
    dispatchClaude({ type: "select-project", projectId });
  };

  const closeSession = (sessionId: string) => {
    if (!active) return;
    const remaining = sessions.filter((session) => session.id !== sessionId);
    const nextSessionId = remaining[0]?.id ?? crypto.randomUUID();
    dispatchClaude({
      type: "close-session",
      projectId: active.id,
      sessionId,
      nextSessionId,
    });
    void closeClaudeProjectChat(active.id, sessionId);
    if (remaining.length === 0) {
      dispatchClaude({ type: "open-session", projectId: active.id, sessionId: nextSessionId });
    }
    setAskingId(null);
  };

  const light = phase !== "ready";

  return (
    <div className="lumio-claude-frame">
      <ProjectRail
        projects={state.projects}
        activeProjectId={state.activeProjectId}
        syncByProject={state.syncByProject}
        sessionsByProject={state.sessionsByProject}
        collapsedHosts={state.collapsedHosts}
        cliByHost={state.cliByHost}
        loginByHost={state.loginByHost}
        onSelectProject={selectProject}
        onNewProject={(host) => {
          onConnect();
          dispatchClaude({ type: "draft-updated", draft: { host } });
        }}
        onConnectServer={onConnect}
      />
      <div className={`lumio-claude-mid${light ? " is-light" : ""}`}>
        {phase === "ready" && active ? (
          <SessionTabs
            sessions={sessions}
            activeSessionId={activeSessionId}
            askingId={askingId}
            onSelect={(sessionId) => {
              dispatchClaude({ type: "select-session", projectId: active.id, sessionId });
            }}
            onNew={() => {
              dispatchClaude({
                type: "open-session",
                projectId: active.id,
                sessionId: crypto.randomUUID(),
              });
            }}
            onClose={closeSession}
            onAskClose={setAskingId}
            onConfirmClose={() => {
              if (askingId) closeSession(askingId);
            }}
            onCancelClose={() => setAskingId(null)}
          />
        ) : null}
        <div className="lumio-claude-mid-body">
          <CenterPane
            active={active}
            phase={phase}
            sync={sync}
            cli={cli}
            login={login}
            sessions={sessions}
            activeSessionId={activeSessionId}
            onBackToCodex={onBackToCodex}
          />
          {active && loginExpired ? (
            <LoginCard
              layout="overlay"
              loginUrl={login?.loginUrl ?? null}
              claudeVersion={cli?.version ?? undefined}
              onOpenBrowser={() => {
                void openLoginBrowser(active.id, login?.loginUrl ?? null);
              }}
              onCopyLink={() => {
                void copyLoginUrl(active.id, login?.loginUrl ?? null);
              }}
              onSubmitCode={(code) => {
                void completeHostLogin(active.id, code);
              }}
            />
          ) : null}
        </div>
      </div>
      {active ? (
        <FileExplorer files={files} project={active} />
      ) : (
        <aside className="lumio-claude-files lumio-claude-fx">
          <header className="lumio-claude-fx-head">
            <h3>文件</h3>
          </header>
          <p className="lumio-claude-fx-empty">挑一个项目后这里会列出文件。</p>
        </aside>
      )}
      <StatusBar active={active} sync={sync} />
      <StatusDrawer state={state} />
    </div>
  );
}

function CenterPane({
  active,
  phase,
  sync,
  cli,
  login,
  sessions,
  activeSessionId,
  onBackToCodex,
}: {
  active: ClaudeProject | null;
  phase: ClaudeState["workspacePhaseByProject"][string] | undefined;
  sync: ClaudeState["syncByProject"][string] | null | undefined;
  cli: ClaudeCliInstallStatus | undefined;
  login: ClaudeLoginStatus | undefined;
  sessions: ClaudeState["sessionsByProject"][string];
  activeSessionId: string | null;
  onBackToCodex?: () => void;
}) {
  if (!active) return <PickProjectHint />;
  if (phase === "init") {
    const init = initChecklistProps(active, sync, cli, login);
    return (
      <InitChecklist
        {...init}
        onRetryInstall={() => void continueClaudeInit(active.id)}
        onRetrySync={() => void continueClaudeInit(active.id)}
        onRetryConnect={() => void continueClaudeInit(active.id)}
        onSwitchToCodex={onBackToCodex}
        onOpenHelp={() => void openInBrowser(HELP_URL)}
        onStartChat={() => {
          if ((getClaudeState().sessionsByProject[active.id] ?? []).length === 0) {
            dispatchClaude({ type: "open-session", projectId: active.id, sessionId: crypto.randomUUID() });
          }
          dispatchClaude({ type: "set-workspace-phase", projectId: active.id, phase: "ready" });
        }}
        login={{
          loginUrl: login?.loginUrl ?? null,
          claudeVersion: cli?.version ?? undefined,
          onOpenBrowser: () => {
            void openLoginBrowser(active.id, login?.loginUrl ?? null);
          },
          onCopyLink: () => {
            void copyLoginUrl(active.id, login?.loginUrl ?? null);
          },
          onSubmitCode: (code) => {
            void completeHostLogin(active.id, code);
          },
        }}
      />
    );
  }
  if (phase === "offline") {
    return (
      <OfflineCard
        host={active.host}
        localRoot={active.localRoot}
        errorCode={sync?.errorCode}
        onRetryConnect={() => void activateClaudeProject(active.id)}
        onViewLocalFiles={() => void refreshClaudeFiles(active.id)}
      />
    );
  }
  if (phase === "ready") {
    return (
      <>
        {sessions.map((session) => (
          <TerminalPane
            hidden={session.id !== activeSessionId}
            key={session.id}
            project={active}
            sessionId={session.id}
          />
        ))}
      </>
    );
  }
  return (
    <ResumeProgress
      projectName={active.name}
      hostLabel={`${active.user}@${active.host}`}
      steps={resumeSteps(sync)}
      onPeek={() => {
        dispatchClaude({ type: "set-workspace-phase", projectId: active.id, phase: "ready" });
      }}
    />
  );
}

function initChecklistProps(
  project: ClaudeProject,
  sync: ClaudeState["syncByProject"][string] | null | undefined,
  cli: ClaudeCliInstallStatus | undefined,
  login: ClaudeLoginStatus | undefined,
): {
  phase: InitPhase;
  hostLabel: string;
  claudeVersion?: string;
  filesDone?: number;
  filesTotal?: number;
  failedStep?: InitStepKey;
  errorCode?: string | null;
  failDetail?: string;
  installDetail?: string;
  localRoot: string;
  steps: Partial<Record<InitStepKey, ChecklistStep>>;
} {
  const hostLabel = `${project.user}@${project.host}`;
  const cliPhase = cli?.phase;
  const loginPhase = login?.phase;
  let phase: InitPhase = "installing";
  let failedStep: InitStepKey | undefined;
  if (cliPhase === "fail") {
    phase = "fail";
    failedStep = "install";
  } else if (cliPhase === "ok" || cliPhase === "skip") {
    phase = loginPhase === "logged-in" ? "done" : "login";
  }
  const installStatus: ChecklistStep["status"] =
    cliPhase === "fail" ? "fail" : cliPhase === "ok" || cliPhase === "skip" ? "done" : "now";
  const loginStatus: ChecklistStep["status"] =
    loginPhase === "logged-in" ? "done" : phase === "login" ? "now" : "pending";
  return {
    phase,
    hostLabel,
    claudeVersion: cli?.version ?? undefined,
    filesDone: sync?.filesDone,
    filesTotal: sync?.filesTotal,
    failedStep,
    errorCode: cli?.errorCode ?? login?.errorCode,
    failDetail: cli?.detail ?? undefined,
    installDetail: cli?.detail ?? undefined,
    localRoot: project.localRoot,
    steps: {
      connect: { status: "done", detail: hostLabel },
      component: { status: "done" },
      sync: {
        status: "done",
        detail: sync?.filesTotal ? `${sync.filesTotal} 个文件` : undefined,
      },
      install: { status: installStatus, detail: cli?.detail ?? undefined },
      login: { status: loginStatus },
    },
  };
}

function resumeSteps(
  sync: ClaudeState["syncByProject"][string] | null | undefined,
): Partial<Record<ResumeStepKey, ChecklistStep>> {
  const connected = Boolean(sync && sync.state !== "idle");
  const aligned = sync?.state === "synced" || (sync?.state === "running" && (sync.filesTotal ?? 0) > 0);
  return {
    connect: { status: connected ? "done" : "now" },
    restore: { status: connected ? (aligned ? "done" : "now") : "pending" },
    align: { status: sync?.state === "synced" ? "done" : aligned ? "now" : "pending" },
  };
}

async function openLoginBrowser(projectId: string, loginUrl: string | null): Promise<void> {
  const url = loginUrl ?? (await beginHostLogin(projectId));
  if (url) void openInBrowser(url);
}

async function copyLoginUrl(projectId: string, loginUrl: string | null): Promise<void> {
  const url = loginUrl ?? (await beginHostLogin(projectId));
  if (!url) return;
  try {
    if (navigator.clipboard?.writeText) await navigator.clipboard.writeText(url);
  } catch {
    /* ignore */
  }
}
