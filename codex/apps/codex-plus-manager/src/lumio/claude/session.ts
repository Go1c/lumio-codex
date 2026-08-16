import { fetchClaudeEntitlement } from "../invoke.ts";
import type { LumioAccountSummary } from "../types.ts";
import {
  fetchClaudeEntitlementFromControlPlane,
  hasClaudeEntitlement,
  resolveClaudeEntitlement,
} from "./entitlement.ts";
import { createProjectFromDraft } from "./machine.ts";
import {
  CLAUDE_SYNC_PROGRESS_EVENT,
  firstClaudeSync,
  listClaudeConflicts,
  listClaudeFiles,
  openClaudeSystemTerminal,
  prepareClaudeRemote,
  probeClaudeConnection,
  resolveClaudeConflict,
  runClaudeRemote,
  subscribeClaudeEvent,
  type ClaudeSshArgs,
} from "./api.ts";
import type { ClaudeConflictResolution, ClaudeEntitlement, ClaudeEntitlementStatus } from "./types.ts";
import {
  dispatchClaude,
  draftPassword,
  getClaudeState,
  projectPassword,
  rememberProjectPassword,
  takeDraftPassword,
} from "./store.ts";

function asEntitlementStatus(value: unknown): ClaudeEntitlementStatus | null {
  if (value === "active" || value === "trialing" || value === "none" || value === "expired") {
    return value;
  }
  return null;
}

function sshFromDraft(): ClaudeSshArgs | null {
  const sheet = getClaudeState().sheet;
  if (sheet === null) return null;
  return {
    host: sheet.draft.host,
    user: sheet.draft.user,
    port: sheet.draft.port,
    password: draftPassword(),
    keyPath: sheet.draft.keyPath,
    hostAlias: sheet.draft.hostAlias,
    auth: sheet.draft.auth,
  };
}

let syncBridgeStarted = false;

export function ensureClaudeEngineBridge(): void {
  if (syncBridgeStarted) return;
  syncBridgeStarted = true;
  void subscribeClaudeEvent<{
    filesDone: number;
    filesTotal: number;
    projectId?: string;
  }>(CLAUDE_SYNC_PROGRESS_EVENT, (payload) => {
    const sheet = getClaudeState().sheet;
    if (sheet !== null) {
      dispatchClaude({
        type: "sync-progress",
        filesDone: payload.filesDone,
        filesTotal: payload.filesTotal,
      });
    }
    if (payload.projectId) {
      const current = getClaudeState().syncByProject[payload.projectId];
      dispatchClaude({
        type: "project-sync-updated",
        projectId: payload.projectId,
        sync: {
          state: current?.state === "conflicts" ? "conflicts" : "running",
          filesDone: payload.filesDone,
          filesTotal: payload.filesTotal,
          errorCode: current?.errorCode ?? null,
          conflicts: current?.conflicts ?? 0,
        },
      });
    }
  });
}

export async function hydrateClaudeWorkspace(account: LumioAccountSummary | null): Promise<void> {
  ensureClaudeEngineBridge();
  const current = getClaudeState();
  let remote: ClaudeEntitlement | null = null;
  let controlUnreachable = false;
  try {
    const payload = await fetchClaudeEntitlement();
    const status = asEntitlementStatus(payload?.status);
    if (status) {
      remote = { status, source: "control-plane" };
    } else {
      controlUnreachable = true;
    }
  } catch {
    controlUnreachable = true;
    remote = await fetchClaudeEntitlementFromControlPlane().catch(() => null);
    if (remote !== null) controlUnreachable = false;
  }
  const entitlement = resolveClaudeEntitlement({
    account,
    remote,
    local: current.entitlement.status === "none" ? null : current.entitlement,
  });
  dispatchClaude({
    type: "entitlement-resolved",
    entitlement,
    controlUnreachable: controlUnreachable && !hasClaudeEntitlement(entitlement),
  });
}

export function cancelClaudeConnect(): void {
  takeDraftPassword();
  dispatchClaude({ type: "cancel-connect" });
}

export async function runConnectProbe(): Promise<void> {
  const args = sshFromDraft();
  if (args === null) return;
  dispatchClaude({ type: "probe-started" });
  const result = await probeClaudeConnection(args);
  dispatchClaude({ type: "probe-finished", result });
}

export async function runConnectSetup(): Promise<void> {
  const sheet = getClaudeState().sheet;
  const args = sshFromDraft();
  if (sheet === null || args === null) return;
  dispatchClaude({ type: "continue-setup" });
  const project = createProjectFromDraft(sheet.draft, "draft", new Date().toISOString());
  const prepared = await prepareClaudeRemote({
    ...args,
    remoteRoot: project.remoteRoot,
    localRoot: project.localRoot,
  });
  dispatchClaude({
    type: "setup-finished",
    ok: prepared.ok,
    detail: prepared.detail,
    errorCode: prepared.errorCode,
  });
}

