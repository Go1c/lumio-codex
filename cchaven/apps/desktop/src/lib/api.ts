import {
  createInvokeDiagnosticsClient,
  type DiagnosticsClient,
} from "./diagnosticsApi";
import {
  createRemoteMonitorClient,
  type RemoteMonitorClient,
} from "./remoteMonitorApi";
import type {
  AppInfo,
  Conflict,
  ConflictControlIdentity,
  ConflictResolutionOperationView,
  CredentialCleanupStatus,
  CredentialRollbackStatus,
  DeployStage,
  DeploymentPreview,
  DeploymentRequest,
  FileNode,
  FilePreview,
  HeartbeatResponse,
  LoginStarted,
  ProbeResult,
  ProjectConfig,
  ProjectPresets,
  ProvisionCredentialRequest,
  Resolution,
  ResolutionReceipt,
  RestoreOutcome,
  ServerConfig,
  SessionView,
  SshHost,
  SshTarget,
  SyncEngineStatus,
  SyncStatus,
  TrashTicket,
  WorkspaceAccessRequest,
} from "./types";

export type EntryKindArg = "file" | "directory";

/** Everything the UI is allowed to ask of the backend. */
export interface Api {
  appInfo(): Promise<AppInfo>;

  beginLogin(): Promise<LoginStarted>;
  reopenBrowser(): Promise<string>;
  cancelLogin(): Promise<void>;
  submitManualCode(code: string): Promise<SessionView>;
  restoreSession(): Promise<RestoreOutcome>;
  logout(): Promise<void>;
  heartbeat(): Promise<HeartbeatResponse>;
  openExternal(url: string): Promise<void>;

  listProjects(): Promise<ProjectConfig[]>;
  saveProject(config: ProjectConfig, password?: string): Promise<ProjectConfig>;
  deleteProject(projectId: string): Promise<void>;
  projectPresets(name: string, user: string): Promise<ProjectPresets>;
  sshHosts(): Promise<SshHost[]>;
  parsePastedTarget(text: string): Promise<SshTarget | null>;
  testConnection(server: ServerConfig, password?: string): Promise<ProbeResult>;

  // Managed remote deployment: preview first, then write, always cancellable.
  previewDeployment(request: DeploymentRequest): Promise<DeploymentPreview>;
  executeDeployment(previewId: string, request: DeploymentRequest): Promise<void>;
  cancelDeployment(projectId: string): Promise<boolean>;
  provisionCredential(request: ProvisionCredentialRequest): Promise<void>;
  probeWorkspaceAccess(request: WorkspaceAccessRequest): Promise<void>;
  cancelProvisioning(projectId: string): Promise<CredentialRollbackStatus>;
  credentialCleanupStatus(projectId: string): Promise<CredentialCleanupStatus>;

  listFiles(projectId: string): Promise<FileNode[]>;
  recentFiles(projectId: string, limit?: number): Promise<FileNode[]>;
  readFile(projectId: string, path: string): Promise<FilePreview>;
  createEntry(
    projectId: string,
    parent: string,
    name: string,
    kind: EntryKindArg,
  ): Promise<string>;
  renameEntry(projectId: string, path: string, newName: string): Promise<string>;
  deleteEntry(projectId: string, path: string): Promise<TrashTicket>;
  undoDelete(projectId: string, token: string): Promise<string>;
  purgeDelete(token: string): Promise<void>;
  revealEntry(projectId: string, path?: string): Promise<void>;
  openEntry(projectId: string, path: string): Promise<void>;
  openLocalFolder(projectId: string): Promise<void>;

  listConflicts(projectId: string): Promise<Conflict[]>;
  /**
   * `identity` scopes the request so a reply cannot land on a conflict page
   * that has since been remounted. Omitted only where no engine session can
   * exist (mock mode).
   */
  resolveConflict(
    projectId: string,
    conflictId: string,
    resolution: Resolution,
    identity?: ConflictControlIdentity,
  ): Promise<ResolutionReceipt>;
  undoConflict(projectId: string, conflictId: string): Promise<Conflict>;
  forgetConflictUndo(projectId: string, conflictId: string): Promise<void>;
  syncStatus(projectId: string): Promise<SyncStatus>;

