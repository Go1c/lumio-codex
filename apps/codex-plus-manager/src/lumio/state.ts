import type {
  LumioAccountSummary,
  LumioBootstrap,
  LumioCodexApp,
  LumioPhase,
  LumioServiceSettings,
} from "./types.ts";

export type { LumioServiceSettings } from "./types.ts";

export const PROVISIONING_STEP_IDS = [
  "verify-account",
  "prepare-connection",
  "sync-models",
  "write-config",
] as const;

export type ProvisioningStepId = (typeof PROVISIONING_STEP_IDS)[number];

export const PROVISIONING_STEP_TITLES: Record<ProvisioningStepId, string> = {
  "verify-account": "验证账户",
  "prepare-connection": "准备连接",
  "sync-models": "同步模型目录",
  "write-config": "写入本机配置",
};

export type ProvisioningStepStatus = "pending" | "running" | "done" | "failed";

export type LumioAuthStep = "idle" | "login" | "register" | "two-factor";

export interface LumioProvisioning {
  steps: Record<ProvisioningStepId, ProvisioningStepStatus>;
  failedStep: ProvisioningStepId | null;
  errorCode: string | null;
  attempts: number;
  suggestRepair: boolean;
}

export interface LumioActions {
  canLaunch: boolean;
  canRefresh: boolean;
  canPay: boolean;
  canRegister: boolean;
  canSignIn: boolean;
}

export interface LumioActionNotes {
  launch: string | null;
  refresh: string | null;
  pay: string | null;
  register: string | null;
  signIn: string | null;
}

export interface LumioState {
  phase: LumioPhase;
  bootstrap: LumioBootstrap | null;
  service: LumioServiceSettings | null;
  serviceAvailable: boolean;
  authStep: LumioAuthStep;
  account: LumioAccountSummary | null;
  codexApp: LumioCodexApp | null;
  defaultModel: string | null;
  provisioning: LumioProvisioning;
  telemetryEnabled: boolean;
  autoUpdateEnabled: boolean;
  cachedAt: string | null;
  errorCode: string | null;
  actions: LumioActions;
  actionNotes: LumioActionNotes;
}

export type LumioEvent =
  | { type: "bootstrapped"; payload: LumioBootstrap }
  | { type: "service-settings-loaded"; settings: LumioServiceSettings }
  | { type: "service-unavailable"; errorCode: string }
  | { type: "auth-step-changed"; step: LumioAuthStep }
  | { type: "two-factor-required" }
  | { type: "authenticated"; account: LumioAccountSummary }
  | { type: "provisioning-step-started"; step: ProvisioningStepId }
  | { type: "provisioning-step-completed"; step: ProvisioningStepId }
  | { type: "provisioning-step-failed"; step: ProvisioningStepId; errorCode: string }
  | {
      type: "online-ready";
      account: LumioAccountSummary;
      cachedAt: string;
      defaultModel: string | null;
      codexApp: LumioCodexApp | null;
    }
  | { type: "offline-ready"; cachedAt: string }
  | { type: "account-refreshed"; account: LumioAccountSummary; cachedAt: string }
  | { type: "repair-required"; errorCode: string }
  | { type: "session-expired"; errorCode: string }
  | { type: "signed-out" };

const PAY_DISABLED_NOTE = "充值功能尚未开放";
const OFFLINE_NOTE = "需要恢复网络连接";
const NO_APP_NOTE = "未检测到官方应用，去设置中选择";
const SERVICE_DOWN_NOTE = "服务暂时不可用，稍后自动重试";
const REGISTRATION_CLOSED_NOTE = "注册暂未开放";
const MAX_PROVISIONING_ATTEMPTS = 2;

function disabledActions(): LumioActions {
  return {
    canLaunch: false,
    canRefresh: false,
    canPay: false,
    canRegister: false,
    canSignIn: false,
  };
}

function noNotes(): LumioActionNotes {
  return { launch: null, refresh: null, pay: PAY_DISABLED_NOTE, register: null, signIn: null };
}

function pendingProvisioning(): LumioProvisioning {
  return {
    steps: {
      "verify-account": "pending",
      "prepare-connection": "pending",
      "sync-models": "pending",
      "write-config": "pending",
    },
    failedStep: null,
    errorCode: null,
    attempts: 0,
    suggestRepair: false,
  };
}

export function initialLumioState(): LumioState {
  return {
    phase: "bootstrapping",
    bootstrap: null,
    service: null,
    serviceAvailable: false,
    authStep: "idle",
    account: null,
    codexApp: null,
    defaultModel: null,
    provisioning: pendingProvisioning(),
    telemetryEnabled: false,
    autoUpdateEnabled: true,
    cachedAt: null,
    errorCode: null,
    actions: disabledActions(),
    actionNotes: noNotes(),
  };
}

function withStepStatus(
  provisioning: LumioProvisioning,
  step: ProvisioningStepId,
  status: ProvisioningStepStatus,
): Record<ProvisioningStepId, ProvisioningStepStatus> {
  return { ...provisioning.steps, [step]: status };
}

