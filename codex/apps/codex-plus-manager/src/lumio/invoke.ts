import { invoke } from "@tauri-apps/api/core";

import type {
  LumioAccountSummary,
  LumioAuthResult,
  LumioBootstrap,
  LumioCodexApp,
  LumioExportLogsResult,
  LumioLaunchAtLoginResult,
  LumioProvisionStepResult,
  LumioServiceSettings,
  LumioTakeoverHealth,
  LumioTelemetryResult,
  LumioUpdateInstallResult,
  LumioUpdateReminder,
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
  checkUpdate: "lumio_check_update",
  downloadUpdate: "lumio_download_update",
  dismissUpdate: "lumio_dismiss_update",
  updateNoticeShown: "lumio_update_notice_shown",
  setTelemetry: "lumio_set_telemetry",
  setLaunchAtLogin: "lumio_set_launch_at_login",
  exportLogs: "lumio_export_logs",
  installOfficialApp: "lumio_install_official_app",
  officialAppStatus: "lumio_official_app_status",
  cancelOfficialApp: "lumio_cancel_official_app",
} as const;

export const LUMIO_BOOTSTRAP_COMMAND = LUMIO_COMMANDS.bootstrap;

export const shellLabels = {
  account: "账户",
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
  codex: "Codex",
  claude: "Claude",
  settings: "设置",
  general: "通用",
  support: "支持",
  helpCenter: "帮助中心",
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

/**
 * 命令层的 `payload` 是 `Option<T>`：失败分支没有合法 payload，`detectCodexApp`
 * 这种命令成功时也可能为空。这里照实建模，不让缺失的 payload 变成视图层的 undefined。
 */
export interface LumioCommandResult<T> {
  ok: boolean;
  errorCode: string | null;
  payload: T | null;
}

const UNKNOWN_ERROR_CODE = "UNKNOWN";

/** 刷新令牌过期或被撤销后，任何命令都可能抛这个码。 */
export const SESSION_EXPIRED_ERROR_CODE = "AUTH_SESSION_EXPIRED";

export function isSessionExpired(error: unknown): boolean {
  return error instanceof LumioCommandError && error.errorCode === SESSION_EXPIRED_ERROR_CODE;
}

let sessionExpiredListener: (() => void) | null = null;

/**
 * 会话彻底过期是全局降级（交互规格 §7：全局 toast → 回登录页），不是某一个调用点的局部
 * 失败。出口登记在命令层，新增命令自动继承，不必也不该在每个 catch 里各写一遍。
 */
export function onSessionExpired(listener: (() => void) | null): void {
  sessionExpiredListener = listener;
}

function reportSessionExpiry(error: unknown): void {
  if (isSessionExpired(error)) sessionExpiredListener?.();
}

/** `ok` 为真却没有 payload 属于契约违约，用一个稳定错误码上报而不是静默放行。 */
export const MISSING_PAYLOAD_ERROR_CODE = "COMMAND_PAYLOAD_MISSING";

/**
 * IPC / 旧包可能漏掉 `account` 字段（JS 侧为 `undefined`）。
 * 调用方若写 `account !== null` 会把假账户推进首页并在读 `email` 时黑屏。
 */
export function normalizeOptionalAccount(
  account: LumioAccountSummary | null | undefined,
): LumioAccountSummary | null {
  return account ?? null;
}

/** 失败分支抛稳定错误码；成功分支允许 payload 合法为空。 */
export function readCommandResult<T>(result: LumioCommandResult<T>): T | null {
  if (!result.ok) {
    throw new LumioCommandError(result.errorCode ?? UNKNOWN_ERROR_CODE);
  }
  return result.payload ?? null;
}

/** 调用方需要 payload 必然存在时走这条。 */
export function readRequiredCommandResult<T>(result: LumioCommandResult<T>): T {
  const payload = readCommandResult(result);
  if (payload === null) {
    throw new LumioCommandError(MISSING_PAYLOAD_ERROR_CODE);
  }
  return payload;
}

async function runCommand<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return readRequiredCommandResult(await invoke<LumioCommandResult<T>>(command, args));
  } catch (error: unknown) {
    reportSessionExpiry(error);
    throw error;
  }
}

