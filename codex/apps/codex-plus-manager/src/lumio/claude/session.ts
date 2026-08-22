import {
  LumioCommandError,
  fetchClaudeEntitlement,
  fetchClaudePlan,
  listClaudeOrders,
  payClaudeWithBalance,
} from "../invoke.ts";
import type { LumioAccountSummary } from "../types.ts";
import { ACCOUNT_INSUFFICIENT_BALANCE_CODE } from "../state.ts";
import {
  entitlementFromSnapshot,
  fetchClaudeEntitlementFromControlPlane,
  hasClaudeEntitlement,
  resolveClaudeEntitlement,
} from "./entitlement.ts";
import { createProjectFromDraft, nextProjectName, sshFieldsForProbe } from "./machine.ts";
import { folderNameFromPath, remoteProjectRoot, replaceLastSegment } from "./paths.ts";
import {
  CLAUDE_CLI_PROGRESS_EVENT,
  CLAUDE_LOGIN_PROGRESS_EVENT,
  CLAUDE_PREPARE_PROGRESS_EVENT,
  CLAUDE_SYNC_PROGRESS_EVENT,
  closeClaudeChat,
  firstClaudeSync,
  inspectClaudeRemote,
  installClaudeCli,
  listClaudeChats,
  listClaudeConflicts,
  listClaudeFiles,
  loadClaudeLoginStatus,
  loadClaudeServerStatus,
  loadClaudeSessions,
  openClaudeSystemTerminal,
  prepareClaudeRemote,
  probeClaudeConnection,
  resolveClaudeConflict,
  resumeOfficialSync,
  runClaudeRemote,
  startClaudeLogin,
  submitClaudeLogin,
  subscribeClaudeEvent,
  type ClaudeSshArgs,
} from "./api.ts";
import { liveSyncStateFromProgress, reconcileSyncWithRemote, resumeSavedProjects } from "./sync-status.ts";
import type {
  ClaudeCliInstallPhase,
  ClaudeConflictResolution,
  ClaudeEntitlement,
  ClaudeEntitlementStatus,
  ClaudeLoginPhase,
  ClaudeProject,
  ClaudeServerStatus,
  ClaudeSessionsSnapshot,
  ClaudeSyncStatus,
} from "./types.ts";
import { DEFAULT_CLAUDE_PLAN_CENTS } from "./types.ts";

export { DEFAULT_CLAUDE_PLAN_CENTS };

export function formatClaudePlanYuan(cents: number): string {
  return (cents / 100).toFixed(2).replace(/\.?0+$/, "");
}

export function formatClaudeOrderYuan(cents: number): string {
  return (cents / 100).toFixed(2);
}
import {
  dispatchClaude,
  draftPassword,
  getClaudeState,
  projectPassword,
  rememberProjectPassword,
  setDraftPassword,
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
  const draft = sshFieldsForProbe(sheet.draft);
  return {
    host: draft.host,
    user: draft.user,
    port: draft.port,
    password: draft.auth === "password" ? draftPassword() : undefined,
    keyPath: draft.keyPath,
    hostAlias: draft.hostAlias,
    auth: draft.auth,
  };
}

let syncBridgeStarted = false;

