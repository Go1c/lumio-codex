import { fetchClaudeEntitlement } from "../invoke.ts";
import type { LumioAccountSummary } from "../types.ts";
import {
  fetchClaudeEntitlementFromControlPlane,
  hasClaudeEntitlement,
  resolveClaudeEntitlement,
} from "./entitlement.ts";
import { createProjectFromDraft } from "./machine.ts";
import type { ClaudeEntitlement, ClaudeEntitlementStatus } from "./types.ts";
import {
  firstClaudeSync,
  listClaudeLocalFiles,
  openClaudeSystemTerminal,
  prepareClaudeRemote,
  probeClaudeConnection,
  runClaudeRemote,
} from "./api.ts";
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

export async function hydrateClaudeWorkspace(account: LumioAccountSummary | null): Promise<void> {
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
  const sheet = getClaudeState().sheet;
  if (sheet === null) return;
  dispatchClaude({ type: "probe-started" });
  const result = await probeClaudeConnection({
    host: sheet.draft.host,
    user: sheet.draft.user,
    port: sheet.draft.port,
    password: draftPassword(),
    keyPath: sheet.draft.keyPath,
    auth: sheet.draft.auth,
  });
  dispatchClaude({ type: "probe-finished", result });
}

export async function runConnectSetup(): Promise<void> {
  const sheet = getClaudeState().sheet;
  if (sheet === null) return;
  dispatchClaude({ type: "continue-setup" });
  const project = createProjectFromDraft(sheet.draft, "draft", new Date().toISOString());
  const prepared = await prepareClaudeRemote({
    host: sheet.draft.host,
    user: sheet.draft.user,
    port: sheet.draft.port,
    password: draftPassword(),
    keyPath: sheet.draft.keyPath,
    auth: sheet.draft.auth,
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
  dispatchClaude({ type: "start-sync" });
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
    auth: sheet.draft.auth,
    remoteRoot: project.remoteRoot,
    localRoot: project.localRoot,
  });
  dispatchClaude({
    type: "sync-progress",
    filesDone: synced.filesDone,
    filesTotal: synced.filesTotal,
  });
  if (password) rememberProjectPassword(project.id, password);
  const keepProject = synced.ok || synced.errorCode === "SYNC_ENGINE_UNAVAILABLE";
  dispatchClaude({
    type: "sync-finished",
    ok: keepProject,
    project,
    errorCode: keepProject ? null : (synced.errorCode ?? "SYNC_FAILED"),
  });
  if (keepProject && !synced.ok) {
    dispatchClaude({
      type: "project-sync-updated",
      projectId: project.id,
      sync: {
        state: "offline",
        filesDone: synced.filesDone,
        filesTotal: synced.filesTotal,
        errorCode: "SYNC_ENGINE_UNAVAILABLE",
        conflicts: 0,
      },
    });
  }
  dispatchClaude({
    type: "append-terminal",
    projectId: project.id,
    line: {
      kind: synced.ok ? "ok" : "dim",
      text: synced.ok
        ? "本机目录已就绪。完整双向同步还没接到这个启动器。"
        : "本机目录已创建。同步组件未接通，可先在系统终端里继续。",
    },
  });
}

export async function refreshClaudeFiles(projectId: string): Promise<void> {
  const project = getClaudeState().projects.find((item) => item.id === projectId);
  if (!project) return;
  const files = await listClaudeLocalFiles(project.localRoot);
  dispatchClaude({ type: "files-loaded", projectId, files });
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
