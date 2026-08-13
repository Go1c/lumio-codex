import { useCallback, useEffect, useId, useRef } from "react";
import type { ReactNode } from "react";
import { t } from "../i18n";

interface Props {
  title: string;
  body: ReactNode;
  /** 破坏性操作的后果说明，用醒目样式单独一行（如「该用户将立即被登出且无法登录。」）。 */
  warning?: string;
  confirmLabel: string;
  danger?: boolean;
  busy?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}

const FOCUSABLE = 'button:not([disabled]), [href], input:not([disabled]), [tabindex]:not([tabindex="-1"])';

/**
 * 二次确认模态。所有破坏性操作（禁用用户、退款）都必须走它（交互设计 7.5）。
 * 焦点圈定在模态内，Esc 可关闭（6.4 / 6.6）。
 */
export function ConfirmDialog({
  title,
  body,
  warning,
  confirmLabel,
  danger = false,
  busy = false,
  onConfirm,
  onCancel,
}: Props) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const confirmRef = useRef<HTMLButtonElement>(null);
  const titleID = useId();
  const bodyID = useId();

  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    confirmRef.current?.focus();
    // 关闭后把焦点还给触发它的按钮，键盘用户不会掉回页面顶部。
    return () => previous?.focus();
  }, []);

  const handleKeyDown = useCallback(
    (event: React.KeyboardEvent<HTMLDivElement>) => {
      if (event.key === "Escape" && !busy) {
        event.stopPropagation();
        onCancel();
        return;
      }
      if (event.key !== "Tab") return;

      const nodes = dialogRef.current?.querySelectorAll<HTMLElement>(FOCUSABLE);
      if (!nodes || nodes.length === 0) return;

      const first = nodes[0]!;
      const last = nodes[nodes.length - 1]!;
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    },
    [busy, onCancel],
  );

  return (
    <div className="modal-backdrop" onKeyDown={handleKeyDown}>
      <div
        className="modal modal-confirm"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleID}
        aria-describedby={bodyID}
        ref={dialogRef}
      >
        <h3 id={titleID}>{title}</h3>
        <div id={bodyID} className="modal-body">
          {body}
          {warning && <p className="modal-warning">{warning}</p>}
        </div>
        <div className="modal-actions">
          <button type="button" className="btn btn-secondary" onClick={onCancel} disabled={busy}>
            {t("common.cancel")}
          </button>
          <button
            type="button"
            ref={confirmRef}
            className={`btn ${danger ? "btn-danger-solid" : "btn-primary"}`}
            onClick={onConfirm}
            disabled={busy}
          >
            {busy && <span className="spinner" />}
            {busy ? t("common.processing") : confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}
