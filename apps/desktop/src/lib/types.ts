/** Shapes shared with the Tauri backend (`apps/desktop/src-tauri/src`). */

export type EntitlementStatus = "active" | "trialing" | "none";

export interface Entitlement {
  status: EntitlementStatus;
  kind?: string;
  expiresAt?: string | null;
  daysLeft: number;
  bonusDaysTotal?: number;
  expiringSoon?: boolean;
}

export interface Activation {
  trialGranted: boolean;
  trialExpiresAt?: string | null;
  trialDeniedReuse?: boolean;
  inviterBonusDays?: number;
}

export interface SessionView {
  email: string;
  entitlement?: Entitlement | null;
  activation?: Activation | null;
}

export type RestoreOutcome =
  | { state: "signedOut"; message?: string | null }
  | { state: "signedIn"; session: SessionView }
  | { state: "offline"; message: string };

export interface LoginStarted {
  authorizeUrl: string;
  redirectUri: string;
}

export interface LoginFailure {
  code: string;
  message: string;
  network: boolean;
}

export interface Notice {
  type: string;
  daysLeft: number;
}

export interface HeartbeatResponse {
  entitlement: Entitlement;
  notices: Notice[];
}

export interface ControlError {
  code: string;
  message: string;
  status?: number;
}

export interface ExternalLinks {
  account: string;
  invite: string;
  docs: string;
  support: string;
  serverGuide: string;
  troubleshooting: string;
}

export interface AppInfo {
  version: string;
  mockControl: boolean;
  links: ExternalLinks;
}

export type AuthMethod = "password" | "key" | "ssh_config";

export interface ServerConfig {
  host: string;
  user: string;
  port: number;
  auth: AuthMethod;
  keyPath?: string | null;
  configAlias?: string | null;
}

export interface SyncConfig {
  mode: "two_way_safe";
  includes: string[];
  excludes: string[];
  /** Always true — M3 ships no switch for this (硬性要求). */
  protectSecrets: true;
}

export interface ProjectConfig {
  id: string;
  name: string;
  server: ServerConfig;
  remoteRoot: string;
  localRoot: string;
  workspaceId: string;
  tmuxSession: string;
  sync: SyncConfig;
  createdAt: string;
}

export interface ProjectPresets {
  remoteRoot: string;
  localRoot: string;
  tmuxSession: string;
}

export interface SshHost {
  alias: string;
  hostname: string | null;
  port: number | null;
  user: string | null;
}

export interface SshTarget {
  host: string;
  user?: string | null;
  port?: number | null;
}

export type ProbeFailure = "unreachable" | "not_ssh" | "auth" | "host_key" | "other";

export interface ProbeResult {
  ok: boolean;
  distro?: string | null;
  failure?: ProbeFailure | null;
  detail?: string | null;
}

export type DeployStage = "connect" | "installAgent" | "createDirectory" | "firstSync";
export type StageState = "pending" | "running" | "done" | "failed";

export const DEPLOY_STAGES: DeployStage[] = [
  "connect",
  "installAgent",
  "createDirectory",
  "firstSync",
];

export interface StageUpdate {
  projectId: string;
  stage: DeployStage;
  state: Exclude<StageState, "pending">;
  detail?: string;
  error?: string;
}

export interface DeployError {
  stage: DeployStage;
  message: string;
}

export type NodeKind = "directory" | "file" | "symlink";

export interface FileNode {
  name: string;
  path: string;
  kind: NodeKind;
  size?: number;
  modifiedMs?: number;
  children?: FileNode[];
}

export interface FilePreview {
  path: string;
  size: number;
  modifiedMs?: number;
  content: string;
  tooLarge: boolean;
  binary: boolean;
}

export interface TrashTicket {
  token: string;
  path: string;
  name: string;
}

export type ConflictKind = "bothModified" | "remoteDeleted" | "localDeleted";

export interface ConflictSide {
  content: string;
  modifiedMs: number;
  deleted?: boolean;
}

export interface Conflict {
  id: string;
  path: string;
  kind: ConflictKind;
  kindLabel: string;
  detectedAtMs: number;
  local: ConflictSide;
  remote: ConflictSide;
}

export type Resolution = "keepLocal" | "keepRemote" | "keepBoth";

export interface ResolutionReceipt {
  conflictId: string;
  path: string;
  resolution: Resolution;
  label: string;
  copyPath?: string;
  remaining: number;
}

/** 6.3 全局唯一语义 — the only sync states that exist anywhere in the app. */
export type SyncState = "synced" | "syncing" | "conflicts" | "offline";

export interface SyncStatus {
  state: SyncState;
  conflicts: number;
  pending: number;
  /** Seconds until the next reconnect attempt while offline (6.3 ladder). */
  retryInSeconds?: number;
  /** Stable, non-sensitive reason the session is down. */
  detail?: string;
}
