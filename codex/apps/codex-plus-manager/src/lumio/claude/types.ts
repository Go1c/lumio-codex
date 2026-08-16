export type ClaudeSurface = "subscribe" | "empty" | "connect" | "workspace";

export type ClaudePage = "subscribe" | "empty" | "workspace";

export type ClaudeConnectStep = "host" | "probe" | "setup" | "sync";

export type ClaudeStageTab = "terminal" | "files" | "conflicts";

export type ClaudeEntitlementStatus = "none" | "active" | "trialing" | "expired";

export type ClaudeEntitlementSource = "account" | "control-plane" | "local";

export type ClaudeAuthMethod = "password" | "key";

export interface ClaudeEntitlement {
  status: ClaudeEntitlementStatus;
  source: ClaudeEntitlementSource;
  expiresAt?: string | null;
}

export interface ClaudeHostDraft {
  host: string;
  user: string;
  port: number;
  auth: ClaudeAuthMethod;
  keyPath: string;
  projectName: string;
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

export type ClaudeSetupStatus = "idle" | "running" | "ok" | "fail";

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
  setupDetail: string | null;
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
}

export interface ClaudeConflictEntry {
  id: string;
  path: string;
  kindLabel: string;
}

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
    };
