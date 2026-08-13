import { ApiError, EVENTS, type Api, type EntryKindArg } from "./api";
import {
  createMemoryDiagnosticsClient,
  type DiagnosticsClient,
  type MemoryDiagnosticsSeed,
} from "./diagnosticsApi";
import {
  createMockRemoteMonitorClient,
  type MockRemoteMonitorSeed,
} from "./mockRemoteMonitor";
import type { RemoteMonitorClient } from "./remoteMonitorApi";
import type {
  AppInfo,
  Conflict,
  ConflictControlIdentity,
  ConflictResolutionOperationView,
  CredentialCleanupStatus,
  CredentialRollbackStatus,
  DeployProgress,
  DeployStep,
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

/** The ten managed deployment steps, in the order `deploy.rs` runs them. */
export const DEPLOY_STEPS: DeployStep[] = [
  "validate_remote",
  "ensure_directories",
  "upload_server",
  "upload_agent",
  "verify_artifacts",
  "prepare_configuration",
  "switch_version",
  "install_services",
  "start_services",
  "verify_health",
];

/**
 * In-memory stand-in for the Tauri backend.
 *
 * Used by `npm run dev` in a plain browser and by every frontend test, so the
 * UI can be built and verified without a control plane or a real server. It
 * deliberately mirrors the Rust command surface, including its error strings.
 */

export interface MockOptions {
  /** Start already authenticated. */
  signedIn?: boolean;
  /** Refresh token present but the network is down (offline mode). */
  offline?: boolean;
  /** Days left on the subscription; ≤3 drives the expiry banner. */
  daysLeft?: number;
  trialing?: boolean;
  projects?: ProjectConfig[];
  /** Fail connection tests and deployments, for exercising error states. */
  failConnection?: boolean;
  /** Index into the ten deployment steps at which execution should fail. */
  failDeployAtStage?: number;
  /** Blocking preview warnings, so the "cannot deploy yet" path is reachable. */
  deployWarnings?: string[];
  remoteMonitor?: MockRemoteMonitorSeed;
  diagnostics?: MemoryDiagnosticsSeed;
  /** Resolve browser authorization automatically after this many ms. */
  authDelayMs?: number;
  /** Make browser authorization time out instead of succeeding. */
  authTimesOut?: boolean;
  conflicts?: Conflict[];
  files?: FileNode[];
  invited?: boolean;
}

const DAY = 86_400_000;

export function sampleProject(overrides: Partial<ProjectConfig> = {}): ProjectConfig {
  return {
    id: "project-1",
    name: "my-project",
    server: { host: "43.156.20.8", user: "root", port: 22, auth: "password" },
    remoteRoot: "/root/cchaven/my-project",
    localRoot: "/Users/mary/CCHaven/my-project",
    workspaceId: "workspace-1",
    tmuxSession: "cchaven-my-project",
    sync: {
      mode: "two_way_safe",
      includes: ["**"],
      excludes: [".git/", "node_modules/", "target/", ".env"],
      protectSecrets: true,
    },
    createdAt: "1",
    ...overrides,
  };
}

export function sampleFiles(now = Date.now()): FileNode[] {
  return [
    {
      name: "src",
      path: "src",
      kind: "directory",
      modifiedMs: now - 2 * 60_000,
      children: [
        {
          name: "engine.rs",
          path: "src/engine.rs",
          kind: "file",
          size: 18_842,
          modifiedMs: now - 2 * 60_000,
        },
        {
          name: "lib.rs",
          path: "src/lib.rs",
          kind: "file",
          size: 1_204,
          modifiedMs: now - 26 * 3_600_000,
        },
      ],
    },
    {
      name: "Cargo.toml",
      path: "Cargo.toml",
      kind: "file",
      size: 812,
      modifiedMs: now - 14 * 60_000,
    },
    {
      name: "README.md",
      path: "README.md",
      kind: "file",
      size: 2_310,
      modifiedMs: now - 7 * DAY,
    },
  ];
}

export function sampleConflicts(now = Date.now()): Conflict[] {
  return [
    {
      id: "conflict-engine",
      path: "src/engine.rs",
      kind: "bothModified",
      kindLabel: "本地与远端同时修改",
      detectedAtMs: now - 2 * 60_000,
      local: {
        content: "pub struct WriteBatcher {\n    pending: Vec<Mutation>,\n    max_batch: usize,\n}\n",
        modifiedMs: now - 2 * 60_000,
      },
      remote: {
        content: "pub struct WriteBatcher {\n    queue: VecDeque<Mutation>,\n}\n",
        modifiedMs: now - 3 * 60_000,
      },
    },
    {
      id: "conflict-cargo",
      path: "Cargo.toml",
      kind: "remoteDeleted",
      kindLabel: "远端已删除，本地已修改",
      detectedAtMs: now - 14 * 60_000,
      local: { content: "[package]\nname = \"sync-engine\"\n", modifiedMs: now - 14 * 60_000 },
      remote: { content: "", modifiedMs: now - 15 * 60_000, deleted: true },
    },
  ];
}

interface Listener {
  event: string;
  handler: (payload: unknown) => void;
}

export class MockApi implements Api {
  readonly calls: string[] = [];
  readonly opened: string[] = [];
  private listeners: Listener[] = [];
  private projects: ProjectConfig[];
  private conflicts: Conflict[];
  private files: FileNode[];
  private previews = new Map<string, string>();
  private trash = new Map<string, { path: string; name: string }>();
  private undoStack = new Map<string, Conflict>();
  private signedIn: boolean;
  private options: MockOptions;
  private pendingLogin = false;
  private deploymentCancelled = false;
  private provisioned = new Set<string>();
  private engineRunning = new Set<string>();
  readonly remoteMonitor: RemoteMonitorClient;
  readonly diagnostics: DiagnosticsClient;

  constructor(options: MockOptions = {}) {
    this.options = options;
    this.signedIn = options.signedIn ?? false;
    this.projects = options.projects ? [...options.projects] : [];
    this.conflicts = options.conflicts ? [...options.conflicts] : [];
    this.files = options.files ?? sampleFiles();
    this.remoteMonitor = createMockRemoteMonitorClient(options.remoteMonitor);
    this.diagnostics = createMemoryDiagnosticsClient(options.diagnostics);
    this.previews.set("src/engine.rs", "pub struct WriteBatcher {\n    pending: Vec<Mutation>,\n}\n");
    this.previews.set("Cargo.toml", '[package]\nname = "sync-engine"\n');
    this.previews.set("README.md", "# my-project\n");
  }

  private record(name: string): void {
    this.calls.push(name);
  }

  emit(event: string, payload: unknown): void {
    for (const listener of this.listeners) {
      if (listener.event === event) listener.handler(payload);
    }
  }

  async on<T>(event: string, handler: (payload: T) => void): Promise<() => void> {
    const listener: Listener = { event, handler: handler as (payload: unknown) => void };
    this.listeners.push(listener);
    return () => {
      this.listeners = this.listeners.filter((entry) => entry !== listener);
    };
  }

  // --- app ---

  async appInfo(): Promise<AppInfo> {
    return {
      version: "0.1.0",
      mockControl: true,
      links: {
        account: "https://cchaven.cn/account",
        invite: "https://cchaven.cn/account#invite",
        docs: "https://cchaven.cn/docs",
        support: "https://cchaven.cn/support",
        serverGuide: "https://cchaven.cn/docs/buy-a-server",
        troubleshooting: "https://cchaven.cn/docs/connection-troubleshooting",
      },
    };
  }

  // --- account ---

  private session(): SessionView {
    const daysLeft = this.options.daysLeft ?? 23;
    return {
      email: "mary@example.com",
      entitlement: {
        status: this.options.trialing === false ? "active" : "trialing",
        kind: this.options.trialing === false ? "monthly" : "trial",
        expiresAt: new Date(Date.now() + daysLeft * DAY).toISOString(),
        daysLeft,
        expiringSoon: daysLeft <= 3,
      },
    };
  }

  async beginLogin(): Promise<LoginStarted> {
    this.record("beginLogin");
    this.pendingLogin = true;
    const started: LoginStarted = {
      authorizeUrl: "https://cchaven.cn/authorize?client_id=cchaven-desktop",
      redirectUri: "http://127.0.0.1:53682/callback",
    };
    this.opened.push(started.authorizeUrl);

    const delay = this.options.authDelayMs;
    if (delay !== undefined) {
      setTimeout(() => this.finishBrowserLogin(), delay);
    }
    return started;
  }

  /** Simulate the browser half of the flow finishing. */
  finishBrowserLogin(): void {
    if (!this.pendingLogin) return;
    this.pendingLogin = false;
    if (this.options.authTimesOut) {
      this.emit(EVENTS.loginFailed, {
        code: "timeout",
        message: "等待授权超时。浏览器可能没有打开，或你尚未在浏览器中完成登录。",
        network: false,
      });
      return;
    }
    this.signedIn = true;
    const session = this.session();
    if (this.options.invited) {
      session.activation = {
        trialGranted: true,
        trialExpiresAt: new Date(Date.now() + 30 * DAY).toISOString(),
      };
    }
    this.emit(EVENTS.loginCompleted, session);
  }

  /** Simulate the loopback listener failing with a network error. */
  failBrowserLogin(message = "无法连接服务器。", network = true): void {
    this.pendingLogin = false;
    this.emit(EVENTS.loginFailed, { code: network ? "network" : "denied", message, network });
  }

  async reopenBrowser(): Promise<string> {
    this.record("reopenBrowser");
    const url = "https://cchaven.cn/authorize?client_id=cchaven-desktop";
    this.opened.push(url);
    return url;
  }

  async cancelLogin(): Promise<void> {
    this.record("cancelLogin");
    this.pendingLogin = false;
  }

  async submitManualCode(code: string): Promise<SessionView> {
    this.record("submitManualCode");
    if (!code.trim()) throw new ApiError("请粘贴浏览器中显示的授权码。", "invalid_request");
    if (code === "invalid") throw new ApiError("授权码无效或已过期，请重新登录。", "invalid_grant");
    this.signedIn = true;
    return this.session();
  }

  async restoreSession(): Promise<RestoreOutcome> {
    this.record("restoreSession");
    if (this.options.offline) {
      return { state: "offline", message: "无法连接服务器。（mock 离线模式）" };
    }
    if (!this.signedIn) return { state: "signedOut" };
    return { state: "signedIn", session: this.session() };
  }

  async logout(): Promise<void> {
    this.record("logout");
    this.signedIn = false;
  }

  async heartbeat(): Promise<HeartbeatResponse> {
    this.record("heartbeat");
    const entitlement = this.session().entitlement!;
    return {
      entitlement,
      notices: entitlement.expiringSoon
        ? [{ type: "expiring_soon", daysLeft: entitlement.daysLeft }]
        : [],
    };
  }

  async openExternal(url: string): Promise<void> {
    this.record("openExternal");
    this.opened.push(url);
  }

  // --- projects ---

  async listProjects(): Promise<ProjectConfig[]> {
    return [...this.projects];
  }

  async saveProject(config: ProjectConfig, password?: string): Promise<ProjectConfig> {
    this.record(password ? "saveProject+password" : "saveProject");
    const index = this.projects.findIndex((project) => project.id === config.id);
    if (index >= 0) this.projects[index] = config;
    else this.projects.push(config);
    return config;
  }

  async deleteProject(projectId: string): Promise<void> {
    this.record("deleteProject");
    this.projects = this.projects.filter((project) => project.id !== projectId);
  }

  async projectPresets(name: string, user: string): Promise<ProjectPresets> {
    const slug = name.trim().replace(/\s+/g, "-") || "my-project";
    const base = !user || user === "root" ? "/root" : `/home/${user}`;
    return {
      remoteRoot: `${base}/cchaven/${slug}`,
      localRoot: `/Users/mary/CCHaven/${slug}`,
      tmuxSession: `cchaven-${slug}`,
    };
  }

  async sshHosts(): Promise<SshHost[]> {
    return [{ alias: "prod", hostname: "10.0.0.1", port: 22, user: "deploy" }];
  }

  async parsePastedTarget(text: string): Promise<SshTarget | null> {
    const trimmed = text.trim();
    const command = trimmed.match(/^ssh\s+(?:-p\s*(\d+)\s+)?(?:([\w.-]+)@)?([\w.-]+)(?:\s+-p\s*(\d+))?$/i);
    if (command) {
      return {
        host: command[3],
        user: command[2] ?? null,
        port: Number(command[1] ?? command[4]) || null,
      };
    }
    const userHost = trimmed.match(/^([\w.-]+)@([\w.-]+)$/);
    if (userHost) return { host: userHost[2], user: userHost[1], port: null };
    if (/^[\w.-]+$/.test(trimmed) && /[a-zA-Z0-9]/.test(trimmed)) {
      return { host: trimmed, user: null, port: null };
    }
    return null;
  }

  async testConnection(_server: ServerConfig, _password?: string): Promise<ProbeResult> {
    this.record("testConnection");
    if (this.options.failConnection) {
      return { ok: false, failure: "auth", detail: "Permission denied" };
    }
    return { ok: true, distro: "Ubuntu 24.04.1 LTS" };
  }

  // --- managed deployment ---

  async previewDeployment(request: DeploymentRequest): Promise<DeploymentPreview> {
    this.record("previewDeployment");
    return {
      previewId: `preview-${request.projectId}`,
      target: request.sshHostAlias,
      version: "0.1.0-mock",
      serviceManager: "system",
      existingVersion: null,
      artifacts: [
        { kind: "server", sha256: "a".repeat(64), bytes: 18_432_000 },
        { kind: "agent", sha256: "b".repeat(64), bytes: 9_216_000 },
      ],
      steps: [...DEPLOY_STEPS],
      warnings: this.options.deployWarnings ?? [],
    };
  }

  async executeDeployment(previewId: string, request: DeploymentRequest): Promise<void> {
    this.record(`executeDeployment:${previewId}`);
    this.deploymentCancelled = false;
    for (const [index, step] of DEPLOY_STEPS.entries()) {
      if (this.deploymentCancelled) throw new ApiError("部署已取消。", "cancelled");
      this.emit(EVENTS.deployProgress, {
        projectId: request.projectId,
        step,
        status: "running",
        errorCode: null,
      } satisfies DeployProgress);

      if (this.options.failDeployAtStage === index) {
        this.emit(EVENTS.deployProgress, {
          projectId: request.projectId,
          step,
          status: "failed",
          errorCode: "insufficient_disk",
        } satisfies DeployProgress);
        throw new ApiError("服务器磁盘空间不足。请清理磁盘后重试。", "insufficient_disk", step);
      }
      this.emit(EVENTS.deployProgress, {
        projectId: request.projectId,
        step,
        status: "succeeded",
        errorCode: null,
      } satisfies DeployProgress);
    }
  }

  async cancelDeployment(projectId: string): Promise<boolean> {
    this.record(`cancelDeployment:${projectId}`);
    this.deploymentCancelled = true;
    return true;
  }

  async provisionCredential(request: ProvisionCredentialRequest): Promise<void> {
    this.record("provisionCredential");
    if (this.options.failConnection) throw new ApiError("无法登录服务器。", "auth");
    this.provisioned.add(request.projectId);
  }

  async probeWorkspaceAccess(request: WorkspaceAccessRequest): Promise<void> {
    this.record("probeWorkspaceAccess");
    if (!this.provisioned.has(request.projectId)) {
      throw new ApiError("同步凭据尚未就绪。", "credential_missing");
    }
  }

  async cancelProvisioning(projectId: string): Promise<CredentialRollbackStatus> {
    this.record("cancelProvisioning");
    this.provisioned.delete(projectId);
    return {
      credentialDeleted: true,
      active: false,
      pendingAgentDeletion: false,
      pendingRevocation: false,
      pendingTunnelCleanup: false,
      lastError: null,
    };
  }

  async credentialCleanupStatus(_projectId: string): Promise<CredentialCleanupStatus> {
    return {
      active: false,
      pendingAgentDeletion: false,
      pendingRevocation: false,
      pendingTunnelCleanup: false,
      lastError: null,
    };
  }

  // --- files ---

  async listFiles(): Promise<FileNode[]> {
    return structuredClone(this.files);
  }

  async recentFiles(_projectId: string, limit = 6): Promise<FileNode[]> {
    const flat: FileNode[] = [];
    const walk = (nodes: FileNode[]) => {
      for (const node of nodes) {
        if (node.children) walk(node.children);
        else if (node.kind === "file") flat.push(node);
      }
    };
    walk(this.files);
    return flat.sort((a, b) => (b.modifiedMs ?? 0) - (a.modifiedMs ?? 0)).slice(0, limit);
  }

  async readFile(_projectId: string, path: string): Promise<FilePreview> {
    const content = this.previews.get(path);
    if (path.endsWith(".bin")) {
      return { path, size: 3, content: "", tooLarge: false, binary: true };
    }
    if (path.endsWith(".log")) {
      return { path, size: 2_000_000, content: "", tooLarge: true, binary: false };
    }
    return {
      path,
      size: content?.length ?? 0,
      modifiedMs: Date.now() - 60_000,
      content: content ?? "（新文件，暂无内容）",
      tooLarge: false,
      binary: false,
    };
  }

  private insert(nodes: FileNode[], parent: string, node: FileNode): boolean {
    if (parent === "") {
      nodes.push(node);
      return true;
    }
    for (const candidate of nodes) {
      if (candidate.path === parent) {
        candidate.children = [...(candidate.children ?? []), node];
        return true;
      }
      if (candidate.children && this.insert(candidate.children, parent, node)) return true;
    }
    return false;
  }

  private remove(nodes: FileNode[], path: string): FileNode | null {
    const index = nodes.findIndex((node) => node.path === path);
    if (index >= 0) return nodes.splice(index, 1)[0];
    for (const node of nodes) {
      if (node.children) {
        const removed = this.remove(node.children, path);
        if (removed) return removed;
      }
    }
    return null;
  }

  private find(nodes: FileNode[], path: string): FileNode | null {
    for (const node of nodes) {
      if (node.path === path) return node;
      if (node.children) {
        const found = this.find(node.children, path);
        if (found) return found;
      }
    }
    return null;
  }

  async createEntry(
    _projectId: string,
    parent: string,
    name: string,
    kind: EntryKindArg,
  ): Promise<string> {
    this.record("createEntry");
    const trimmed = name.trim();
    if (!trimmed) throw new ApiError("请输入名称。");
    const path = parent ? `${parent}/${trimmed}` : trimmed;
    if (this.find(this.files, path)) throw new ApiError(`「${trimmed}」已存在，请换一个名称。`);
    this.insert(this.files, parent, {
      name: trimmed,
      path,
      kind: kind === "directory" ? "directory" : "file",
      size: 0,
      modifiedMs: Date.now(),
      ...(kind === "directory" ? { children: [] } : {}),
    });
    return path;
  }

  async renameEntry(_projectId: string, path: string, newName: string): Promise<string> {
    this.record("renameEntry");
    const trimmed = newName.trim();
    if (!trimmed) throw new ApiError("请输入名称。");
    const node = this.find(this.files, path);
    if (!node) throw new ApiError("原文件已不存在，可能刚刚被同步删除。");
    const parent = path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : "";
    const next = parent ? `${parent}/${trimmed}` : trimmed;
    node.name = trimmed;
    node.path = next;
    node.modifiedMs = Date.now();
    return next;
  }

  async deleteEntry(_projectId: string, path: string): Promise<TrashTicket> {
    this.record("deleteEntry");
    const node = this.remove(this.files, path);
    if (!node) throw new ApiError("文件已不存在。");
    const token = `trash-${this.trash.size + 1}`;
    this.trash.set(token, { path, name: node.name });
    this.deleted.set(token, node);
    return { token, path, name: node.name };
  }

  private deleted = new Map<string, FileNode>();

  async undoDelete(_projectId: string, token: string): Promise<string> {
    this.record("undoDelete");
    const node = this.deleted.get(token);
    const ticket = this.trash.get(token);
    if (!node || !ticket) throw new ApiError("撤销已过期，无法恢复。");
    const parent = ticket.path.includes("/")
      ? ticket.path.slice(0, ticket.path.lastIndexOf("/"))
      : "";
    this.insert(this.files, parent, node);
    this.deleted.delete(token);
    this.trash.delete(token);
    return ticket.path;
  }

  async purgeDelete(token: string): Promise<void> {
    this.deleted.delete(token);
    this.trash.delete(token);
  }

  async revealEntry(_projectId: string, path?: string): Promise<void> {
    this.record(`revealEntry:${path ?? ""}`);
  }

  async openEntry(_projectId: string, path: string): Promise<void> {
    this.record(`openEntry:${path}`);
  }

  async openLocalFolder(): Promise<void> {
    this.record("openLocalFolder");
  }

  // --- conflicts ---

  async listConflicts(): Promise<Conflict[]> {
    return [...this.conflicts];
  }

  async resolveConflict(
    _projectId: string,
    conflictId: string,
    resolution: Resolution,
    _identity?: ConflictControlIdentity,
  ): Promise<ResolutionReceipt> {
    this.record(`resolveConflict:${resolution}`);
    const index = this.conflicts.findIndex((conflict) => conflict.id === conflictId);
    if (index < 0) throw new ApiError("该冲突已被处理。");
    const [conflict] = this.conflicts.splice(index, 1);
    this.undoStack.set(conflictId, conflict);
    const labels: Record<Resolution, string> = {
      keepLocal: "保留本地",
      keepRemote: "保留远端",
      keepBoth: "两者都保留",
    };
    return {
      conflictId,
      path: conflict.path,
      resolution,
      label: labels[resolution],
      copyPath: resolution === "keepBoth" ? `${conflict.path}.服务器版本` : undefined,
      remaining: this.conflicts.length,
    };
  }

  async undoConflict(_projectId: string, conflictId: string): Promise<Conflict> {
    this.record("undoConflict");
    const conflict = this.undoStack.get(conflictId);
    if (!conflict) throw new ApiError("撤销已过期。");
    this.undoStack.delete(conflictId);
    this.conflicts = [conflict, ...this.conflicts];
    return conflict;
  }

  async forgetConflictUndo(_projectId: string, conflictId: string): Promise<void> {
    this.undoStack.delete(conflictId);
  }

  async syncStatus(): Promise<SyncStatus> {
    if (this.options.offline) return { state: "offline", conflicts: 0, pending: 0 };
    if (this.conflicts.length > 0) {
      return { state: "conflicts", conflicts: this.conflicts.length, pending: 0 };
    }
    return { state: "synced", conflicts: 0, pending: 0 };
  }

  // --- terminal ---

  async startTerminal(projectId: string): Promise<void> {
    this.record("startTerminal");
    setTimeout(() => {
      this.emit(
        EVENTS.terminalOutput(projectId),
        Array.from(new TextEncoder().encode("cchaven ready\r\n")),
      );
    }, 0);
  }

  async writeTerminal(): Promise<void> {}
  async resizeTerminal(): Promise<void> {}
  async closeTerminal(): Promise<void> {
    this.record("closeTerminal");
  }

  async newClaudeSession(projectId: string): Promise<void> {
    this.record("newClaudeSession");
    this.emit(
      EVENTS.terminalOutput(projectId),
      Array.from(new TextEncoder().encode("claude\r\n")),
    );
  }

  async closeTmuxWindow(): Promise<void> {
    this.record("closeTmuxWindow");
  }

  async listTmuxWindows(): Promise<void> {
    this.record("listTmuxWindows");
  }

  async killAllSessions(projectId: string): Promise<void> {
    this.record("killAllSessions");
    this.emit(EVENTS.terminalClosed(projectId), null);
  }

  // --- sync engine ---

  async startSync(projectId: string): Promise<SyncEngineStatus> {
    this.record(`startSync:${projectId}`);
    this.engineRunning.add(projectId);
    return this.syncEngineStatus(projectId);
  }

  async stopSync(projectId: string): Promise<void> {
    this.record(`stopSync:${projectId}`);
    this.engineRunning.delete(projectId);
  }

  async syncEngineStatus(projectId: string): Promise<SyncEngineStatus> {
    const running = this.engineRunning.has(projectId) && !this.options.offline;
    return {
      running,
      localPort: running ? 19_050 : null,
      message: running ? "已连接到服务器上的同步代理。" : "同步未运行。",
      error: running ? null : this.options.offline ? { primary: "transport", cleanup: [] } : null,
    };
  }

  async cancelConflictRequest(
    _projectId: string,
    identity: ConflictControlIdentity,
  ): Promise<ConflictResolutionOperationView> {
    this.record("cancelConflictRequest");
    return {
      requestId: identity.requestId,
      projectGeneration: identity.projectGeneration,
      conflictId: "",
      conflictRevision: 0,
      choice: "current",
      phase: "cancelled",
      receipt: null,
      error: null,
    };
  }

  async cancelConflictGeneration(): Promise<ConflictResolutionOperationView[]> {
    this.record("cancelConflictGeneration");
    return [];
  }

  async listConflictOperations(): Promise<ConflictResolutionOperationView[]> {
    return [];
  }
}