export function reduceLumioState(state: LumioState, event: LumioEvent): LumioState {
  switch (event.type) {
    case "bootstrapped":
      return {
        ...state,
        phase: event.payload.account === null ? "signed-out" : "provisioning",
        bootstrap: event.payload,
        account: event.payload.account,
        codexApp: event.payload.codexApp,
        telemetryEnabled: event.payload.telemetryEnabled,
        autoUpdateEnabled: event.payload.autoUpdateEnabled,
        actions: disabledActions(),
        actionNotes: noNotes(),
      };

    case "service-settings-loaded":
      return {
        ...state,
        service: event.settings,
        serviceAvailable: true,
        defaultModel: event.settings.defaultModel,
        errorCode: null,
        actions: {
          ...state.actions,
          canSignIn: true,
          canRegister: event.settings.registrationEnabled,
        },
        actionNotes: {
          ...state.actionNotes,
          signIn: null,
          register: event.settings.registrationEnabled ? null : REGISTRATION_CLOSED_NOTE,
        },
      };

    case "service-unavailable":
      return {
        ...state,
        serviceAvailable: false,
        errorCode: event.errorCode,
        actions: { ...state.actions, canSignIn: false, canRegister: false, canRefresh: false },
        actionNotes: {
          ...state.actionNotes,
          signIn: SERVICE_DOWN_NOTE,
          register: SERVICE_DOWN_NOTE,
          refresh: OFFLINE_NOTE,
        },
      };

    case "auth-step-changed":
      return {
        ...state,
        phase: event.step === "idle" ? "signed-out" : "authenticating",
        authStep: event.step,
        errorCode: null,
      };

    case "two-factor-required":
      return { ...state, phase: "authenticating", authStep: "two-factor", errorCode: null };

    case "authenticated":
      return {
        ...state,
        phase: "provisioning",
        authStep: "idle",
        account: event.account,
        errorCode: null,
        provisioning: pendingProvisioning(),
      };

    case "provisioning-step-started":
      return {
        ...state,
        phase: "provisioning",
        provisioning: {
          ...state.provisioning,
          steps: withStepStatus(state.provisioning, event.step, "running"),
          failedStep: null,
          errorCode: null,
        },
      };

    case "provisioning-step-completed":
      return {
        ...state,
        provisioning: {
          ...state.provisioning,
          steps: withStepStatus(state.provisioning, event.step, "done"),
        },
      };

    case "provisioning-step-failed": {
      const attempts = state.provisioning.attempts + 1;
      return {
        ...state,
        phase: "provisioning",
        provisioning: {
          ...state.provisioning,
          steps: withStepStatus(state.provisioning, event.step, "failed"),
          failedStep: event.step,
          errorCode: event.errorCode,
          attempts,
          suggestRepair: attempts >= MAX_PROVISIONING_ATTEMPTS,
        },
      };
    }

    case "online-ready":
      return {
        ...state,
        phase: "ready-online",
        account: event.account,
        codexApp: event.codexApp,
        defaultModel: event.defaultModel,
        cachedAt: event.cachedAt,
        serviceAvailable: true,
        errorCode: null,
        actions: {
          ...state.actions,
          canLaunch: event.codexApp !== null,
          canRefresh: true,
          canPay: false,
        },
        actionNotes: {
          ...state.actionNotes,
          launch: event.codexApp === null ? NO_APP_NOTE : null,
          refresh: null,
          pay: PAY_DISABLED_NOTE,
        },
      };

    case "offline-ready":
      return {
        ...state,
        phase: "ready-offline",
        cachedAt: event.cachedAt,
        serviceAvailable: false,
        actions: { ...state.actions, canLaunch: true, canRefresh: false, canPay: false },
        actionNotes: {
          ...state.actionNotes,
          launch: null,
          refresh: OFFLINE_NOTE,
          pay: OFFLINE_NOTE,
        },
      };

    case "account-refreshed":
      return { ...state, account: event.account, cachedAt: event.cachedAt };

    case "repair-required":
      return {
        ...state,
        phase: "needs-repair",
        errorCode: event.errorCode,
        actions: disabledActions(),
        actionNotes: noNotes(),
      };

    case "session-expired":
      return {
        ...initialLumioState(),
        phase: "signed-out",
        bootstrap: state.bootstrap,
        service: state.service,
        serviceAvailable: state.serviceAvailable,
        codexApp: state.codexApp,
        telemetryEnabled: state.telemetryEnabled,
        autoUpdateEnabled: state.autoUpdateEnabled,
        errorCode: event.errorCode,
      };

    case "signed-out":
      return {
        ...initialLumioState(),
        phase: "signed-out",
        bootstrap: state.bootstrap,
        service: state.service,
        serviceAvailable: state.serviceAvailable,
        codexApp: state.codexApp,
        telemetryEnabled: state.telemetryEnabled,
        autoUpdateEnabled: state.autoUpdateEnabled,
      };
  }
}
