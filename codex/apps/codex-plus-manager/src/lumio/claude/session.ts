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
  CLAUDE_PREPARE_PROGRESS_EVENT,
  CLAUDE_SYNC_PROGRESS_EVENT,
  firstClaudeSync,
  inspectClaudeRemote,
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

export async function runConnectProbe(): Promise<void> {
  const sheet = getClaudeState().sheet;
  const args = sshFromDraft();
  if (sheet === null || args === null || sheet.probeStatus === "running") return;
  dispatchClaude({ type: "probe-started" });
  const result = await probeClaudeConnection(args);
  dispatchClaude({ type: "probe-finished", result });
}

export async function runConnectSetup(decision?: "use" | "create"): Promise<void> {
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

  const project = createProjectFromDraft(draft, "draft", new Date().toISOString());
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
  if (sheet.setupStatus === "fail" || sheet.setupStatus === "choose") return;
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
