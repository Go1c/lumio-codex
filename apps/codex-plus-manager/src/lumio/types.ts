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

export type LumioCredentialStatus = "present" | "missing" | "invalid";

export interface LumioBootstrap {
  version: string;
  platform: string;
  arch: string;
  codexApp: LumioCodexApp | null;
  account: LumioAccountSummary | null;
  telemetryEnabled: boolean;
  autoUpdateEnabled: boolean;
  // Optional while the command layer lands: the shell must stay usable against a
  // bootstrap payload that predates the credential probe.
  credentialStatus?: LumioCredentialStatus;
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
}

export interface LumioTakeoverHealth {
  health: string;
  errorCode: string | null;
}

export interface LumioTelemetryResult {
  enabled: boolean;
}

export interface LumioExportLogsResult {
  path: string;
}
