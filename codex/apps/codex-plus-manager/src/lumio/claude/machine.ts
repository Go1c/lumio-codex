import { hasClaudeEntitlement } from "./entitlement.ts";
import { localProjectRoot, projectSlug, remoteProjectRoot } from "./paths.ts";
import { parseSshTarget } from "./ssh-target.ts";
import {
  DEFAULT_CLAUDE_PLAN_CENTS,
  type ClaudeChatSession,
  type ClaudeConnectSheet,
  type ClaudeEntitlement,
  type ClaudeEvent,
  type ClaudeHostDraft,
  type ClaudePage,
  type ClaudeProject,
  type ClaudeState,
  type ClaudeSurface,
  type ClaudeSyncStatus,
  type PersistableClaudeState,
} from "./types.ts";

export const CONNECT_STEPS = ["host", "probe", "setup", "sync"] as const;

export function resolveClaudeSurface(input: {
  entitlement: Pick<ClaudeEntitlement, "status"> | null;
  projectCount: number;
  sheetOpen: boolean;
  controlUnreachable?: boolean;
}): ClaudeSurface {
  if (
    input.controlUnreachable &&
    input.projectCount > 0 &&
    !hasClaudeEntitlement(input.entitlement)
  ) {
    return "workspace";
  }
  if (!hasClaudeEntitlement(input.entitlement)) return "subscribe";
  if (input.sheetOpen) return "connect";
  if (input.projectCount > 0) return "workspace";
  return "empty";
}

function idleSync(): ClaudeSyncStatus {
  return { state: "idle", filesDone: 0, filesTotal: 0, errorCode: null, conflicts: 0 };
}

export function emptyHostDraft(): ClaudeHostDraft {
  return {
    host: "",
    user: "root",
    port: 22,
    auth: "password",
    keyPath: "",
    hostAlias: "",
    projectName: "my-project",
    localRoot: localProjectRoot("my-project"),
    remoteRoot: remoteProjectRoot("root", "my-project"),
  };
}

export function sshFieldsForProbe(draft: ClaudeHostDraft): ClaudeHostDraft {
  if (draft.auth === "config") {
    return draft;
  }
  return {
    ...draft,
    hostAlias: "",
    keyPath: draft.auth === "key" ? draft.keyPath : "",
  };
}

function emptySheet(projectName = "my-project"): ClaudeConnectSheet {
  return {
    mode: "server",
    step: "host",
    draft: {
      ...emptyHostDraft(),
      projectName,
      localRoot: localProjectRoot(projectName),
      remoteRoot: remoteProjectRoot("root", projectName),
    },
    probeStatus: "idle",
    probe: null,
    setupStatus: "idle",
    setupProgress: null,
    setupDetail: null,
    setupErrorCode: null,
    rootChoice: null,
    componentsInstalled: false,
    sync: idleSync(),
  };
}

export function initialClaudeState(): ClaudeState {
  return {
    entitlement: { status: "none", source: "local" },
    controlUnreachable: false,
    page: "subscribe",
    sheet: null,
    projects: [],
    activeProjectId: null,
    syncByProject: {},
    terminalByProject: {},
    filesByProject: {},
    conflictsByProject: {},
    paying: false,
    payError: null,
    payMode: "balance",
    orders: [],
    ordersOpen: false,
    planAmountCents: DEFAULT_CLAUDE_PLAN_CENTS,
    sessionsByProject: {},
    activeSessionByProject: {},
    collapsedHosts: {},
    cliByHost: {},
    loginByHost: {},
    statusDrawer: "closed",
    workspacePhaseByProject: {},
  };
}

function derivePage(state: ClaudeState): ClaudePage {
  const surface = resolveClaudeSurface({
    entitlement: state.entitlement,
    projectCount: state.projects.length,
    sheetOpen: state.sheet !== null,
    controlUnreachable: state.controlUnreachable,
  });
  if (surface === "subscribe") return "subscribe";
  if (surface === "workspace" || (surface === "connect" && state.projects.length > 0)) {
    return "workspace";
  }
  return "empty";
}

function withPage(state: ClaudeState): ClaudeState {
  const page = derivePage(state);
  return state.page === page ? state : { ...state, page };
}

export function nextProjectName(existing: string[], base = "my-project"): string {
  if (!existing.includes(base)) return base;
  let n = 2;
  while (existing.includes(`${base}-${n}`)) n += 1;
  return `${base}-${n}`;
}

