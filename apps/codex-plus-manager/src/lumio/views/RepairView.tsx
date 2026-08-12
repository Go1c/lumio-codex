import { AlertTriangle } from "lucide-react";
import { useState } from "react";

import { lumioErrorCopy, lumioErrorLabel } from "../errors.ts";
import { LumioCommandError, checkTakeover, exportLogs, restoreConfig } from "../invoke.ts";
import type { ToastTone } from "./Toast.tsx";

/** 恢复写回的是接管前的整份快照，不是逐字段撤销，二次确认必须说清这个后果。 */
export const RESTORE_CONFIRM_COPY =
  "恢复会把本机的 Codex 配置文件整份还原到 Lumio 接管前的状态：接管之后你在这个文件里做的修改都会丢失，包括你自己新增或改过的设置。";
const CREDENTIAL_CODES = ["AUTH_SESSION_EXPIRED", "KEY_STORAGE_UNAVAILABLE"];

function errorCodeOf(error: unknown): string {
  return error instanceof LumioCommandError ? error.errorCode : "UNKNOWN";
}

function explain(code: string | null): string {
  if (code !== null && CREDENTIAL_CODES.includes(code)) {
    return "本机保存的登录凭据已失效，请重新登录。你的本机配置与官方 Codex 均未被修改。";
  }
  if (code === "CODEX_CONFIG_CONFLICT") {
    return "检测到本机配置被其他工具修改过。为保护你的设置，Lumio 不会自动覆盖这些改动——你可以重新检查，或恢复到接管前的原始配置。";
  }
  return `${lumioErrorCopy(code)}。本机配置尚未被修改，你可以重新检查或恢复到接管前的原始配置。`;
}

interface RepairViewProps {
  errorCode: string | null;
  onResolved: () => void;
  onSignOut: () => void;
  pushToast: (input: string, tone?: ToastTone) => void;
}

export function RepairView({ errorCode, onResolved, onSignOut, pushToast }: RepairViewProps) {
  const [checking, setChecking] = useState(false);
  const [restoring, setRestoring] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [unresolvedCode, setUnresolvedCode] = useState<string | null>(null);

  const credentialProblem = errorCode !== null && CREDENTIAL_CODES.includes(errorCode);

  const recheck = () => {
    setChecking(true);
    setUnresolvedCode(null);
    void checkTakeover()
      .then((health) => {
        if (health.errorCode === null && health.health === "healthy") {
          onResolved();
          return;
        }
        setUnresolvedCode(health.errorCode ?? errorCode);
      })
      .catch((error: unknown) => setUnresolvedCode(errorCodeOf(error)))
      .finally(() => setChecking(false));
  };

  const restore = () => {
    setConfirmOpen(false);
    setRestoring(true);
    setUnresolvedCode(null);
    void restoreConfig()
      .then(() => {
        pushToast("已恢复接管前的本机配置", "success");
        onSignOut();
      })
      .catch((error: unknown) => setUnresolvedCode(errorCodeOf(error)))
      .finally(() => setRestoring(false));
  };

  const diagnose = () => {
    setExporting(true);
    void exportLogs()
      .then((result) => pushToast(`已导出到 ${result.path}`, "success"))
      .catch((error: unknown) => pushToast(errorCodeOf(error)))
      .finally(() => setExporting(false));
  };

  return (
    <section className="lumio-repair">
      <div className="lumio-repair-card">
        <span className="lumio-panel-icon is-warning">
          <AlertTriangle size={22} />
        </span>
        <div>
          <p className="lumio-eyebrow">启动检查发现问题</p>
          <h1>需要检查配置</h1>
          <p>{explain(errorCode)}</p>
          <p>
            <code className="lumio-code-chip">{errorCode ?? "UNKNOWN"}</code>
          </p>

          <div className="lumio-repair-actions">
            {credentialProblem ? (
              <button className="lumio-button is-primary" onClick={onSignOut} type="button">
                重新登录
              </button>
            ) : (
              <button
                className="lumio-button is-primary"
                disabled={checking}
                onClick={recheck}
                type="button"
              >
                {checking ? "正在检查…" : "重新检查"}
              </button>
            )}
            <button
              className="lumio-button is-warning"
              disabled={restoring}
              onClick={() => setConfirmOpen(true)}
              type="button"
            >
              {restoring ? "正在恢复…" : "恢复本机配置"}
            </button>
            <button
              className="lumio-button is-secondary"
              disabled={exporting}
              onClick={diagnose}
              type="button"
            >
              {exporting ? "正在导出…" : "导出诊断日志"}
            </button>
          </div>

          {unresolvedCode === null ? null : (
            <p className="lumio-banner" role="alert">
              问题仍未解决，已保留原始快照与诊断信息，可再次尝试。{lumioErrorLabel(unresolvedCode)}
            </p>
          )}

          <p className="lumio-settings-note">
            导出前会再次扫描并移除敏感内容；修复过程不会删除最后可恢复的配置副本。
          </p>
        </div>
      </div>

      {confirmOpen ? (
        <div aria-modal="true" className="lumio-modal-backdrop" role="dialog">
          <div className="lumio-modal">
            <h3>恢复本机配置？</h3>
            <p>{RESTORE_CONFIRM_COPY}</p>
            <p>恢复后需要重新登录才能再次使用 Lumio 连接。</p>
            <div className="lumio-modal-actions">
              <button
                className="lumio-button is-secondary"
                onClick={() => setConfirmOpen(false)}
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
