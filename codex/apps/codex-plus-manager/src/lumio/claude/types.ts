export type ClaudeSurface = "subscribe" | "empty" | "connect" | "workspace";

export type ClaudePage = "subscribe" | "empty" | "workspace";

export type ClaudeConnectStep = "host" | "probe" | "setup" | "sync";

export type ClaudeStageTab = "terminal" | "files" | "conflicts" | "server" | "sessions";

export type ClaudeEntitlementStatus = "none" | "active" | "trialing" | "expired";

export type ClaudeEntitlementSource = "account" | "control-plane" | "local";

export type ClaudeAuthMethod = "password" | "key" | "config";

export interface ClaudeEntitlement {
  status: ClaudeEntitlementStatus;
  source: ClaudeEntitlementSource;
  expiresAt?: string | null;
  daysLeft?: number | null;
  expiringSoon?: boolean | null;
}

/** 套餐拉取失败时回落 19.9 元（1990 分），禁止回落 68。 */
export const DEFAULT_CLAUDE_PLAN_CENTS = 1990;

export type ClaudePayMode = "balance" | "recharge";

export interface ClaudeOrder {
  orderNo: string;
  amountCents: number;
  status: string;
  paidAt?: string;
  createdAt: string;
}

export interface ClaudeHostDraft {
  host: string;
  user: string;
  port: number;
  auth: ClaudeAuthMethod;
  keyPath: string;
  hostAlias: string;
  projectName: string;
  localRoot: string;
  remoteRoot: string;
}

export type ClaudeProbeStatus = "idle" | "running" | "ok" | "fail";

export interface ClaudeProbeResult {
  ok: boolean;
  reachable: boolean;
  authenticated: boolean;
  target: string;
  user: string;
  distro: string | null;
  cpu: string | null;
  memory: string | null;
  errorCode: string | null;
  detail: string | null;
}

export type ClaudeSetupStatus = "idle" | "running" | "ok" | "fail" | "choose";

export type ClaudeSetupPhase = "inspect" | "mkdir" | "upload" | "finish";

export interface ClaudeSetupProgress {
  phase: ClaudeSetupPhase;
  step: number;
  total: number;
  detail: string;
}

export interface ClaudeRootChoice {
  existingName: string;
  existingRoot: string;
  nextName: string;
  nextRoot: string;
}

export type ClaudeSyncPhase = "idle" | "running" | "ok" | "fail" | "synced" | "conflicts" | "offline";

export interface ClaudeSyncStatus {
  state: ClaudeSyncPhase;
  filesDone: number;
  filesTotal: number;
  errorCode: string | null;
  conflicts: number;
}

export interface ClaudeProject {
  id: string;
  name: string;
  host: string;
  user: string;
  port: number;
  auth: ClaudeAuthMethod;
  keyPath: string | null;
  hostAlias: string | null;
  remoteRoot: string;
  localRoot: string;
  createdAt: string;
}

export interface ClaudeConnectSheet {
  step: ClaudeConnectStep;
  draft: ClaudeHostDraft;
  probeStatus: ClaudeProbeStatus;
  probe: ClaudeProbeResult | null;
  setupStatus: ClaudeSetupStatus;
  setupProgress: ClaudeSetupProgress | null;
  setupDetail: string | null;
  setupErrorCode: string | null;
  rootChoice: ClaudeRootChoice | null;
  sync: ClaudeSyncStatus;
}

export interface ClaudeTerminalLine {
  kind: "dim" | "ok" | "out" | "err" | "in";
  text: string;
}

export interface ClaudeFileEntry {
  path: string;
  name: string;
  kind: "file" | "directory";
  side?: "local" | "remote";
  size?: number | null;
  fingerprint?: string | null;
  children?: ClaudeFileEntry[];
}

export interface ClaudeFilePreview {
  path: string;
  side: "local" | "remote";
  content: string;
  tooLarge: boolean;
  binary: boolean;
}

export interface ClaudeConflictEntry {
  id: string;
  path: string;
  kindLabel: string;
  localContent?: string;
  remoteContent?: string;
  canResolve?: boolean;
}

export interface ClaudeConflictDiff {
  path: string;
  local: string;
  remote: string;
}

export type ClaudeConflictResolution = "keepLocal" | "keepRemote" | "keepBoth";

export interface ClaudeSshHost {
  alias: string;
  hostname: string | null;
  port: number | null;
  user: string | null;
}

export interface ClaudeResumeResult {
  ok: boolean;
  running: boolean;
  filesDone: number;
  filesTotal: number;
  errorCode: string | null;
}

export interface ClaudeStatusError {
  code: string;
  message: string;
}

export interface ClaudeServerDisk {
  mount: string;
  totalBytes: number;
  usedBytes: number;
  usedPercent: number;
}

export interface ClaudeServerService {
  key: string;
  displayName: string;
  running: boolean;
  processCount: number;
  cpuPercent: number;
  memoryRssBytes: number;
}

export interface ClaudeServerStatus {
  projectId: string;
  capturedAt: string;
  ok: boolean;
  error?: ClaudeStatusError | null;
  host?: {
    hostname?: string | null;
    uptimeSeconds?: number | null;
    cpu: { usagePercent: number; load1?: number | null; cores?: number | null };
    memory: { totalBytes: number; usedBytes: number; usedPercent: number };
    disks: ClaudeServerDisk[];
  } | null;
  services?: {
    items: ClaudeServerService[];
  } | null;
}

export interface ClaudeSessionWindow {
  index: number;
  id: string;
  title: string;
  active: boolean;
}

