import type { LumioBootstrap, LumioPhase } from "./types.ts";

export interface LumioActions {
  canLaunch: boolean;
  canRefresh: boolean;
  canPay: boolean;
}

export interface LumioState {
  phase: LumioPhase;
  bootstrap: LumioBootstrap | null;
  telemetryEnabled: boolean;
  autoUpdateEnabled: boolean;
  cachedAt: string | null;
  errorCode: string | null;
  actions: LumioActions;
}

export type LumioEvent =
  | { type: "bootstrapped"; payload: LumioBootstrap }
  | { type: "offline-ready"; cachedAt: string }
  | { type: "repair-required"; errorCode: string };

function disabledActions(): LumioActions {
  return { canLaunch: false, canRefresh: false, canPay: false };
}

export function initialLumioState(): LumioState {
  return {
    phase: "bootstrapping",
    bootstrap: null,
    telemetryEnabled: false,
    autoUpdateEnabled: true,
    cachedAt: null,
    errorCode: null,
    actions: disabledActions(),
  };
}

export function reduceLumioState(state: LumioState, event: LumioEvent): LumioState {
  if (event.type === "bootstrapped") {
    return {
      ...state,
      phase: event.payload.account === null ? "signed-out" : "provisioning",
      bootstrap: event.payload,
      telemetryEnabled: event.payload.telemetryEnabled,
      autoUpdateEnabled: event.payload.autoUpdateEnabled,
      actions: disabledActions(),
    };
  }

  if (event.type === "offline-ready") {
    return {
      ...state,
      phase: "ready-offline",
      cachedAt: event.cachedAt,
      actions: { canLaunch: true, canRefresh: false, canPay: false },
    };
  }

  return {
    ...state,
    phase: "needs-repair",
    errorCode: event.errorCode,
    actions: disabledActions(),
  };
}
