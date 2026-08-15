import { open } from "@tauri-apps/plugin-dialog";
import { Activity, Download, FileArchive, Laptop, LogOut, RefreshCw, Rocket, RotateCcw, ShieldCheck } from "lucide-react";
import { useEffect, useState } from "react";

import { lumioErrorLabel } from "../errors.ts";
import {
  LumioCommandError,
  detectCodexApp,
  exportLogs,
  restoreConfig,
  selectCodexApp,
  setLaunchAtLogin,
  shellLabels,
} from "../invoke.ts";
import { isOfficialAppInstallInProgress } from "../state.ts";
import type { LumioCodexApp, LumioOfficialAppInstall } from "../types.ts";
import { RESTORE_CONFIRM_COPY } from "./RepairView.tsx";
import type { ToastTone } from "./Toast.tsx";

const DETECT_FLASH_MS = 1200;

// Out-of-scope switches stay disabled with a stated reason rather than pretending
// to take effect. Telemetry is in the same bucket: the backend only flips an
// in-memory flag and never sends or persists anything.
const AUTO_UPDATE_NOTE = "自动安装尚未开放；有新版本时会在首页提醒";
const TELEMETRY_NOTE = "使用数据收集尚未开放";
const INVALID_APP_COPY = "所选应用无法识别为官方 Codex";

function errorCodeOf(error: unknown): string {
  return error instanceof LumioCommandError ? error.errorCode : "UNKNOWN";
}

function Toggle({
  checked,
  disabled,
  label,
  onToggle,
}: {
  checked: boolean;
  disabled?: boolean;
  label: string;
  onToggle?: () => void;
}) {
  return (
    <button
      aria-checked={checked}
      aria-label={label}
      className={`lumio-toggle${checked ? " is-on" : ""}`}
      disabled={disabled}
      onClick={onToggle}
      role="switch"
      type="button"
    >
      <span />
    </button>
  );
}

interface SettingsViewProps {
  autoUpdateEnabled: boolean;
  codexApp: LumioCodexApp | null;
  launchAtLoginEnabled: boolean;
  officialAppInstall: LumioOfficialAppInstall;
  signedIn: boolean;
  telemetryEnabled: boolean;
  onCodexAppChanged: (app: LumioCodexApp) => void;
  onLaunchAtLoginChanged: (enabled: boolean) => void;
  onSignOut: () => void;
  pushToast: (input: string, tone?: ToastTone) => void;
}

