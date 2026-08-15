import { lumioErrorLabel } from "./errors.ts";
import type { LumioOfficialAppInstallStatus } from "./invoke.ts";
import type { LumioOfficialAppInstall, LumioOfficialAppInstallPhase } from "./types.ts";
import { LUMIO_OFFICIAL_APP_INSTALL_PHASES } from "./types.ts";

function asInstallPhase(phase: string): LumioOfficialAppInstallPhase {
  return (LUMIO_OFFICIAL_APP_INSTALL_PHASES as readonly string[]).includes(phase)
    ? (phase as LumioOfficialAppInstallPhase)
    : "failed";
}

/**
 * 命令层的 `bytesDownloaded` / `bytesTotal` 是 745MB 包体唯一可感知的进度信号
 * （D-19），映射时不许丢；`started` 只用于判断「本来就装过」，不进视图状态。
 */
export function toInstallProgress(status: LumioOfficialAppInstallStatus): LumioOfficialAppInstall {
  return {
    phase: asInstallPhase(status.phase),
    stage: status.stage,
    errorCode: status.errorCode,
    path: status.installedPath,
    bytesDownloaded: status.bytesDownloaded,
    bytesTotal: status.bytesTotal,
  };
}

export function installStageLabel(stage: string | null, phase: string): string | null {
  const key = (stage ?? phase).toLowerCase();
  if (key.includes("download")) return "下载";
  if (key.includes("verify")) return "校验";
  if (key.includes("install") || key.includes("detect") || key.includes("plan")) return "安装";
  return null;
}

/** 有正向总量才有百分比；分块传输拿不到 Content-Length 时返回 null 走不确定态。 */
export function downloadPercent(
  bytesDownloaded: number | null,
  bytesTotal: number | null,
): number | null {
  if (bytesDownloaded === null || bytesTotal === null || bytesTotal <= 0) return null;
  return Math.min(100, Math.floor((bytesDownloaded / bytesTotal) * 100));
}

export function formatBytes(bytes: number): string {
  if (bytes >= 1024 * 1024 * 1024) {
    return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
  }
  return `${Math.round(bytes / (1024 * 1024))} MB`;
}

/**
 * 失败 / 取消后的常驻文案（D-20）。toast 只有 4 秒，行动面板必须自己
 * 留住失败原因，直到用户再次尝试安装。
 */
export function installFailureCopy(
  install: Pick<LumioOfficialAppInstall, "phase" | "errorCode">,
): string {
  if (install.phase === "cancelled") {
    return "已取消安装官方 Codex";
  }
  return lumioErrorLabel(install.errorCode ?? "CODEX_APP_INSTALL_FAILED");
}

/** 进度条下方的人话；按钮文案只讲「正在安装」，量化信息由这里独占。 */
export function installProgressCopy(
  install: Pick<
    LumioOfficialAppInstall,
    "phase" | "stage" | "bytesDownloaded" | "bytesTotal"
  >,
  percent: number | null,
): string {
  if (install.phase === "downloading") {
    const downloaded = install.bytesDownloaded ?? null;
    const total = install.bytesTotal ?? null;
    if (percent !== null && downloaded !== null && total !== null) {
      return `下载 ${percent}% · ${formatBytes(downloaded)} / ${formatBytes(total)}`;
    }
    if (downloaded !== null) {
      return `已下载 ${formatBytes(downloaded)}`;
    }
    return "正在获取下载进度…";
  }
  if (install.phase === "verifying") return "正在校验安装包…";
  if (install.phase === "installing") return "正在安装官方 Codex…";
  if (install.phase === "planning") return "正在准备安装…";
  if (install.phase === "detecting") return "正在确认安装结果…";
  return "正在安装官方 Codex…";
}