export async function runConnectSync(): Promise<void> {
  const sheet = getClaudeState().sheet;
  if (sheet === null || sheet.sync.state === "running") return;
  if (sheet.setupStatus === "fail") return;
  ensureClaudeEngineBridge();
  dispatchClaude({ type: "start-sync" });
  if (getClaudeState().sheet?.step !== "sync") return;
  const password = draftPassword();
  const project = createProjectFromDraft(
    sheet.draft,
    crypto.randomUUID(),
    new Date().toISOString(),
  );
  const synced = await firstClaudeSync({
    host: sheet.draft.host,
    user: sheet.draft.user,
    port: sheet.draft.port,
    password,
    keyPath: sheet.draft.keyPath,
    hostAlias: sheet.draft.hostAlias,
    auth: sheet.draft.auth,
    remoteRoot: project.remoteRoot,
    localRoot: project.localRoot,
    projectId: project.id,
  });
  dispatchClaude({
    type: "sync-progress",
    filesDone: synced.filesDone,
    filesTotal: synced.filesTotal,
  });
  if (password) rememberProjectPassword(project.id, password);
  dispatchClaude({
    type: "sync-finished",
    ok: synced.ok,
    project,
    errorCode: synced.ok ? null : (synced.errorCode ?? "SYNC_FAILED"),
  });
  if (synced.ok) {
    dispatchClaude({
      type: "project-sync-updated",
      projectId: project.id,
      sync: {
        state: "synced",
        filesDone: synced.filesDone,
        filesTotal: synced.filesTotal,
        errorCode: null,
        conflicts: 0,
      },
    });
  }
}

export async function refreshClaudeFiles(projectId: string): Promise<void> {
  const project = getClaudeState().projects.find((item) => item.id === projectId);
  if (!project) return;
  const trees = await listClaudeFiles({
    host: project.host,
    user: project.user,
    port: project.port,
    password: projectPassword(projectId),
    keyPath: project.keyPath,
    hostAlias: project.hostAlias,
    auth: project.auth,
    localRoot: project.localRoot,
    remoteRoot: project.remoteRoot,
  });
  dispatchClaude({ type: "files-loaded", projectId, files: [...trees.local, ...trees.remote] });
}

export async function refreshClaudeConflicts(projectId: string): Promise<void> {
  const project = getClaudeState().projects.find((item) => item.id === projectId);
  if (!project) return;
  const conflicts = await listClaudeConflicts({ projectId, localRoot: project.localRoot });
  dispatchClaude({ type: "conflicts-loaded", projectId, conflicts });
  if (conflicts.length > 0) {
    const current = getClaudeState().syncByProject[projectId];
    dispatchClaude({
      type: "project-sync-updated",
      projectId,
      sync: {
        state: "conflicts",
        filesDone: current?.filesDone ?? 0,
        filesTotal: current?.filesTotal ?? 0,
        errorCode: current?.errorCode ?? null,
        conflicts: conflicts.length,
      },
    });
  }
}

export async function resolveProjectConflict(
  projectId: string,
  conflictId: string,
  resolution: ClaudeConflictResolution,
): Promise<void> {
  const project = getClaudeState().projects.find((item) => item.id === projectId);
  if (!project) return;
  await resolveClaudeConflict({
    projectId,
    localRoot: project.localRoot,
    conflictId,
    resolution,
  });
  await refreshClaudeConflicts(projectId);
  await refreshClaudeFiles(projectId);
}

export async function openProjectSystemTerminal(projectId: string): Promise<void> {
  const project = getClaudeState().projects.find((item) => item.id === projectId);
  if (!project) return;
  await openClaudeSystemTerminal({
    host: project.host,
    user: project.user,
    port: project.port,
  });
}

export async function runProjectCommand(projectId: string, command: string): Promise<void> {
  const project = getClaudeState().projects.find((item) => item.id === projectId);
  if (!project || command.trim() === "") return;
  dispatchClaude({
    type: "append-terminal",
    projectId,
    line: { kind: "in", text: `> ${command}` },
  });
  const result = await runClaudeRemote({
    host: project.host,
    user: project.user,
    port: project.port,
    password: projectPassword(projectId),
    keyPath: project.keyPath,
    hostAlias: project.hostAlias,
    auth: project.auth,
    command,
  });
  if (result.stdout.trim() !== "") {
    dispatchClaude({
      type: "append-terminal",
      projectId,
      line: { kind: "out", text: result.stdout.trimEnd() },
    });
  }
  if (result.stderr.trim() !== "") {
    dispatchClaude({
      type: "append-terminal",
      projectId,
      line: { kind: "err", text: result.stderr.trimEnd() },
    });
  }
}
