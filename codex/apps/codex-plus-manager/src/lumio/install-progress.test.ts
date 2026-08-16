import assert from "node:assert/strict";
import test from "node:test";

import type { LumioOfficialAppInstallStatus } from "./invoke.ts";
import {
  downloadPercent,
  formatBytes,
  installFailureCopy,
  installProgressCopy,
  installStageLabel,
  resolveInstallDestination,
  toInstallProgress,
} from "./install-progress.ts";

/** D-19：命令层已把 bytesDownloaded/bytesTotal 透出到 IPC，前端不许再丢。 */
test("toInstallProgress carries the byte counters through", () => {
  const status: LumioOfficialAppInstallStatus = {
    phase: "downloading",
    stage: "download",
    bytesDownloaded: 314_572_800,
    bytesTotal: 781_185_024,
    errorCode: null,
    installedPath: null,
    started: true,
  };

  const progress = toInstallProgress(status);
  assert.equal(progress.phase, "downloading");
  assert.equal(progress.stage, "download");
  assert.equal(progress.bytesDownloaded, 314_572_800);
  assert.equal(progress.bytesTotal, 781_185_024);
});

test("unknown phases collapse to failed instead of leaking internals", () => {
  const progress = toInstallProgress({
    phase: "weird-phase",
    stage: null,
    bytesDownloaded: null,
    bytesTotal: null,
    errorCode: null,
    installedPath: null,
  });
  assert.equal(progress.phase, "failed");
});

test("downloadPercent is only meaningful with a positive total", () => {
  assert.equal(downloadPercent(50, 200), 25);
  assert.equal(downloadPercent(500, 200), 100, "超过总量按满格收敛，不许超过 100");
  assert.equal(downloadPercent(0, 0), null);
  assert.equal(downloadPercent(120, null), null);
  assert.equal(downloadPercent(null, 200), null);
});

test("formatBytes reads back in MB and GB", () => {
  assert.equal(formatBytes(0), "0 MB");
  assert.equal(formatBytes(734_003_200), "700 MB");
  assert.equal(formatBytes(1_610_612_736), "1.5 GB");
});

test("stage labels keep the download / verify / install wording", () => {
  assert.equal(installStageLabel("download", "downloading"), "下载");
  assert.equal(installStageLabel("verify", "verifying"), "校验");
  assert.equal(installStageLabel("install", "installing"), "安装");
  assert.equal(installStageLabel("detect", "detecting"), "安装");
  assert.equal(installStageLabel("plan", "planning"), "安装");
  assert.equal(installStageLabel(null, "succeeded"), null);
});

test("installProgressCopy narrates quantities while downloading", () => {
  assert.equal(
    installProgressCopy(
      { phase: "downloading", stage: "download", bytesDownloaded: 335_544_320, bytesTotal: 734_003_200 },
      45,
    ),
    "下载 45% · 320 MB / 700 MB",
  );
  assert.equal(
    installProgressCopy(
      { phase: "downloading", stage: "download", bytesDownloaded: 104_857_600, bytesTotal: null },
      null,
    ),
    "已下载 100 MB",
  );
  assert.equal(
    installProgressCopy({ phase: "downloading", stage: "download", bytesDownloaded: null, bytesTotal: null }, null),
    "正在获取下载进度…",
  );
});

test("installProgressCopy falls back to stage narration without numbers", () => {
  assert.equal(installProgressCopy({ phase: "verifying", stage: "verify" }, null), "正在校验安装包…");
  assert.equal(installProgressCopy({ phase: "planning", stage: "plan" }, null), "正在准备安装…");
  assert.equal(installProgressCopy({ phase: "detecting", stage: "detect" }, null), "正在确认安装结果…");
});

test("installProgressCopy names the chosen write path while installing", () => {
  assert.equal(
    installProgressCopy({ phase: "installing", stage: "install" }, null, "/Applications/Codex.app"),
    "正在写入 /Applications/Codex.app",
  );
  assert.equal(
    installProgressCopy({ phase: "installing", stage: "install" }, null, "/Users/me/Apps/Codex.app"),
    "正在写入 /Users/me/Apps/Codex.app",
  );
});

test("resolveInstallDestination prefers the chosen folder and defaults macOS to Codex.app", () => {
  assert.equal(resolveInstallDestination("/Users/me/Apps", "macos"), "/Users/me/Apps");
  assert.equal(resolveInstallDestination(null, "macos"), "/Applications/Codex.app");
  assert.equal(resolveInstallDestination(null, "windows"), null);
});

/** D-20：失败必须留在行动面板上说清原因，不能只靠 4 秒的 toast。 */
test("installFailureCopy explains the failure with its code", () => {
  assert.equal(
    installFailureCopy({ phase: "failed", errorCode: "CODEX_APP_DOWNLOAD_FAILED" }),
    "下载官方应用失败，请检查网络后重试（CODEX_APP_DOWNLOAD_FAILED）",
  );
  assert.equal(
    installFailureCopy({ phase: "failed", errorCode: "CODEX_APP_VERIFY_FAILED" }),
    "官方应用校验未通过，已放弃安装（CODEX_APP_VERIFY_FAILED）",
  );
  assert.equal(
    installFailureCopy({ phase: "failed", errorCode: null }),
    "安装官方应用失败，可重试（CODEX_APP_INSTALL_FAILED）",
  );
});

test("cancelled installs read as cancelled, not failed", () => {
  assert.equal(
    installFailureCopy({ phase: "cancelled", errorCode: null }),
    "已取消安装官方 Codex",
  );
});