async function runNullableCommand<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T | null> {
  try {
    return readCommandResult(await invoke<LumioCommandResult<T>>(command, args));
  } catch (error: unknown) {
    reportSessionExpiry(error);
    throw error;
  }
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

function normalizeAuthResult(result: LumioAuthResult): LumioAuthResult {
  return { ...result, account: normalizeOptionalAccount(result.account) };
}

export async function registerAccount(input: {
  email: string;
  password: string;
  verifyCode: string;
  acceptedRevision: string;
  invitationCode: string;
}): Promise<LumioAuthResult> {
  return normalizeAuthResult(
    await runCommand<LumioAuthResult>(LUMIO_COMMANDS.register, {
      email: input.email,
      password: input.password,
      verifyCode: input.verifyCode,
      acceptedRevision: input.acceptedRevision,
      // 空串由命令层归一化成「未填写」，前端不替服务端判断这个字段是否必填。
      invitationCode: input.invitationCode,
    }),
  );
}

export async function signIn(email: string, password: string): Promise<LumioAuthResult> {
  return normalizeAuthResult(
    await runCommand<LumioAuthResult>(LUMIO_COMMANDS.login, { email, password }),
  );
}

export async function submitTwoFactor(code: string): Promise<LumioAuthResult> {
  return normalizeAuthResult(
    await runCommand<LumioAuthResult>(LUMIO_COMMANDS.loginTwoFactor, { code }),
  );
}

export async function signOut(): Promise<void> {
  await runNullableCommand<unknown>(LUMIO_COMMANDS.logout);
}

export async function refreshAccount(): Promise<LumioAccountSummary> {
  return runCommand<LumioAccountSummary>(LUMIO_COMMANDS.refreshAccount);
}

export async function fetchClaudeEntitlement(): Promise<{ status: string } | null> {
  return runNullableCommand<{ status: string }>("lumio_claude_entitlement");
}

export interface ClaudePayWithBalanceResult {
  status: string;
  expiresAt?: string | null;
  daysLeft?: number | null;
  orderNo: string;
}

export interface ClaudeBillingOrder {
  orderNo: string;
  amountCents: number;
  status: string;
  paidAt?: string | null;
  createdAt: string;
}

export async function payClaudeWithBalance(): Promise<ClaudePayWithBalanceResult> {
  return runCommand<ClaudePayWithBalanceResult>("lumio_claude_pay_with_balance");
}

export async function listClaudeOrders(): Promise<ClaudeBillingOrder[]> {
  const payload = await runCommand<{ items: ClaudeBillingOrder[] }>("lumio_claude_orders");
  return payload.items ?? [];
}

export async function fetchClaudePlan(): Promise<{ amountCents: number } | null> {
  return runNullableCommand<{ amountCents: number }>("lumio_claude_plan");
}

export async function runProvisioningStep(step: string): Promise<LumioProvisionStepResult> {
  const result = await runCommand<LumioProvisionStepResult>(LUMIO_COMMANDS.provisionStep, {
    step,
  });
  return { ...result, account: normalizeOptionalAccount(result.account) };
}

export async function checkTakeover(): Promise<LumioTakeoverHealth> {
  return runCommand<LumioTakeoverHealth>(LUMIO_COMMANDS.takeoverHealth);
}

export async function restoreConfig(): Promise<void> {
  await runNullableCommand<unknown>(LUMIO_COMMANDS.restoreConfig);
}

export async function launchCodex(): Promise<void> {
  await runNullableCommand<unknown>(LUMIO_COMMANDS.launchCodex);
}

/** 未检测到官方应用时命令层返回空 payload，这是合法结果而不是失败。 */
export async function detectCodexApp(): Promise<LumioCodexApp | null> {
  return runNullableCommand<LumioCodexApp>(LUMIO_COMMANDS.detectCodexApp);
}

export async function selectCodexApp(path: string): Promise<LumioCodexApp> {
  return runCommand<LumioCodexApp>(LUMIO_COMMANDS.selectCodexApp, { path });
}

export async function openInBrowser(url: string): Promise<void> {
  await runNullableCommand<unknown>(LUMIO_COMMANDS.openBrowser, { url });
}

export async function checkUpdate(): Promise<LumioUpdateReminder> {
  return runCommand<LumioUpdateReminder>(LUMIO_COMMANDS.checkUpdate);
}

export async function downloadUpdate(): Promise<LumioUpdateInstallResult> {
  return runCommand<LumioUpdateInstallResult>(LUMIO_COMMANDS.downloadUpdate);
}

/** 弹窗「稍后」：忽略这个版本，下一个版本才恢复弹窗。 */
export async function dismissUpdate(version: string): Promise<void> {
  await runNullableCommand<unknown>(LUMIO_COMMANDS.dismissUpdate, { version });
}

/** 弹窗已渲染：记录今天弹过一次（失败静默，不影响提醒链路）。 */
export async function updateNoticeShown(): Promise<void> {
  await runNullableCommand<unknown>(LUMIO_COMMANDS.updateNoticeShown).catch(() => undefined);
}

export async function setTelemetry(enabled: boolean): Promise<LumioTelemetryResult> {
  return runCommand<LumioTelemetryResult>(LUMIO_COMMANDS.setTelemetry, { enabled });
}

export async function setLaunchAtLogin(enabled: boolean): Promise<LumioLaunchAtLoginResult> {
  return runCommand<LumioLaunchAtLoginResult>(LUMIO_COMMANDS.setLaunchAtLogin, { enabled });
}

export async function exportLogs(): Promise<LumioExportLogsResult> {
  return runCommand<LumioExportLogsResult>(LUMIO_COMMANDS.exportLogs);
}

export interface LumioOfficialAppInstallStatus {
  phase: string;
  stage: string | null;
  bytesDownloaded: number | null;
  bytesTotal: number | null;
  errorCode: string | null;
  installedPath: string | null;
  started?: boolean;
}

export async function installOfficialApp(
  destination: string | null = null,
): Promise<LumioOfficialAppInstallStatus> {
  return runCommand<LumioOfficialAppInstallStatus>(LUMIO_COMMANDS.installOfficialApp, {
    destination,
  });
}

export async function officialAppStatus(): Promise<LumioOfficialAppInstallStatus> {
  return runCommand<LumioOfficialAppInstallStatus>(LUMIO_COMMANDS.officialAppStatus);
}

export async function cancelOfficialApp(): Promise<LumioOfficialAppInstallStatus> {
  return runCommand<LumioOfficialAppInstallStatus>(LUMIO_COMMANDS.cancelOfficialApp);
}