export function SettingsView({
  autoUpdateEnabled,
  codexApp,
  launchAtLoginEnabled,
  officialAppInstall,
  signedIn,
  telemetryEnabled: _telemetryEnabled,
  onCodexAppChanged,
  onLaunchAtLoginChanged,
  onSignOut,
  pushToast,
}: SettingsViewProps) {
  const [detecting, setDetecting] = useState(false);
  const [detectFailed, setDetectFailed] = useState(false);
  const [selectErrorCode, setSelectErrorCode] = useState<string | null>(null);
  // The reducer only carries a detected app once the home surface is online, so
  // the row shows the freshest local pick either way.
  const [pickedApp, setPickedApp] = useState<LumioCodexApp | null>(null);
  const [flashPath, setFlashPath] = useState(false);
  const [launchAtLoginBusy, setLaunchAtLoginBusy] = useState(false);
  const [restoreConfirmOpen, setRestoreConfirmOpen] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [restoring, setRestoring] = useState(false);

  useEffect(() => {
    if (!flashPath) return;
    const timer = setTimeout(() => setFlashPath(false), DETECT_FLASH_MS);
    return () => clearTimeout(timer);
  }, [flashPath]);

  const acceptApp = (app: LumioCodexApp) => {
    setDetectFailed(false);
    setSelectErrorCode(null);
    setFlashPath(true);
    setPickedApp(app);
    onCodexAppChanged(app);
  };

  const redetect = () => {
    setDetecting(true);
    void detectCodexApp()
      .then((app) => {
        if (app === null) {
          setDetectFailed(true);
          return;
        }
        acceptApp(app);
      })
      .catch((error: unknown) => {
        setDetectFailed(true);
        pushToast(errorCodeOf(error));
      })
      .finally(() => setDetecting(false));
  };

  const pickApp = () => {
    void open({ directory: false, multiple: false, title: shellLabels.officialAppPath })
      .then((picked) => {
        if (typeof picked !== "string") return undefined;
        return selectCodexApp(picked).then(acceptApp);
      })
      .catch((error: unknown) => {
        const code = errorCodeOf(error);
        setSelectErrorCode(code);
        pushToast(code);
      });
  };

  // 即点即生效：不做乐观翻转，命令返回后才推进状态；失败保持原状（= 规格的「失败回弹」）。
  const toggleLaunchAtLogin = () => {
    setLaunchAtLoginBusy(true);
    void setLaunchAtLogin(!launchAtLoginEnabled)
      .then((result) => onLaunchAtLoginChanged(result.enabled))
      .catch((error: unknown) => pushToast(lumioErrorLabel(errorCodeOf(error))))
      .finally(() => setLaunchAtLoginBusy(false));
  };

  const exportDiagnostics = () => {
    setExporting(true);
    void exportLogs()
      .then((result) => pushToast(`已导出到 ${result.path}`, "success"))
      .catch((error: unknown) => pushToast(errorCodeOf(error)))
      .finally(() => setExporting(false));
  };

  const restore = () => {
    setRestoreConfirmOpen(false);
    setRestoring(true);
    void restoreConfig()
      .then(() => pushToast("已恢复接管前的本机配置", "success"))
      .catch((error: unknown) => pushToast(errorCodeOf(error)))
      .finally(() => setRestoring(false));
  };

  return (
    <section className="lumio-settings-page">
      <div className="lumio-page-heading">
        <p className="lumio-eyebrow">桌面偏好</p>
        <h1>{shellLabels.settings}</h1>
        <p>这里只保留 Lumio Codex 运行所需的本机选项。</p>
      </div>

      <div className="lumio-settings-list">
        <article className="lumio-setting-row">
          <span className="lumio-setting-icon">
            <Rocket size={19} />
          </span>
          <div>
            <strong>{shellLabels.launchAtLogin}</strong>
            <p>登录电脑后自动启动 Lumio Codex（默认开启，可随时关闭）</p>
          </div>
          <Toggle
            checked={launchAtLoginEnabled}
            disabled={launchAtLoginBusy}
            label={shellLabels.launchAtLogin}
            onToggle={toggleLaunchAtLogin}
          />
        </article>

        <article className="lumio-setting-row">
          <span className="lumio-setting-icon">
            <Download size={19} />
          </span>
          <div>
            <strong>{shellLabels.automaticUpdates}</strong>
            <p>仅安装经过校验且适用于当前平台的版本</p>
            {autoUpdateEnabled ? null : <p className="lumio-setting-note">你仍会收到重要安全更新提示</p>}
            <p className="lumio-setting-note">{AUTO_UPDATE_NOTE}</p>
          </div>
          <Toggle checked={autoUpdateEnabled} disabled label={shellLabels.automaticUpdates} />
        </article>

        <article className="lumio-setting-row is-path-row">
          <span className="lumio-setting-icon">
            <Laptop size={19} />
          </span>
          <div>
            <strong>{shellLabels.officialAppPath}</strong>
            <p className={`lumio-path-value${flashPath ? " is-flash" : ""}`}>
              {isOfficialAppInstallInProgress(officialAppInstall)
                ? "正在安装官方应用…"
                : ((pickedApp ?? codexApp)?.path ?? "未自动检测到")}
            </p>
            {detectFailed ? <p className="lumio-field-error">未检测到，可手动选择</p> : null}
            {selectErrorCode === null ? null : (
              <p className="lumio-field-error">
                {selectErrorCode === "CODEX_APP_INVALID" ? INVALID_APP_COPY : lumioErrorLabel(selectErrorCode)}
              </p>
            )}
          </div>
          <span className="lumio-setting-actions">
            <button className="lumio-small-button" disabled={detecting} onClick={redetect} type="button">
              <RefreshCw className={detecting ? "lumio-spin" : undefined} size={15} />
              重新检测
            </button>
            {detectFailed ? (
              <button className="lumio-small-button" onClick={pickApp} type="button">
                手动选择…
              </button>
            ) : null}
          </span>
        </article>

        <article className="lumio-setting-row">
          <span className="lumio-setting-icon">
            <Activity size={19} />
          </span>
          <div>
            <strong>{shellLabels.telemetry}</strong>
            <p>默认关闭；开启后也只发送版本、平台、阶段和脱敏错误码</p>
            <p className="lumio-setting-note">{TELEMETRY_NOTE}</p>
          </div>
          <Toggle checked={false} disabled label={shellLabels.telemetry} />
        </article>

        <article className="lumio-setting-row">
          <span className="lumio-setting-icon">
            <FileArchive size={19} />
          </span>
          <div>
            <strong>{shellLabels.exportLogs}</strong>
            <p>导出前会再次扫描并移除敏感内容</p>
          </div>
          <button
            className="lumio-small-button"
            disabled={exporting}
            onClick={exportDiagnostics}
            type="button"
          >
            {exporting ? "正在导出…" : "导出"}
          </button>
        </article>

        <article className="lumio-setting-row">
          <span className="lumio-setting-icon is-warning">
            <RotateCcw size={19} />
          </span>
          <div>
            <strong>{shellLabels.restoreConfiguration}</strong>
            <p>把配置文件还原到接管前的状态，接管后在这个文件里的修改会丢失</p>
          </div>
          <button
            className="lumio-small-button is-warning"
            disabled={restoring}
            onClick={() => setRestoreConfirmOpen(true)}
            type="button"
          >
            {restoring ? "正在恢复…" : "恢复"}
          </button>
        </article>

        {signedIn ? (
          <article className="lumio-setting-row">
            <span className="lumio-setting-icon">
              <LogOut size={19} />
            </span>
            <div>
              <strong>退出登录</strong>
              <p>只清除本机保存的登录状态；本机配置保持现状，需要撤销接管请用上面的配置恢复</p>
            </div>
            <button className="lumio-small-button" onClick={onSignOut} type="button">
              退出登录
            </button>
          </article>
        ) : null}
      </div>

      <p className="lumio-settings-note">
        <ShieldCheck size={15} />
        不可用的选项会保持禁用，不会修改本机配置。
      </p>

      {restoreConfirmOpen ? (
        <div aria-modal="true" className="lumio-modal-backdrop" role="dialog">
          <div className="lumio-modal">
            <h3>恢复本机配置？</h3>
            <p>{RESTORE_CONFIRM_COPY}</p>
            <p>恢复后需要重新登录才能再次使用 Lumio 连接。</p>
            <div className="lumio-modal-actions">
              <button
                className="lumio-button is-secondary"
                onClick={() => setRestoreConfirmOpen(false)}
                type="button"
              >
                取消
              </button>
              <button className="lumio-button is-warning" onClick={restore} type="button">
                确认恢复
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </section>
  );
}