  startSync(projectId: string): Promise<SyncEngineStatus>;
  stopSync(projectId: string): Promise<void>;
  syncEngineStatus(projectId: string): Promise<SyncEngineStatus>;
  cancelConflictRequest(
    projectId: string,
    identity: ConflictControlIdentity,
  ): Promise<ConflictResolutionOperationView>;
  cancelConflictGeneration(
    projectId: string,
    projectGeneration: string,
  ): Promise<ConflictResolutionOperationView[]>;
  listConflictOperations(
    projectId: string,
  ): Promise<ConflictResolutionOperationView[]>;

  startTerminal(projectId: string, cols: number, rows: number): Promise<void>;
  writeTerminal(projectId: string, data: number[]): Promise<void>;
  resizeTerminal(projectId: string, cols: number, rows: number): Promise<void>;
  closeTerminal(projectId: string): Promise<void>;
  newClaudeSession(projectId: string): Promise<void>;
  closeTmuxWindow(projectId: string): Promise<void>;
  listTmuxWindows(projectId: string): Promise<void>;
  killAllSessions(projectId: string): Promise<void>;

  /** Host metrics and project-scoped Claude sessions. */
  remoteMonitor: RemoteMonitorClient;
  /** Event timeline, health, self test and support bundle export. */
  diagnostics: DiagnosticsClient;

  /** Subscribe to a backend event; resolves to an unsubscribe function. */
  on<T>(event: string, handler: (payload: T) => void): Promise<() => void>;
}

/** Normalised error: backend commands fail with either a string or an object. */
export class ApiError extends Error {
  code: string;
  stage?: DeployStage;

  constructor(message: string, code = "unknown", stage?: DeployStage) {
    super(message);
    this.name = "ApiError";
    this.code = code;
    this.stage = stage;
  }
}

export function toApiError(error: unknown): ApiError {
  if (error instanceof ApiError) return error;
  if (typeof error === "string") return new ApiError(error);
  if (error && typeof error === "object") {
    const record = error as Record<string, unknown>;
    // The agent supervisor answers with `{primary, cleanup}` rather than a
    // message; its codes are stable and are what support asks for.
    if (typeof record.primary === "string") {
      const cleanup = Array.isArray(record.cleanup)
        ? (record.cleanup as unknown[]).map(String)
        : [];
      const detail = cleanup.length ? `${record.primary}（${cleanup.join("、")}）` : record.primary;
      return new ApiError(detail, record.primary);
    }
    const message =
      typeof record.message === "string" ? record.message : "操作失败，请重试。";
    const code = typeof record.code === "string" ? record.code : "unknown";
    const stage = typeof record.stage === "string" ? (record.stage as DeployStage) : undefined;
    return new ApiError(message, code, stage);
  }
  return new ApiError("操作失败，请重试。");
}

/** The stable failure code, for log lines and support tickets. */
export function stableFailure(error: unknown): string {
  return toApiError(error).code;
}

/** True when running inside the Tauri webview rather than a plain browser. */
export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

type Invoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
type Listen = <T>(
  event: string,
  handler: (event: { payload: T }) => void,
) => Promise<() => void>;

