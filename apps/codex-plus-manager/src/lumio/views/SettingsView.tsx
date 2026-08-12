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
  setTelemetry,
  shellLabels,
} from "../invoke.ts";
import type { LumioCodexApp } from "../types.ts";
import { RESTORE_CONFIRM_COPY } from "./RepairView.tsx";
import type { ToastTone } from "./Toast.tsx";

const DETECT_FLASH_MS = 1200;

// Neither switch has a command behind it this cycle, so both stay disabled with
// a stated reason rather than pretending to take effect.
const LAUNCH_AT_LOGIN_NOTE = "本机开机启动尚未开放";
const AUTO_UPDATE_NOTE = "自动更新尚未开放";
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
  signedIn: boolean;
  telemetryEnabled: boolean;
  onCodexAppChanged: (app: LumioCodexApp) => void;
  onSignOut: () => void;
  pushToast: (input: string, tone?: ToastTone) => void;
}

export function SettingsView({
  autoUpdateEnabled,
  codexApp,
  signedIn,
  telemetryEnabled,
  onCodexAppChanged,
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
  const [telemetryOn, setTelemetryOn] = useState(telemetryEnabled);
  const [telemetryConfirmOpen, setTelemetryConfirmOpen] = useState(false);
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

  const applyTelemetry = (enabled: boolean) => {
    void setTelemetry(enabled)
      .then((result) => setTelemetryOn(result.enabled))
      .catch((error: unknown) => {
        setTelemetryOn(!enabled);
        pushToast(errorCodeOf(error));
      });
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
            <p>登录电脑后自动准备 Lumio Codex</p>
            <p className="lumio-setting-note">{LAUNCH_AT_LOGIN_NOTE}</p>
          </div>
          <Toggle checked={false} disabled label={shellLabels.launchAtLogin} />
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
              {(pickedApp ?? codexApp)?.path ?? "未自动检测到"}
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
          </div>
          <Toggle
            checked={telemetryOn}
            label={shellLabels.telemetry}
            onToggle={() => {
              if (telemetryOn) {
                setTelemetryOn(false);
                applyTelemetry(false);
                return;
              }
              setTelemetryConfirmOpen(true);
            }}
          />
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
            <p>撤销 Lumio 管理的字段并保留其他本机设置</p>
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

      {telemetryConfirmOpen ? (
        <div aria-modal="true" className="lumio-modal-backdrop" role="dialog">
          <div className="lumio-modal">
            <h3>开启{shellLabels.telemetry}？</h3>
            <p>开启后只发送以下四类信息，用于改进产品稳定性：</p>
            <ul>
              <li>客户端版本</li>
              <li>操作系统平台与架构</li>
              <li>启动阶段</li>
              <li>脱敏后的错误码</li>
            </ul>
            <p>永远不会发送：邮箱、任何凭据、提示词、代码、文件路径或请求内容。</p>
            <div className="lumio-modal-actions">
              <button
                className="lumio-button is-secondary"
                onClick={() => setTelemetryConfirmOpen(false)}
                type="button"
              >
                取消
              </button>
              <button
                className="lumio-button is-primary"
                onClick={() => {
                  setTelemetryConfirmOpen(false);
                  setTelemetryOn(true);
                  applyTelemetry(true);
                }}
                type="button"
              >
                确认开启
              </button>
            </div>
          </div>
        </div>
      ) : null}

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
