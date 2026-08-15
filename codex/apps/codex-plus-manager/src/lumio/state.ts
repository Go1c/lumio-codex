import type {
  LumioAccountSummary,
  LumioBootstrap,
  LumioCodexApp,
  LumioOfficialAppInstall,
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

/** 余额不足：用户可操作的账户态，与宕机 / 本机故障分别对待。 */
export const ACCOUNT_INSUFFICIENT_BALANCE_CODE = "ACCOUNT_INSUFFICIENT_BALANCE";

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
  officialAppInstall: LumioOfficialAppInstall;
  launchAtLoginEnabled: boolean;
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
  // 启动时进入离线可能没有任何可信的同步时间，缺失要如实传下去，不许补一个假时间戳。
  | { type: "offline-ready"; cachedAt: string | null }
  // 设置页手动选择了官方应用：任何阶段都接受——离线/登出下自动检测失败时，
  // 这是用户唯一的补救路径（QA D-3），不能再被「仅在线」的守卫丢弃。
  | { type: "codex-app-changed"; app: LumioCodexApp }
  | { type: "official-app-install-progress"; status: LumioOfficialAppInstall }
  | { type: "launch-at-login-changed"; enabled: boolean }
  | { type: "account-refreshed"; account: LumioAccountSummary; cachedAt: string }
  | { type: "repair-required"; errorCode: string }
  | { type: "session-expired"; errorCode: string }
  | { type: "signed-out" };

const OFFLINE_NOTE = "需要恢复网络连接";
const OFFLINE_NO_APP_NOTE = "安装官方应用需要网络";
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
  return { launch: null, refresh: null, pay: null, register: null, signIn: null };
}

/**
 * Entry-point affordances for the signed-out surface. Every disabled button must
 * carry a note explaining why, otherwise the user is stuck with no way forward.
 *
 * Missing settings force `serviceAvailable` to false: the surface retries the
 * public settings while that flag is false, so claiming availability here would
 * suppress the retry and leave the sign-in button permanently dark.
 */
function signedOutEntry(
  service: LumioServiceSettings | null,
  serviceAvailable: boolean,
): Pick<LumioState, "actions" | "actionNotes" | "serviceAvailable"> {
  if (service === null || !serviceAvailable) {
    return {
      serviceAvailable: false,
      actions: disabledActions(),
      actionNotes: { ...noNotes(), signIn: SERVICE_DOWN_NOTE, register: SERVICE_DOWN_NOTE },
    };
  }

  return {
    serviceAvailable: true,
    actions: { ...disabledActions(), canSignIn: true, canRegister: service.registrationEnabled },
    actionNotes: {
      ...noNotes(),
      register: service.registrationEnabled ? null : REGISTRATION_CLOSED_NOTE,
    },
  };
}

function withoutCachedAccount(bootstrap: LumioBootstrap | null): LumioBootstrap | null {
  return bootstrap === null ? null : { ...bootstrap, account: null };
}

export function idleOfficialAppInstall(): LumioOfficialAppInstall {
  return { phase: "idle", stage: null, errorCode: null };
}

export function isOfficialAppInstallInProgress(install: LumioOfficialAppInstall): boolean {
  return (
    install.phase === "planning" ||
    install.phase === "downloading" ||
    install.phase === "verifying" ||
    install.phase === "installing" ||
    install.phase === "detecting"
  );
}