export function decideRemoteProjectRoot(
  desiredName: string,
  existingNames: string[],
):
  | { action: "create"; name: string }
  | { action: "choose"; existingName: string; nextName: string } {
  const name = projectSlug(desiredName);
  if (!existingNames.includes(name)) {
    return { action: "create", name };
  }
  return {
    action: "choose",
    existingName: name,
    nextName: nextProjectName(existingNames, name),
  };
}

export function createProjectFromDraft(
  draft: ClaudeHostDraft,
  id: string,
  createdAt: string,
): ClaudeProject {
  const name = draft.projectName.trim() || "my-project";
  return {
    id,
    name,
    host: draft.host.trim(),
    user: draft.user.trim() || "root",
    port: draft.port || 22,
    auth: draft.auth,
    keyPath: draft.keyPath.trim() === "" ? null : draft.keyPath.trim(),
    hostAlias: draft.hostAlias.trim() === "" ? null : draft.hostAlias.trim(),
    remoteRoot: draft.remoteRoot.trim() || remoteProjectRoot(draft.user, name),
    localRoot: draft.localRoot.trim() || localProjectRoot(name),
    createdAt,
  };
}

export function persistableClaudeState(state: ClaudeState): PersistableClaudeState {
  return {
    entitlement: state.entitlement,
    projects: state.projects,
    activeProjectId: state.activeProjectId,
  };
}

function defaultChatSession(projectId: string, sessionId: string): ClaudeChatSession {
  return {
    id: sessionId,
    projectId,
    title: null,
    titleLocked: false,
    running: false,
  };
}