export interface ClaudeSessionsSnapshot {
  projectId: string;
  capturedAt: string;
  ok: boolean;
  sessionExists: boolean;
  windows: ClaudeSessionWindow[];
  error?: ClaudeStatusError | null;
}

export interface ClaudeChatSession {
  id: string;
  projectId: string;
  title: string | null; // null 表示显示「新对话」
  titleLocked: boolean;
  running: boolean;
}

export type ClaudeCliInstallPhase =
  | "idle"
  | "detect"
  | "install"
  | "upgrade"
  | "skip"
  | "ok"
  | "fail";

export interface ClaudeCliInstallStatus {
  phase: ClaudeCliInstallPhase;
  version: string | null;
  latest: string | null;
  errorCode: string | null;
  detail: string | null;
}

export type ClaudeLoginPhase =
  | "unknown"
  | "logged-out"
  | "logging-in"
  | "logged-in"
  | "expired";

export interface ClaudeLoginStatus {
  phase: ClaudeLoginPhase;
  errorCode: string | null;
}

export type ClaudeStatusDrawerPane = "closed" | "server" | "sessions" | "conflicts";

export type ClaudeWorkspacePhase = "init" | "ready" | "resume" | "offline";

export interface ClaudeState {
  entitlement: ClaudeEntitlement;
  controlUnreachable: boolean;
  page: ClaudePage;
  sheet: ClaudeConnectSheet | null;
  projects: ClaudeProject[];
  activeProjectId: string | null;
  stageTab: ClaudeStageTab;
  syncByProject: Record<string, ClaudeSyncStatus>;
  terminalByProject: Record<string, ClaudeTerminalLine[]>;
  filesByProject: Record<string, ClaudeFileEntry[]>;
  conflictsByProject: Record<string, ClaudeConflictEntry[]>;
  paying: boolean;
  payError: string | null;
  payMode: ClaudePayMode;
  orders: ClaudeOrder[];
  ordersOpen: boolean;
  planAmountCents: number;
  sessionsByProject: Record<string, ClaudeChatSession[]>;
  activeSessionByProject: Record<string, string | null>;
  collapsedHosts: Record<string, boolean>;
  cliByHost: Record<string, ClaudeCliInstallStatus>;
  loginByHost: Record<string, ClaudeLoginStatus>;
  statusDrawer: ClaudeStatusDrawerPane;
  workspacePhaseByProject: Record<string, ClaudeWorkspacePhase>;
}

export interface PersistableClaudeState {
  entitlement: ClaudeEntitlement;
  projects: ClaudeProject[];
  activeProjectId: string | null;
}

export type ClaudeEvent =
  | { type: "entitlement-resolved"; entitlement: ClaudeEntitlement; controlUnreachable?: boolean }
  | { type: "open-connect" }
  | { type: "cancel-connect" }
  | { type: "draft-updated"; draft: Partial<ClaudeHostDraft> }
  | { type: "ssh-pasted"; text: string }
  | { type: "probe-started" }
  | { type: "probe-finished"; result: ClaudeProbeResult }
  | { type: "back-to-host" }
  | { type: "continue-setup" }
  | {
      type: "setup-progress";
      phase: ClaudeSetupPhase;
      step: number;
      total: number;
      detail: string;
    }
  | {
      type: "setup-choose-root";
      existingName: string;
      existingRoot: string;
      nextName: string;
      nextRoot: string;
    }
  | { type: "setup-finished"; ok: boolean; detail?: string | null; errorCode?: string | null }
  | { type: "start-sync" }
  | { type: "sync-progress"; filesDone: number; filesTotal: number }
  | { type: "sync-finished"; ok: boolean; project: ClaudeProject; errorCode?: string | null }
  | { type: "select-project"; projectId: string }
  | { type: "set-stage-tab"; tab: ClaudeStageTab }
  | { type: "append-terminal"; projectId: string; line: ClaudeTerminalLine }
  | { type: "files-loaded"; projectId: string; files: ClaudeFileEntry[] }
  | { type: "conflicts-loaded"; projectId: string; conflicts: ClaudeConflictEntry[] }
  | { type: "project-sync-updated"; projectId: string; sync: ClaudeSyncStatus }
  | {
      type: "projects-hydrated";
      projects: ClaudeProject[];
      activeProjectId: string | null;
      entitlement?: ClaudeEntitlement;
    }
  | { type: "pay-started" }
  | { type: "pay-finished" }
  | { type: "pay-failed"; errorCode: string; forceRecharge?: boolean }
  | { type: "orders-loaded"; orders: ClaudeOrder[] }
  | { type: "orders-toggled"; open?: boolean }
  | { type: "plan-loaded"; amountCents: number }
  | { type: "open-session"; projectId: string; sessionId: string }
  | { type: "close-session"; projectId: string; sessionId: string; nextSessionId: string }
  | { type: "select-session"; projectId: string; sessionId: string }
  | { type: "session-title-locked"; projectId: string; sessionId: string; title: string }
  | { type: "session-running"; projectId: string; sessionId: string; running: boolean }
  | { type: "toggle-server-group"; host: string }
  | {
      type: "cli-install-progress";
      host: string;
      phase: ClaudeCliInstallPhase;
      version?: string | null;
      latest?: string | null;
      errorCode?: string | null;
      detail?: string | null;
    }
  | { type: "login-status"; host: string; phase: ClaudeLoginPhase; errorCode?: string | null }
  | { type: "set-status-drawer"; pane: ClaudeStatusDrawerPane }
  | { type: "set-workspace-phase"; projectId: string; phase: ClaudeWorkspacePhase };
