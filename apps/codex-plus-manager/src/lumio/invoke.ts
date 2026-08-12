import { invoke } from "@tauri-apps/api/core";

import type {
  LumioAccountSummary,
  LumioAuthResult,
  LumioBootstrap,
  LumioCodexApp,
  LumioExportLogsResult,
  LumioProvisionStepResult,
  LumioServiceSettings,
  LumioTakeoverHealth,
  LumioTelemetryResult,
  LumioVerifyCodeResult,
} from "./types.ts";

export const LUMIO_COMMANDS = {
  bootstrap: "lumio_bootstrap",
  publicSettings: "lumio_public_settings",
  sendVerifyCode: "lumio_send_verify_code",
  register: "lumio_register",
  login: "lumio_login",
  loginTwoFactor: "lumio_login_two_factor",
  logout: "lumio_logout",
  refreshAccount: "lumio_refresh_account",
  provisionStep: "lumio_provision_step",
  takeoverHealth: "lumio_takeover_health",
  restoreConfig: "lumio_restore_config",
  launchCodex: "lumio_launch_codex",
  detectCodexApp: "lumio_detect_codex_app",
  selectCodexApp: "lumio_select_codex_app",
  openBrowser: "lumio_open_browser",
  setTelemetry: "lumio_set_telemetry",
  exportLogs: "lumio_export_logs",
} as const;

export const LUMIO_BOOTSTRAP_COMMAND = LUMIO_COMMANDS.bootstrap;

export const shellLabels = {
  accountStatus: "账户状态",
  balanceAndPlan: "余额与套餐",
  connectionStatus: "连接状态",
  defaultModel: "默认模型",
  payment: "充值",
  launch: "启动 Codex",
  launchAtLogin: "开机启动",
  automaticUpdates: "自动更新",
  officialAppPath: "官方应用路径",
  telemetry: "遥测",
  exportLogs: "日志导出",
  restoreConfiguration: "配置恢复",
  signIn: "登录",
  createAccount: "创建账户",
  home: "首页",
  settings: "设置",
  verifyAccount: "验证账户",
  prepareConnection: "准备连接",
  syncModels: "同步模型目录",
  writeConfig: "写入本机配置",
  recheck: "重新检查",
  restoreLocalConfig: "恢复本机配置",
  exportDiagnostics: "导出诊断日志",
} as const;

export const visibleShellLabels = Object.values(shellLabels);

export class LumioCommandError extends Error {
  readonly errorCode: string;

  constructor(errorCode: string) {
    super(errorCode);
    this.name = "LumioCommandError";
    this.errorCode = errorCode;
  }
}

interface CommandResult<T> {
  ok: boolean;
  errorCode: string | null;
  payload: T;
}

const UNKNOWN_ERROR_CODE = "UNKNOWN";

async function runCommand<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const result = await invoke<CommandResult<T>>(command, args);
  if (!result.ok) {
    throw new LumioCommandError(result.errorCode ?? UNKNOWN_ERROR_CODE);
  }
  return result.payload;
}

export async function loadLumioBootstrap(): Promise<LumioBootstrap> {
  return runCommand<LumioBootstrap>(LUMIO_COMMANDS.bootstrap);
}

export async function loadPublicSettings(): Promise<LumioServiceSettings> {
  return runCommand<LumioServiceSettings>(LUMIO_COMMANDS.publicSettings);
}

export async function sendVerifyCode(email: string): Promise<LumioVerifyCodeResult> {
  return runCommand<LumioVerifyCodeResult>(LUMIO_COMMANDS.sendVerifyCode, { email });
}

export async function registerAccount(input: {
  email: string;
  password: string;
  verifyCode: string;
  acceptedRevision: string;
}): Promise<LumioAuthResult> {
  return runCommand<LumioAuthResult>(LUMIO_COMMANDS.register, {
    email: input.email,
    password: input.password,
    verifyCode: input.verifyCode,
    acceptedRevision: input.acceptedRevision,
  });
}

export async function signIn(email: string, password: string): Promise<LumioAuthResult> {
  return runCommand<LumioAuthResult>(LUMIO_COMMANDS.login, { email, password });
}

export async function submitTwoFactor(code: string): Promise<LumioAuthResult> {
  return runCommand<LumioAuthResult>(LUMIO_COMMANDS.loginTwoFactor, { code });
}

export async function signOut(): Promise<void> {
  await runCommand<unknown>(LUMIO_COMMANDS.logout);
}

export async function refreshAccount(): Promise<LumioAccountSummary> {
  return runCommand<LumioAccountSummary>(LUMIO_COMMANDS.refreshAccount);
}

export async function runProvisioningStep(step: string): Promise<LumioProvisionStepResult> {
  return runCommand<LumioProvisionStepResult>(LUMIO_COMMANDS.provisionStep, { step });
}

export async function checkTakeover(): Promise<LumioTakeoverHealth> {
  return runCommand<LumioTakeoverHealth>(LUMIO_COMMANDS.takeoverHealth);
}

export async function restoreConfig(): Promise<void> {
  await runCommand<unknown>(LUMIO_COMMANDS.restoreConfig);
}

export async function launchCodex(): Promise<void> {
  await runCommand<unknown>(LUMIO_COMMANDS.launchCodex);
}

export async function detectCodexApp(): Promise<LumioCodexApp | null> {
  return runCommand<LumioCodexApp | null>(LUMIO_COMMANDS.detectCodexApp);
}

export async function selectCodexApp(path: string): Promise<LumioCodexApp> {
  return runCommand<LumioCodexApp>(LUMIO_COMMANDS.selectCodexApp, { path });
}

export async function openInBrowser(url: string): Promise<void> {
  await runCommand<unknown>(LUMIO_COMMANDS.openBrowser, { url });
}

export async function setTelemetry(enabled: boolean): Promise<LumioTelemetryResult> {
  return runCommand<LumioTelemetryResult>(LUMIO_COMMANDS.setTelemetry, { enabled });
}

export async function exportLogs(): Promise<LumioExportLogsResult> {
  return runCommand<LumioExportLogsResult>(LUMIO_COMMANDS.exportLogs);
}