export function reduceClaudeState(state: ClaudeState, event: ClaudeEvent): ClaudeState {
  switch (event.type) {
    case "entitlement-resolved":
      return withPage({
        ...state,
        entitlement: event.entitlement,
        controlUnreachable: event.controlUnreachable ?? false,
      });
    case "open-connect": {
      if (!hasClaudeEntitlement(state.entitlement)) return state;
      const projectName = nextProjectName(state.projects.map((project) => project.name));
      const sibling = event.host
        ? state.projects.find((project) => project.host === event.host)
        : undefined;
      const sheet = emptySheet(projectName);
      if (sibling) {
        sheet.mode = "project";
        sheet.draft = {
          ...sheet.draft,
          host: sibling.host,
          user: sibling.user,
          port: sibling.port,
          auth: sibling.auth,
          keyPath: sibling.keyPath ?? "",
          hostAlias: sibling.hostAlias ?? "",
          projectName,
          localRoot: localProjectRoot(projectName),
          remoteRoot: remoteProjectRoot(sibling.user, projectName),
        };
        if (event.skipHost) sheet.step = "probe";
      } else if (event.host) {
        sheet.draft = { ...sheet.draft, host: event.host };
      }
      return withPage({ ...state, sheet });
    }
    case "cancel-connect":
      return withPage({ ...state, sheet: null });
    case "draft-updated":
      if (state.sheet === null) return state;
      return {
        ...state,
        sheet: { ...state.sheet, draft: { ...state.sheet.draft, ...event.draft } },
      };
    case "ssh-pasted": {
      if (state.sheet === null) return state;
      const parsed = parseSshTarget(event.text);
      if (parsed === null) return state;
      return {
        ...state,
        sheet: {
          ...state.sheet,
          draft: {
            ...state.sheet.draft,
            host: parsed.host,
            user: parsed.user ?? state.sheet.draft.user,
            port: parsed.port ?? state.sheet.draft.port,
          },
        },
      };
    }
    case "probe-started":
      if (state.sheet === null) return state;
      return {
        ...state,
        sheet: { ...state.sheet, probeStatus: "running" },
      };
    case "probe-finished":
      if (state.sheet === null) return state;
      return {
        ...state,
        sheet: {
          ...state.sheet,
          step: "probe",
          probeStatus: event.result.ok ? "ok" : "fail",
          probe: event.result,
        },
      };
    case "back-to-host":
      if (state.sheet === null) return state;
      return { ...state, sheet: { ...state.sheet, step: "host" } };
    case "continue-setup":
      if (state.sheet === null) return state;
      return {
        ...state,
        sheet: {
          ...state.sheet,
          step: "setup",
          setupStatus: "running",
          setupProgress: {
            phase: "inspect",
            step: 1,
            total: 4,
            detail: "正在检查服务器…",
          },
          rootChoice: null,
        },
      };
    case "setup-progress":
      if (state.sheet === null || state.sheet.setupStatus !== "running") return state;
      return {
        ...state,
        sheet: {
          ...state.sheet,
          setupProgress: {
            phase: event.phase,
            step: event.step,
            total: event.total,
            detail: event.detail,
          },
        },
      };
    case "setup-choose-root":
      if (state.sheet === null) return state;
      return {
        ...state,
        sheet: {
          ...state.sheet,
          step: "setup",
          setupStatus: "choose",
          setupDetail: null,
          setupErrorCode: null,
          rootChoice: {
            existingName: event.existingName,
            existingRoot: event.existingRoot,
            nextName: event.nextName,
            nextRoot: event.nextRoot,
          },
        },
      };
    case "setup-inspected":
      if (state.sheet === null) return state;
      return {
        ...state,
        sheet: { ...state.sheet, componentsInstalled: event.componentsInstalled },
      };
    case "setup-needs-reinstall":
      if (state.sheet === null) return state;
      return {
        ...state,
        sheet: {
          ...state.sheet,
          step: "setup",
          setupStatus: "reinstall",
          setupDetail: null,
          setupErrorCode: null,
        },
      };
    case "setup-finished":
      if (state.sheet === null) return state;
      return {
        ...state,
        sheet: {
          ...state.sheet,
          setupStatus: event.ok ? "ok" : "fail",
          setupDetail: event.detail ?? event.errorCode ?? null,
          setupErrorCode: event.ok ? null : (event.errorCode ?? "SSH_PREPARE_FAILED"),
          rootChoice: event.ok ? null : state.sheet.rootChoice,
        },
      };
    case "start-sync":
      if (state.sheet === null) return state;
      if (
        state.sheet.setupStatus === "fail" ||
        state.sheet.setupStatus === "choose" ||
        state.sheet.setupStatus === "reinstall"
      ) {
        return state;
      }
      return {
        ...state,
        sheet: {
          ...state.sheet,
          step: "sync",
          sync: { ...state.sheet.sync, state: "running" },
        },
      };
    case "sync-progress":
      if (state.sheet === null) return state;
      return {
        ...state,
        sheet: {
          ...state.sheet,
          sync: {
            ...state.sheet.sync,
            state: "running",
            filesDone: event.filesDone,
            filesTotal: event.filesTotal,
          },
        },
      };
    case "sync-finished": {
      if (!event.ok) {
        if (state.sheet === null) return state;
        return {
          ...state,
          sheet: {
            ...state.sheet,
            step: "sync",
            sync: {
              ...state.sheet.sync,
              state: "fail",
              errorCode: event.errorCode ?? "SYNC_FAILED",
            },
          },
        };
      }
      const exists = state.projects.some((project) => project.id === event.project.id);
      const projects = exists
        ? state.projects.map((project) =>
            project.id === event.project.id ? event.project : project,
          )
        : [...state.projects, event.project];
      return withPage({
        ...state,
        sheet: null,
        projects,
        activeProjectId: event.project.id,
        workspacePhaseByProject: {
          ...state.workspacePhaseByProject,
          [event.project.id]: "init",
        },
        syncByProject: {
          ...state.syncByProject,
          [event.project.id]: {
            state: "synced",
            filesDone: 0,
            filesTotal: 0,
            errorCode: null,
            conflicts: 0,
          },
        },
        terminalByProject: {
          ...state.terminalByProject,
          [event.project.id]: [
            { kind: "dim", text: `BestCodex · Claude · ${event.project.name}` },
            {
              kind: "ok",
              text: `connected  ${event.project.user}@${event.project.host}`,
            },
          ],
        },
      });
    }
    case "select-project":
      if (!state.projects.some((project) => project.id === event.projectId)) return state;
      return { ...state, activeProjectId: event.projectId };
    case "append-terminal": {
      const lines = state.terminalByProject[event.projectId] ?? [];
      return {
        ...state,
        terminalByProject: {
          ...state.terminalByProject,
          [event.projectId]: [...lines, event.line],
        },
      };
    }
    case "files-loaded":
      return {
        ...state,
        filesByProject: { ...state.filesByProject, [event.projectId]: event.files },
      };
    case "conflicts-loaded":
      return {
        ...state,
        conflictsByProject: { ...state.conflictsByProject, [event.projectId]: event.conflicts },
      };
    case "project-sync-updated":
      return {
        ...state,
        syncByProject: { ...state.syncByProject, [event.projectId]: event.sync },
      };
    case "projects-hydrated":
      return withPage({
        ...state,
        projects: event.projects,
        activeProjectId: event.activeProjectId,
        entitlement: event.entitlement ?? state.entitlement,
      });
    case "pay-started":
      return { ...state, paying: true, payError: null };
    case "pay-finished":
      return { ...state, paying: false, payError: null, payMode: "balance" };
    case "pay-failed":
      return {
        ...state,
        paying: false,
        payError: event.errorCode,
        payMode: event.forceRecharge ? "recharge" : state.payMode,
      };
    case "orders-loaded":
      return { ...state, orders: event.orders };
    case "orders-toggled":
      return { ...state, ordersOpen: event.open ?? !state.ordersOpen };
    case "plan-loaded":
      return {
        ...state,
        planAmountCents: event.amountCents > 0 ? event.amountCents : DEFAULT_CLAUDE_PLAN_CENTS,
      };
    case "open-session": {
      const current = state.sessionsByProject[event.projectId] ?? [];
      const sessions = current.some((session) => session.id === event.sessionId)
        ? current
        : [...current, defaultChatSession(event.projectId, event.sessionId)];
      return {
        ...state,
        sessionsByProject: { ...state.sessionsByProject, [event.projectId]: sessions },
        activeSessionByProject: {
          ...state.activeSessionByProject,
          [event.projectId]: event.sessionId,
        },
      };
    }
    case "close-session": {
      const current = state.sessionsByProject[event.projectId] ?? [];
      let remaining = current.filter((session) => session.id !== event.sessionId);
      if (!remaining.some((session) => session.id === event.nextSessionId)) {
        remaining = [...remaining, defaultChatSession(event.projectId, event.nextSessionId)];
      }
      return {
        ...state,
        sessionsByProject: { ...state.sessionsByProject, [event.projectId]: remaining },
        activeSessionByProject: {
          ...state.activeSessionByProject,
          [event.projectId]: event.nextSessionId,
        },
      };
    }
    case "select-session":
      return {
        ...state,
        activeSessionByProject: {
          ...state.activeSessionByProject,
          [event.projectId]: event.sessionId,
        },
      };
    case "session-title-locked":
      return {
        ...state,
        sessionsByProject: {
          ...state.sessionsByProject,
          [event.projectId]: (state.sessionsByProject[event.projectId] ?? []).map((session) =>
            session.id === event.sessionId
              ? { ...session, title: event.title, titleLocked: true }
              : session,
          ),
        },
      };
    case "session-running":
      return {
        ...state,
        sessionsByProject: {
          ...state.sessionsByProject,
          [event.projectId]: (state.sessionsByProject[event.projectId] ?? []).map((session) =>
            session.id === event.sessionId ? { ...session, running: event.running } : session,
          ),
        },
      };
    case "toggle-server-group":
      return {
        ...state,
        collapsedHosts: {
          ...state.collapsedHosts,
          [event.host]: event.collapsed ?? !state.collapsedHosts[event.host],
        },
      };
    case "cli-install-progress": {
      const current = state.cliByHost[event.host];
      return {
        ...state,
        cliByHost: {
          ...state.cliByHost,
          [event.host]: {
            phase: event.phase,
            version: event.version !== undefined ? event.version : (current?.version ?? null),
            latest: event.latest !== undefined ? event.latest : (current?.latest ?? null),
            errorCode: event.errorCode !== undefined ? event.errorCode : (current?.errorCode ?? null),
            detail: event.detail !== undefined ? event.detail : (current?.detail ?? null),
          },
        },
      };
    }
    case "login-status": {
      const current = state.loginByHost[event.host];
      return {
        ...state,
        loginByHost: {
          ...state.loginByHost,
          [event.host]: {
            phase: event.phase,
            errorCode: event.errorCode !== undefined ? event.errorCode : (current?.errorCode ?? null),
            loginUrl: event.loginUrl !== undefined ? event.loginUrl : (current?.loginUrl ?? null),
          },
        },
      };
    }
    case "set-status-drawer":
      return { ...state, statusDrawer: event.pane };
    case "set-workspace-phase":
      return {
        ...state,
        workspacePhaseByProject: {
          ...state.workspacePhaseByProject,
          [event.projectId]: event.phase,
        },
      };
    default:
      return state;
  }
}