function readyLaunch(
  phase: LumioPhase,
  codexApp: LumioCodexApp | null,
  install: LumioOfficialAppInstall,
): { canLaunch: boolean; launchNote: string | null } {
  if (isOfficialAppInstallInProgress(install)) {
    return {
      canLaunch: false,
      launchNote: phase === "ready-offline" && codexApp === null ? OFFLINE_NO_APP_NOTE : null,
    };
  }
  if (phase === "ready-online") {
    return { canLaunch: true, launchNote: null };
  }
  if (phase === "ready-offline") {
    return {
      canLaunch: codexApp !== null,
      launchNote: codexApp === null ? OFFLINE_NO_APP_NOTE : null,
    };
  }
  return { canLaunch: false, launchNote: null };
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
    officialAppInstall: idleOfficialAppInstall(),
    launchAtLoginEnabled: false,
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
        // 旧包不带该字段：本机偏好按「关」如实显示，用户可手动打开。
        launchAtLoginEnabled: event.payload.launchAtLoginEnabled ?? false,
        actions: disabledActions(),
        actionNotes: noNotes(),
      };

    case "service-settings-loaded":
      return {
        ...state,
        service: event.settings,
        serviceAvailable: true,
        defaultModel: event.settings.defaultModel,
        // 修复页正在用 errorCode 解释冲突原因；服务恢复只更新可用性，不清这个码（QA D-12）。
        errorCode: state.phase === "needs-repair" ? state.errorCode : null,
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
        // 同上：探活失败不许覆盖修复页的错误码，服务状态由 serviceAvailable 表达。
        errorCode: state.phase === "needs-repair" ? state.errorCode : event.errorCode,
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
        // 充值入口只在失败面上出现；步骤一旦重新开跑就收回。
        actions: { ...state.actions, canPay: false },
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
      // runCommand 在会话过期时先同步派发 session-expired（监听器随后把用户带回
      // 登录入口）再 rethrow；这个迟到的失败事件不得把用户拽回 provisioning
      // 失败态，否则「重试→再过期」会形成死循环（QA D-2）。
      if (state.phase === "signed-out" || state.phase === "authenticating") {
        return state;
      }
      const attempts = state.provisioning.attempts + 1;
      // 余额不足修本机配置没有意义，永远不引导去修复页；充值是唯一出路，
      // 入口必须留在失败面上，不能逼用户「稍后处理」退出登录再重来。
      const payable = event.errorCode === ACCOUNT_INSUFFICIENT_BALANCE_CODE;
      return {
        ...state,
        phase: "provisioning",
        provisioning: {
          ...state.provisioning,
          steps: withStepStatus(state.provisioning, event.step, "failed"),
          failedStep: event.step,
          errorCode: event.errorCode,
          attempts,
          suggestRepair: !payable && attempts >= MAX_PROVISIONING_ATTEMPTS,
        },
        actions: { ...state.actions, canPay: payable },
      };
    }

    case "online-ready": {
      const launch = readyLaunch("ready-online", event.codexApp, state.officialAppInstall);
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
          canLaunch: launch.canLaunch,
          canRefresh: true,
          canPay: true,
        },
        actionNotes: {
          ...state.actionNotes,
          launch: launch.launchNote,
          refresh: null,
          pay: null,
        },
      };
    }

    case "offline-ready": {
      const launch = readyLaunch("ready-offline", state.codexApp, state.officialAppInstall);
      return {
        ...state,
        phase: "ready-offline",
        cachedAt: event.cachedAt,
        serviceAvailable: false,
        actions: {
          ...state.actions,
          canLaunch: launch.canLaunch,
          canRefresh: false,
          canPay: false,
        },
        actionNotes: {
          ...state.actionNotes,
          launch: launch.launchNote,
          refresh: OFFLINE_NOTE,
          pay: OFFLINE_NOTE,
        },
      };
    }

    case "account-refreshed":
      return { ...state, account: event.account, cachedAt: event.cachedAt };

    case "codex-app-changed": {
      const officialAppInstall = isOfficialAppInstallInProgress(state.officialAppInstall)
        ? { phase: "succeeded" as const, stage: null, errorCode: null, path: event.app.path }
        : state.officialAppInstall;
      const ready = state.phase === "ready-online" || state.phase === "ready-offline";
      if (!ready) {
        // 非就绪阶段只记住选择：offline-ready/online-ready 派发时会消费它。
        return { ...state, codexApp: event.app, officialAppInstall };
      }
      const launch = readyLaunch(state.phase, event.app, officialAppInstall);
      return {
        ...state,
        codexApp: event.app,
        officialAppInstall,
        actions: { ...state.actions, canLaunch: launch.canLaunch },
        actionNotes: { ...state.actionNotes, launch: launch.launchNote },
      };
    }

    case "official-app-install-progress": {
      const officialAppInstall = event.status;
      const ready = state.phase === "ready-online" || state.phase === "ready-offline";
      if (!ready) {
        return { ...state, officialAppInstall };
      }
      const launch = readyLaunch(state.phase, state.codexApp, officialAppInstall);
      return {
        ...state,
        officialAppInstall,
        actions: { ...state.actions, canLaunch: launch.canLaunch },
        actionNotes: { ...state.actionNotes, launch: launch.launchNote },
      };
    }

    case "launch-at-login-changed":
      return { ...state, launchAtLoginEnabled: event.enabled };

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
        bootstrap: withoutCachedAccount(state.bootstrap),
        service: state.service,
        codexApp: state.codexApp,
        telemetryEnabled: state.telemetryEnabled,
        autoUpdateEnabled: state.autoUpdateEnabled,
        launchAtLoginEnabled: state.launchAtLoginEnabled,
        errorCode: event.errorCode,
        ...signedOutEntry(state.service, state.serviceAvailable),
      };

    case "signed-out":
      return {
        ...initialLumioState(),
        phase: "signed-out",
        bootstrap: withoutCachedAccount(state.bootstrap),
        service: state.service,
        codexApp: state.codexApp,
        telemetryEnabled: state.telemetryEnabled,
        autoUpdateEnabled: state.autoUpdateEnabled,
        launchAtLoginEnabled: state.launchAtLoginEnabled,
        ...signedOutEntry(state.service, state.serviceAvailable),
      };
  }
}