export function ensureClaudeEngineBridge(): void {
  if (syncBridgeStarted) return;
  syncBridgeStarted = true;
  void subscribeClaudeEvent<{
    phase: "inspect" | "mkdir" | "upload" | "finish";
    step: number;
    total: number;
    detail: string;
  }>(CLAUDE_PREPARE_PROGRESS_EVENT, (payload) => {
    if (getClaudeState().sheet === null) return;
    dispatchClaude({
      type: "setup-progress",
      phase: payload.phase,
      step: payload.step,
      total: payload.total,
      detail: payload.detail,
    });
  });
  void subscribeClaudeEvent<{
    filesDone: number;
    filesTotal: number;
    projectId?: string;
    running?: boolean;
    errorCode?: string | null;
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
      const stopped = payload.running === false;
      dispatchClaude({
        type: "project-sync-updated",
        projectId: payload.projectId,
        sync: {
          state: liveSyncStateFromProgress({
            stopped,
            filesDone: payload.filesDone,
            filesTotal: payload.filesTotal,
            conflicts: current?.conflicts ?? 0,
            engineRunning: payload.running === true,
            previous: current?.state,
          }),
          filesDone: payload.filesDone,
          filesTotal: payload.filesTotal,
          errorCode: stopped ? (payload.errorCode ?? "SYNC_FAILED") : (current?.errorCode ?? null),
          conflicts: current?.conflicts ?? 0,
        },
      });
    }
  });
  void subscribeClaudeEvent<{
    host: string;
    phase: string;
    version?: string | null;
    latest?: string | null;
    errorCode?: string | null;
    detail?: string | null;
  }>(CLAUDE_CLI_PROGRESS_EVENT, (payload) => {
    dispatchClaude({
      type: "cli-install-progress",
      host: payload.host,
      phase: asCliPhase(payload.phase),
      version: payload.version,
      latest: payload.latest,
      errorCode: payload.errorCode,
      detail: payload.detail,
    });
  });
  void subscribeClaudeEvent<{
    host: string;
    phase: string;
    loginUrl?: string | null;
    errorCode?: string | null;
  }>(CLAUDE_LOGIN_PROGRESS_EVENT, (payload) => {
    dispatchClaude({
      type: "login-status",
      host: payload.host,
      phase: asLoginPhase(payload.phase),
      errorCode: payload.errorCode,
      loginUrl: payload.loginUrl,
    });
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
      remote = entitlementFromSnapshot(status, {
        expiresAt: payload?.expiresAt,
        daysLeft: payload?.daysLeft,
        expiringSoon: payload?.expiringSoon,
      });
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
  const plan = await fetchClaudePlan().catch(() => null);
  const amountCents = plan?.amountCents;
  dispatchClaude({
    type: "plan-loaded",
    amountCents: typeof amountCents === "number" && amountCents > 0 ? amountCents : DEFAULT_CLAUDE_PLAN_CENTS,
  });
  await loadClaudeOrders().catch(() => undefined);
  const after = getClaudeState();
  await resumeSavedProjects(after.projects, after.activeProjectId, (projectId) =>
    activateClaudeProject(projectId),
  );
}

export async function payClaudeSubscribe(account: LumioAccountSummary | null): Promise<void> {
  if (getClaudeState().paying) return;
  dispatchClaude({ type: "pay-started" });
  try {
    const paid = await payClaudeWithBalance();
    const status = asEntitlementStatus(paid.status);
    if (status) {
      dispatchClaude({
        type: "entitlement-resolved",
        entitlement: entitlementFromSnapshot(status, {
          expiresAt: paid.expiresAt,
          daysLeft: paid.daysLeft,
          expiringSoon: paid.expiringSoon,
        }),
      });
    }
    await hydrateClaudeWorkspace(account);
    dispatchClaude({ type: "pay-finished" });
  } catch (error) {
    const errorCode = error instanceof LumioCommandError ? error.errorCode : "UNKNOWN";
    dispatchClaude({
      type: "pay-failed",
      errorCode,
      forceRecharge: errorCode === ACCOUNT_INSUFFICIENT_BALANCE_CODE,
    });
    throw error;
  }
}

export async function loadClaudeOrders(): Promise<void> {
  const items = await listClaudeOrders();
  dispatchClaude({
    type: "orders-loaded",
    orders: items.map((item) => ({
      orderNo: item.orderNo,
      amountCents: item.amountCents,
      status: item.status,
      paidAt: item.paidAt ?? undefined,
      createdAt: item.createdAt,
    })),
  });
}

export function toggleClaudeOrders(): void {
  const open = !getClaudeState().ordersOpen;
  dispatchClaude({ type: "orders-toggled", open });
  if (open) {
    void loadClaudeOrders().catch(() => undefined);
  }
}

export function cancelClaudeConnect(): void {
  takeDraftPassword();
  dispatchClaude({ type: "cancel-connect" });
}

export function beginNewProjectOnHost(host: string): void {
  const sibling = getClaudeState().projects.find((project) => project.host === host);
  if (!sibling) {
    dispatchClaude({ type: "open-connect" });
    return;
  }
  const password = projectPassword(sibling.id) ?? "";
  if (password) setDraftPassword(password);
  const skipHost = sibling.auth !== "password" || Boolean(password);
  dispatchClaude({ type: "open-connect", host, skipHost });
  if (skipHost) void runConnectProbe();
}

export async function runConnectProbe(): Promise<void> {
  const sheet = getClaudeState().sheet;
  const args = sshFromDraft();
  if (sheet === null || args === null || sheet.probeStatus === "running") return;
  dispatchClaude({ type: "probe-started" });
  const result = await probeClaudeConnection(args);
  dispatchClaude({ type: "probe-finished", result });
}

export async function runConnectSetup(decision?: "use" | "create" | "reinstall"): Promise<void> {
  const sheet = getClaudeState().sheet;
  const args = sshFromDraft();
  if (sheet === null || args === null) return;
  ensureClaudeEngineBridge();

  let draft = sheet.draft;
  if (decision === "create" && sheet.rootChoice) {
    draft = {
      ...draft,
      projectName: sheet.rootChoice.nextName,
      remoteRoot: sheet.rootChoice.nextRoot,
    };
    dispatchClaude({
      type: "draft-updated",
      draft: { projectName: draft.projectName, remoteRoot: draft.remoteRoot },
    });
  }

  dispatchClaude({ type: "continue-setup" });

  if (decision === undefined) {
    const remoteRoot = draft.remoteRoot.trim() || remoteProjectRoot(draft.user, draft.projectName);
    const inspected = await inspectClaudeRemote({ ...args, remoteRoot });
    if (!inspected.ok) {
      dispatchClaude({
        type: "setup-finished",
        ok: false,
        detail: inspected.detail,
        errorCode: inspected.errorCode ?? "SSH_PREPARE_FAILED",
      });
      return;
    }
    dispatchClaude({
      type: "setup-inspected",
      componentsInstalled: inspected.componentsInstalled,
    });
    if (inspected.exists) {
      const existingName = folderNameFromPath(remoteRoot);
      const nextName = nextProjectName([existingName, ...inspected.names], existingName);
      dispatchClaude({
        type: "setup-choose-root",
        existingName,
        existingRoot: remoteRoot,
        nextName,
        nextRoot: replaceLastSegment(remoteRoot, nextName),
      });
      return;
    }
  }

  const installed =
    decision === "reinstall" ? false : getClaudeState().sheet?.componentsInstalled === true;
  if (installed) {
    dispatchClaude({ type: "setup-needs-reinstall" });
    return;
  }

  const project = createProjectFromDraft(draft, "draft", new Date().toISOString());
  const prepared = await prepareClaudeRemote({
    ...args,
    remoteRoot: project.remoteRoot,
    localRoot: project.localRoot,
    replace: decision === "reinstall",
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
  if (
    sheet.setupStatus === "fail" ||
    sheet.setupStatus === "choose" ||
    sheet.setupStatus === "reinstall"
  ) {
    return;
  }
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
    void continueClaudeInit(project.id);
  }
}

function sshFromProject(project: ClaudeProject): ClaudeSshArgs {
  return {
    host: project.host,
    user: project.user,
    port: project.port,
    password: projectPassword(project.id),
    keyPath: project.keyPath,
    hostAlias: project.hostAlias,
    auth: project.auth,
  };
}

export async function resumeClaudeSync(projectId: string): Promise<void> {
  const project = getClaudeState().projects.find((item) => item.id === projectId);
  if (!project) return;
  const result = await resumeOfficialSync({
    ...sshFromProject(project),
    remoteRoot: project.remoteRoot,
    localRoot: project.localRoot,
    projectId: project.id,
  });
  const after = getClaudeState().syncByProject[projectId];
  const filesDone = result.filesDone || (after?.filesDone ?? 0);
  const filesTotal = result.filesTotal || (after?.filesTotal ?? 0);
  const conflicts = after?.conflicts ?? 0;
  if (!result.ok || !result.running) {
    dispatchClaude({
      type: "project-sync-updated",
      projectId,
      sync: {
        state: "fail",
        filesDone,
        filesTotal,
        errorCode: result.errorCode ?? "SYNC_FAILED",
        conflicts,
      },
    });
    return;
  }
  dispatchClaude({
    type: "project-sync-updated",
    projectId,
    sync: {
      state: liveSyncStateFromProgress({
        stopped: false,
        filesDone,
        filesTotal,
        conflicts,
        engineRunning: true,
        previous: after?.state,
      }),
      filesDone,
      filesTotal,
      errorCode: null,
      conflicts,
    },
  });
  applyRemoteSyncHealth(projectId, await fetchClaudeServerStatus(projectId));
}

export async function reinstallWorkspaceSync(projectId: string): Promise<void> {
  const project = getClaudeState().projects.find((item) => item.id === projectId);
  if (!project) return;
  const prepared = await prepareClaudeRemote({
    ...sshFromProject(project),
    remoteRoot: project.remoteRoot,
    localRoot: project.localRoot,
    replace: true,
  });
  if (!prepared.ok) {
    const after = getClaudeState().syncByProject[projectId];
    dispatchClaude({
      type: "project-sync-updated",
      projectId,
      sync: {
        state: "fail",
        filesDone: after?.filesDone ?? 0,
        filesTotal: after?.filesTotal ?? 0,
        errorCode: prepared.errorCode ?? "SYNC_FAILED",
        conflicts: after?.conflicts ?? 0,
      },
    });
    return;
  }
  await resumeClaudeSync(projectId);
}

export function applyRemoteSyncHealth(
  projectId: string,
  snapshot: ClaudeServerStatus | null,
): void {
  const current = getClaudeState().syncByProject[projectId];
  if (!current) return;
  const next = reconcileSyncWithRemote(current, snapshot);
  if (!next || syncStatusUnchanged(current, next)) return;
  dispatchClaude({ type: "project-sync-updated", projectId, sync: next });
}

function syncStatusUnchanged(left: ClaudeSyncStatus, right: ClaudeSyncStatus): boolean {
  return (
    left.state === right.state &&
    left.errorCode === right.errorCode &&
    left.filesDone === right.filesDone &&
    left.filesTotal === right.filesTotal &&
    left.conflicts === right.conflicts
  );
}

export async function fetchClaudeServerStatus(projectId: string): Promise<ClaudeServerStatus | null> {
  const project = getClaudeState().projects.find((item) => item.id === projectId);
  if (!project) return null;
  return loadClaudeServerStatus({
    ...sshFromProject(project),
    projectId: project.id,
    remoteRoot: project.remoteRoot,
  });
}

export async function fetchClaudeSessions(projectId: string): Promise<ClaudeSessionsSnapshot | null> {
  const project = getClaudeState().projects.find((item) => item.id === projectId);
  if (!project) return null;
  return loadClaudeSessions({
    ...sshFromProject(project),
    projectId: project.id,
  });
}

export async function refreshClaudeFiles(projectId: string): Promise<void> {
  const project = getClaudeState().projects.find((item) => item.id === projectId);
  if (!project) return;
  const trees = await listClaudeFiles({
    projectId,
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

const activating = new Set<string>();

function asCliPhase(value: string): ClaudeCliInstallPhase {
  if (
    value === "idle" ||
    value === "detect" ||
    value === "install" ||
    value === "upgrade" ||
    value === "skip" ||
    value === "ok" ||
    value === "fail"
  ) {
    return value;
  }
  return "idle";
}

export function finalizeCliInstallPhase(ok: boolean, phase: string): ClaudeCliInstallPhase {
  if (!ok) return "fail";
  const normalized = asCliPhase(phase);
  if (normalized === "install" || normalized === "detect" || normalized === "idle") {
    return "ok";
  }
  return normalized;
}

export function isCliInstallFinished(phase: string | undefined): boolean {
  return phase === "ok" || phase === "skip" || phase === "upgrade";
}

function isWorkspaceOfflineSyncError(code: string | null): boolean {
  return (
    code === "SSH_UNREACHABLE" ||
    code === "SSH_AUTH_FAILED" ||
    code === "SSH_CLIENT_MISSING" ||
    code === "SSH_HOST_REQUIRED" ||
    code === "SSH_ALIAS_UNKNOWN"
  );
}

function asLoginPhase(value: string): ClaudeLoginPhase {
  if (
    value === "unknown" ||
    value === "logged-out" ||
    value === "logging-in" ||
    value === "logged-in" ||
    value === "expired"
  ) {
    return value;
  }
  return "unknown";
}

async function restoreClaudeChats(project: ClaudeProject): Promise<void> {
  const ids = await listClaudeChats(project.id);
  for (const sessionId of ids) {
    dispatchClaude({ type: "open-session", projectId: project.id, sessionId });
  }
}

export async function ensureHostCli(projectId: string): Promise<void> {
  const project = getClaudeState().projects.find((item) => item.id === projectId);
  if (!project) return;
  dispatchClaude({
    type: "cli-install-progress",
    host: project.host,
    phase: "detect",
  });
  const result = await installClaudeCli(sshFromProject(project));
  dispatchClaude({
    type: "cli-install-progress",
    host: project.host,
    phase: finalizeCliInstallPhase(result.ok, result.phase),
    version: result.version,
    latest: result.latest,
    errorCode: result.errorCode,
    detail: result.detail,
  });
}

export async function refreshHostLogin(projectId: string): Promise<void> {
  const project = getClaudeState().projects.find((item) => item.id === projectId);
  if (!project) return;
  const result = await loadClaudeLoginStatus(sshFromProject(project));
  dispatchClaude({
    type: "login-status",
    host: project.host,
    phase: asLoginPhase(result.phase),
    errorCode: result.errorCode,
  });
}

export async function beginHostLogin(projectId: string): Promise<string | null> {
  const project = getClaudeState().projects.find((item) => item.id === projectId);
  if (!project) return null;
  dispatchClaude({
    type: "login-status",
    host: project.host,
    phase: "logging-in",
    errorCode: null,
  });
  const result = await startClaudeLogin(sshFromProject(project));
  const phase: ClaudeLoginPhase = result.ok
    ? "logging-in"
    : result.errorCode === "CLAUDE_LOGIN_EXPIRED"
      ? "expired"
      : "logged-out";
  dispatchClaude({
    type: "login-status",
    host: project.host,
    phase,
    errorCode: result.errorCode,
    loginUrl: result.loginUrl,
  });
  return result.loginUrl;
}

export async function completeHostLogin(projectId: string, code: string): Promise<boolean> {
  const project = getClaudeState().projects.find((item) => item.id === projectId);
  if (!project) return false;
  const result = await submitClaudeLogin({ ...sshFromProject(project), code });
  dispatchClaude({
    type: "login-status",
    host: project.host,
    phase: result.ok ? "logged-in" : "logged-out",
    errorCode: result.errorCode,
  });
  return result.ok;
}

export async function continueClaudeInit(projectId: string): Promise<void> {
  const project = getClaudeState().projects.find((item) => item.id === projectId);
  if (!project) return;
  const gate = `init:${projectId}`;
  if (activating.has(gate)) return;
  activating.add(gate);
  try {
    dispatchClaude({ type: "set-workspace-phase", projectId, phase: "init" });
    await ensureHostCli(projectId);
    const cli = getClaudeState().cliByHost[project.host];
    if (cli?.phase === "fail") return;
    await refreshHostLogin(projectId);
    const login = getClaudeState().loginByHost[project.host];
    if (login?.phase === "logged-in") return;
    await beginHostLogin(projectId);
  } finally {
    activating.delete(gate);
  }
}

export async function closeClaudeProjectChat(projectId: string, sessionId: string): Promise<void> {
  const project = getClaudeState().projects.find((item) => item.id === projectId);
  if (!project) return;
  await closeClaudeChat({ ...sshFromProject(project), projectId, sessionId });
  dispatchClaude({ type: "session-running", projectId, sessionId, running: false });
}

export async function activateClaudeProject(projectId: string): Promise<void> {
  const project = getClaudeState().projects.find((item) => item.id === projectId);
  if (!project) return;
  if (activating.has(projectId)) return;
  activating.add(projectId);
  try {
    const currentPhase = getClaudeState().workspacePhaseByProject[projectId];
    if (currentPhase === "init") {
      await continueClaudeInit(projectId);
      return;
    }
    if (currentPhase === "ready") {
      await resumeClaudeSync(projectId);
      await refreshClaudeFiles(projectId);
      return;
    }
    dispatchClaude({ type: "set-workspace-phase", projectId, phase: "resume" });
    await resumeClaudeSync(projectId);
    const sync = getClaudeState().syncByProject[projectId];
    if (sync?.state === "fail" && isWorkspaceOfflineSyncError(sync.errorCode)) {
      dispatchClaude({ type: "set-workspace-phase", projectId, phase: "offline" });
      await refreshClaudeFiles(projectId);
      return;
    }
    await restoreClaudeChats(project);
    await refreshClaudeFiles(projectId);
    await refreshClaudeConflicts(projectId);
    void ensureHostCli(projectId).then(() => refreshHostLogin(projectId));
    const sessions = getClaudeState().sessionsByProject[projectId] ?? [];
    if (sessions.length === 0) {
      dispatchClaude({ type: "open-session", projectId, sessionId: crypto.randomUUID() });
    }
    dispatchClaude({ type: "set-workspace-phase", projectId, phase: "ready" });
  } catch {
    dispatchClaude({ type: "set-workspace-phase", projectId, phase: "offline" });
    await refreshClaudeFiles(projectId).catch(() => undefined);
  } finally {
    activating.delete(projectId);
  }
}