/** Tauri-backed implementation. */
export function createTauriApi(invoke: Invoke, listen: Listen): Api {
  const call = async <T>(command: string, args?: Record<string, unknown>): Promise<T> => {
    try {
      return await invoke<T>(command, args);
    } catch (error) {
      throw toApiError(error);
    }
  };

  return {
    appInfo: () => call("app_info"),

    beginLogin: () => call("auth_begin_login"),
    reopenBrowser: () => call("auth_reopen_browser"),
    cancelLogin: () => call("auth_cancel_login"),
    submitManualCode: (code) => call("auth_submit_manual_code", { code }),
    restoreSession: () => call("auth_restore_session"),
    logout: () => call("auth_logout"),
    heartbeat: () => call("auth_heartbeat"),
    openExternal: (url) => call("open_external", { url }),

    listProjects: () => call("list_projects"),
    saveProject: (config, password) => call("save_project", { request: { config, password } }),
    deleteProject: (projectId) => call("delete_project", { projectId }),
    projectPresets: (name, user) => call("project_presets", { name, user }),
    sshHosts: () => call("parse_ssh_hosts"),
    parsePastedTarget: (text) => call("parse_pasted_target", { text }),
    testConnection: (server, password) => call("test_connection", { server, password }),

    previewDeployment: (request) => call("preview_remote_deployment", { request }),
    executeDeployment: (previewId, request) =>
      call("execute_remote_deployment", { previewId, request }),
    cancelDeployment: (projectId) => call("cancel_remote_deployment", { projectId }),
    provisionCredential: (request) => call("provision_workspace_credential", { request }),
    probeWorkspaceAccess: (request) => call("probe_workspace_access", { request }),
    cancelProvisioning: (projectId) => call("cancel_workspace_provisioning", { projectId }),
    credentialCleanupStatus: (projectId) =>
      call("workspace_credential_cleanup_status", { projectId }),

    listFiles: (projectId) => call("list_files", { projectId }),
    recentFiles: (projectId, limit) => call("recent_files", { projectId, limit }),
    readFile: (projectId, path) => call("read_file", { projectId, path }),
    createEntry: (projectId, parent, name, kind) =>
      call("create_entry", { projectId, parent, name, kind }),
    renameEntry: (projectId, path, newName) =>
      call("rename_entry", { projectId, path, newName }),
    deleteEntry: (projectId, path) => call("delete_entry", { projectId, path }),
    undoDelete: (projectId, token) => call("undo_delete", { projectId, token }),
    purgeDelete: (token) => call("purge_delete", { token }),
    revealEntry: (projectId, path) => call("reveal_entry", { projectId, path }),
    openEntry: (projectId, path) => call("open_entry", { projectId, path }),
    openLocalFolder: (projectId) => call("open_local_folder", { projectId }),

    listConflicts: (projectId) => call("list_conflicts", { projectId }),
    resolveConflict: (projectId, conflictId, resolution, identity) =>
      call("resolve_conflict", { projectId, conflictId, resolution, identity }),
    undoConflict: (projectId, conflictId) => call("undo_conflict", { projectId, conflictId }),
    forgetConflictUndo: (projectId, conflictId) =>
      call("forget_conflict_undo", { projectId, conflictId }),
    syncStatus: (projectId) => call("sync_status", { projectId }),

    startSync: (projectId) => call("start_sync", { projectId }),
    stopSync: (projectId) => call("stop_sync", { projectId }),
    syncEngineStatus: (projectId) => call("sync_engine_status", { projectId }),
    cancelConflictRequest: (projectId, identity) =>
      call("cancel_sync_conflict_request", { projectId, identity }),
    cancelConflictGeneration: (projectId, projectGeneration) =>
      call("cancel_sync_conflict_generation", { projectId, projectGeneration }),
    listConflictOperations: (projectId) =>
      call("list_sync_conflict_operations", { projectId }),

    startTerminal: (projectId, cols, rows) =>
      call("start_terminal", { request: { projectId, cols, rows } }),
    writeTerminal: (projectId, data) => call("write_terminal", { projectId, data }),
    resizeTerminal: (projectId, cols, rows) =>
      call("resize_terminal", { projectId, cols, rows }),
    closeTerminal: (projectId) => call("close_terminal", { projectId }),
    newClaudeSession: (projectId) => call("new_claude_session", { projectId }),
    closeTmuxWindow: (projectId) => call("close_tmux_window", { projectId }),
    listTmuxWindows: (projectId) => call("list_tmux_windows", { projectId }),
    killAllSessions: (projectId) => call("kill_all_sessions", { projectId }),

    remoteMonitor: createRemoteMonitorClient(call),
    diagnostics: createInvokeDiagnosticsClient(call),

    on: <T>(event: string, handler: (payload: T) => void) =>
      listen<T>(event, ({ payload }) => handler(payload)),
  };
}

/** Backend event names, mirrored from Rust. */
export const EVENTS = {
  loginCompleted: "auth://login-completed",
  loginFailed: "auth://login-failed",
  deployProgress: "deploy://progress",
  terminalOutput: (projectId: string) => `terminal-output-${projectId}`,
  terminalClosed: (projectId: string) => `terminal-closed-${projectId}`,
} as const;
