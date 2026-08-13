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

// --- Deployment (managed, ten-step, previewable and cancellable) ---

/** The ten steps `deploy.rs` plans, in execution order. */
export type DeployStep =
  | "validate_remote"
  | "ensure_directories"
  | "upload_server"
  | "upload_agent"
  | "verify_artifacts"
  | "prepare_configuration"
  | "switch_version"
  | "install_services"
  | "start_services"
  | "verify_health";

export type StageState = "pending" | "running" | "succeeded" | "failed";

/** Kept for the legacy four-stage error shape still returned by some paths. */
export type DeployStage = DeployStep;

export interface DeployArtifact {
  kind: string;
  sha256: string;
  bytes: number;
}

/** Read-only plan: nothing has been written to the server yet. */
export interface DeploymentPreview {
  previewId: string;
  target: string;
  version: string;
  serviceManager: "system" | "user" | null;
  existingVersion: string | null;
  artifacts: DeployArtifact[];
  steps: DeployStep[];
  /** Blocking conditions; a preview with warnings must not be executed. */
  warnings: string[];
}

export interface DeploymentRequest {
  projectId: string;
  sshHostAlias: string;
  workspaceId: string;
  remoteRoot: string;
  includes: string[];
  excludes: string[];
  protectSecrets: true;
}

/** One `deploy://progress` event. */
export interface DeployProgress {
  projectId: string;
  step: DeployStep;
  status: "running" | "succeeded" | "failed";
  errorCode: string | null;
}

export interface DeployError {
  stage: DeployStage;
  message: string;
}

export interface CredentialCleanupStatus {
  active: boolean;
  pendingAgentDeletion: boolean;
  pendingRevocation: boolean;
  pendingTunnelCleanup: boolean;
  lastError: string | null;
}

export interface CredentialRollbackStatus extends CredentialCleanupStatus {
  credentialDeleted: boolean;
}

export interface ProvisionCredentialRequest {
  projectId: string;
  sshHostAlias: string;
  username: string;
  password: string;
}

export interface WorkspaceAccessRequest {
  projectId: string;
  sshHostAlias: string;
  workspaceId: string;
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
  /** False while the engine is still settling this conflict. */
  canResolve?: boolean;
  /** A choice already on its way to the server, if any. */
  pendingResolution?: string | null;
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
  /**
   * Seconds until the next reconnect attempt while offline (6.3 ladder).
   *
   * Only set when a real deadline is known. The agent publishes an attempt
   * counter rather than a deadline, so this is currently absent and the bar
   * reads plain 「离线」 instead of inventing a countdown.
   */
  retryInSeconds?: number;
  /** Stable, non-sensitive reason the session is down. */
  detail?: string;
}

// --- Sync engine control surface ---

/** Structured failure from the agent supervisor: one primary, N cleanup codes. */
export interface SyncFailure {
  primary: string;
  cleanup: string[];
}

/** Raw session state, as opposed to the reduced 6.3 label above. */
export interface SyncEngineStatus {
  running: boolean;
  localPort: number | null;
  message: string;
  error: SyncFailure | null;
}

/**
 * Idempotency key for one conflict resolution. `projectGeneration` scopes it to
 * a single mount of the conflict page so a stale reply cannot land on a fresh
 * one; see `ConflictRequestScope`.
 */
export interface ConflictControlIdentity {
  requestId: string;
  projectGeneration: string;
}

export type ConflictOperationPhase =
  | "pending"
  | "dispatched"
  | "queued"
  | "failed"
  | "cancelled";

export interface ConflictResolutionOperationView {
  requestId: string;
  projectGeneration: string;
  conflictId: string;
  conflictRevision: number;
  choice: "current" | "incoming" | "merged" | "delete";
  phase: ConflictOperationPhase;
  receipt: { status: string; operationId: string } | null;
  error: string | null;
}
