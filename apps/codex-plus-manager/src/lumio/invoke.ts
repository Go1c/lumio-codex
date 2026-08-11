import { invoke } from "@tauri-apps/api/core";

import type { LumioBootstrap } from "./types.ts";

export const LUMIO_BOOTSTRAP_COMMAND = "lumio_bootstrap";

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
} as const;

export const visibleShellLabels = Object.values(shellLabels);

interface CommandResult<T> {
  ok: boolean;
  errorCode: string | null;
  payload: T;
}

export async function loadLumioBootstrap(): Promise<LumioBootstrap> {
  const result = await invoke<CommandResult<LumioBootstrap>>(LUMIO_BOOTSTRAP_COMMAND);
  if (!result.ok) {
    throw new Error(result.errorCode ?? "BOOTSTRAP_FAILED");
  }
  return result.payload;
}
