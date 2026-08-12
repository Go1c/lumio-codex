import {
  useEffect,
  useRef,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
} from "react";
import { t } from "../i18n";
import type { SyncState } from "../lib/types";

export function Spinner({ dark = false }: { dark?: boolean }) {
  return <span className={dark ? "spinner dark" : "spinner"} aria-hidden="true" />;
}

/** 6.3 状态点：颜色之外始终带文字标签，不只靠颜色区分（6.6）。 */
export function StatusDot({ state, label }: { state: SyncState; label?: string }) {
  const text = label ?? t(`sync.label.${state}`);
  return (
    <>
      <span className={`dot ${state}`} aria-hidden="true" />
      <span className="visually-hidden">{text}</span>
    </>
  );
}

export function Banner({
  tone,
  children,
  action,
  block = false,
}: {
  tone: "error" | "warn" | "ok";
  children: ReactNode;
  action?: ReactNode;
  block?: boolean;
}) {
  return (
    <div className={`banner ${tone}${block ? " block" : ""}`} role={tone === "error" ? "alert" : undefined}>
      <div>{children}</div>
      {action}
    </div>
  );
}

/**
 * Modal with Esc-to-close and a focus trap (6.4 / 6.6).
 *
 * `dismissible` is false while a deployment is running: 6.4 exempts exactly
 * that case from Esc.
 */
export function Modal({
  title,
  onClose,
  dismissible = true,
  small = false,
  children,
}: {
  title: string;
  onClose: () => void;
  dismissible?: boolean;
  small?: boolean;
  children: ReactNode;
}) {
  const dialogRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape" && dismissible) {
        event.stopPropagation();
        onClose();
      }
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [dismissible, onClose]);

  useEffect(() => {
    const focusable = dialogRef.current?.querySelector<HTMLElement>(
      "input, select, textarea, button, [href], [tabindex]:not([tabindex='-1'])",
    );
    focusable?.focus();
  }, []);

  function trapFocus(event: ReactKeyboardEvent<HTMLDivElement>) {
    if (event.key !== "Tab" || !dialogRef.current) return;
    const focusable = Array.from(
      dialogRef.current.querySelectorAll<HTMLElement>(
        "input:not([disabled]), select:not([disabled]), textarea:not([disabled]), button:not([disabled]), [href], summary, [tabindex]:not([tabindex='-1'])",
      ),
    ).filter((element) => element.offsetParent !== null);
    if (focusable.length === 0) return;

    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  }

  return (
    <div
      className="modal-backdrop"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget && dismissible) onClose();
      }}
    >
      <div
        className={small ? "modal small" : "modal"}
        role="dialog"
        aria-modal="true"
        aria-label={title}
        ref={dialogRef}
        onKeyDown={trapFocus}
      >
        {children}
      </div>
    </div>
  );
}

export interface MenuItem {
  label: string;
  onSelect: () => void;
  danger?: boolean;
  separatorBefore?: boolean;
}

/** Right-click / overflow menu; closes on outside click or Esc. */
export function ContextMenu({
  x,
  y,
  items,
  onClose,
  label,
}: {
  x: number;
  y: number;
  items: MenuItem[];
  onClose: () => void;
  label: string;
}) {
  useEffect(() => {
    function close() {
      onClose();
    }
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") onClose();
    }
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [onClose]);

  return (
    <div
      className="ctx-menu"
      style={{ left: x, top: y }}
      role="menu"
      aria-label={label}
      onMouseDown={(event) => event.stopPropagation()}
    >
      {items.map((item) => (
        <div key={item.label}>
          {item.separatorBefore && <div className="ctx-sep" />}
          <button
            type="button"
            role="menuitem"
            className={item.danger ? "ctx-item danger" : "ctx-item"}
            onClick={() => {
              item.onSelect();
              onClose();
            }}
          >
            {item.label}
          </button>
        </div>
      ))}
    </div>
  );
}
