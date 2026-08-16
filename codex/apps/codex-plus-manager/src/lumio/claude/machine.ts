import { hasClaudeEntitlement } from "./entitlement.ts";
import { localProjectRoot, remoteProjectRoot } from "./paths.ts";
import { parseSshTarget } from "./ssh-target.ts";
import type {
  ClaudeConnectSheet,
  ClaudeEntitlement,
  ClaudeEvent,
  ClaudeHostDraft,
  ClaudePage,
  ClaudeProject,
  ClaudeState,
  ClaudeSurface,
  ClaudeSyncStatus,
  PersistableClaudeState,
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
  };
}

function emptySheet(projectName = "my-project"): ClaudeConnectSheet {
  return {
    step: "host",
    draft: { ...emptyHostDraft(), projectName },
    probeStatus: "idle",
    probe: null,
    setupStatus: "idle",
    setupDetail: null,
    setupErrorCode: null,
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
    stageTab: "terminal",
    syncByProject: {},
    terminalByProject: {},
    filesByProject: {},
    conflictsByProject: {},
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
    remoteRoot: remoteProjectRoot(draft.user, name),
    localRoot: localProjectRoot(name),
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
      return withPage({ ...state, sheet: emptySheet(projectName) });
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
        sheet: { ...state.sheet, step: "probe", probeStatus: "running" },
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
        sheet: { ...state.sheet, step: "setup", setupStatus: "running" },
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
        },
      };
    case "start-sync":
      if (state.sheet === null) return state;
      if (state.sheet.setupStatus === "fail") return state;
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
        stageTab: "terminal",
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
    case "set-stage-tab":
      return { ...state, stageTab: event.tab };
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
    default:
      return state;
  }
}
