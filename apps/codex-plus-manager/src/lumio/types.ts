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

export interface LumioBootstrap {
  version: string;
  platform: string;
  arch: string;
  codexApp: LumioCodexApp | null;
  account: LumioAccountSummary | null;
  telemetryEnabled: boolean;
  autoUpdateEnabled: boolean;
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

export type LumioCredentialStatus = "present" | "missing" | "invalid";
