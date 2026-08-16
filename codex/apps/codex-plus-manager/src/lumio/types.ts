export type LumioPhase =
  | "bootstrapping"
  | "signed-out"
  | "authenticating"
  | "provisioning"
  | "ready-online"
  | "ready-offline"
  | "needs-repair";

export interface LumioAccountSummary {
  email: string;
  balance: number;
  planLabel: string | null;
}

export interface LumioCodexApp {
  path: string;
  version: string | null;
  source: "automatic" | "manual";
}

export const LUMIO_OFFICIAL_APP_INSTALL_PHASES = [
  "idle",
  "planning",
  "downloading",
  "verifying",
  "installing",
  "detecting",
  "succeeded",
  "failed",
  "cancelled",
] as const;

export type LumioOfficialAppInstallPhase = (typeof LUMIO_OFFICIAL_APP_INSTALL_PHASES)[number];

/** Ready-home install progress. Not a LumioPhase — the shell stays on ready-online/offline. */
export interface LumioOfficialAppInstall {
  phase: LumioOfficialAppInstallPhase;
  stage: string | null;
  errorCode: string | null;
  path?: string | null;
  /** 下载阶段才有值；命令层拿不到 Content-Length 时 bytesTotal 为 null。 */
  bytesDownloaded?: number | null;
  bytesTotal?: number | null;
}

export type LumioCredentialStatus = "present" | "missing" | "invalid";

export interface LumioBootstrap {
  version: string;
  platform: string;
  arch: string;
  codexApp: LumioCodexApp | null;
  account: LumioAccountSummary | null;
  telemetryEnabled: boolean;
  autoUpdateEnabled: boolean;
  /** 默认开启（opt-out）：系统现状为权威；旧包不带该字段按「关」显示。 */
  launchAtLoginEnabled?: boolean;
  // Optional while the command layer lands: the shell must stay usable against a
  // bootstrap payload that predates the credential probe.
  credentialStatus?: LumioCredentialStatus;
}

export interface LumioLaunchAtLoginResult {
  enabled: boolean;
}

export interface LumioAgreementDocument {
  id: string;
  title: string;
  contentMd: string;
}

export interface LumioServiceSettings {
  registrationEnabled: boolean;
  emailVerifyEnabled: boolean;
  emailSuffixWhitelist: string[];
  passwordResetEnabled: boolean;
  agreementEnabled: boolean;
  agreementRevision: string;
  agreementDocuments: LumioAgreementDocument[];
  defaultModel: string | null;
  siteBaseUrl: string;
  paymentPath: string;
  /** 账户网页源（重置密码等）；与官网 siteBaseUrl 分开。 */
  apiBaseUrl: string;
  // Optional while the command layer lands: the register form must stay usable
  // against public settings that predate the invitation switch.
  invitationCodeEnabled?: boolean;
}

export interface LumioUpdateReminder {
  currentVersion: string;
  latestVersion: string | null;
  updateAvailable: boolean;
  /** 弹窗静默位：该版本已被忽略或今天已弹过（绿标入口不受影响）。 */
  noticeMuted: boolean;
  downloadUrl: string;
  releaseSummary: string;
}

/** 手动触发的更新下载结果：安装包已落盘并打开安装向导。 */
export interface LumioUpdateInstallResult {
  latestVersion: string;
  installerPath: string;
}

export interface LumioAuthResult {
  requiresTwoFactor: boolean;
  maskedEmail: string | null;
  account: LumioAccountSummary | null;
}

export interface LumioVerifyCodeResult {
  countdown: number;
}

export interface LumioProvisionStepResult {
  step: string;
  /** 只有 `verify-account` 带回真实账户，其余步骤为 null。 */
  account: LumioAccountSummary | null;
}

export type LumioTakeoverHealthStatus = "not-applied" | "healthy" | "conflicted";

export interface LumioTakeoverHealth {
  health: LumioTakeoverHealthStatus;
  errorCode: string | null;
}

export interface LumioTelemetryResult {
  enabled: boolean;
}

export interface LumioExportLogsResult {
  path: string;
}
