import { AlertTriangle, CheckCircle2, Info, X } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";

import { lumioErrorLabel } from "../errors.ts";

const MAX_VISIBLE_TOASTS = 3;
const TOAST_LIFETIME_MS = 4000;
const ERROR_CODE_PATTERN = /^[A-Z][A-Z0-9_]*$/;

export type ToastTone = "info" | "success" | "error";

export interface LumioToast {
  id: number;
  text: string;
  tone: ToastTone;
}

interface ToastRequest {
  text: string;
  tone: ToastTone;
}

/**
 * A bare stable error code is expanded into "人话（错误码）"; anything else is
 * already user-facing copy and is shown as written.
 */
function resolveRequest(input: string, tone: ToastTone | undefined): ToastRequest {
  if (ERROR_CODE_PATTERN.test(input)) {
    return { text: lumioErrorLabel(input), tone: tone ?? "error" };
  }
  return { text: input, tone: tone ?? "info" };
}

export interface ToastController {
  toasts: LumioToast[];
  pushToast: (input: string, tone?: ToastTone) => void;
  dismiss: (id: number) => void;
}

export function useToasts(): ToastController {
  const [toasts, setToasts] = useState<LumioToast[]>([]);
  const nextId = useRef(1);

  const dismiss = useCallback((id: number) => {
    setToasts((current) => current.filter((toast) => toast.id !== id));
  }, []);

  const pushToast = useCallback((input: string, tone?: ToastTone) => {
    const request = resolveRequest(input, tone);
    const id = nextId.current;
    nextId.current += 1;
    setToasts((current) =>
      // Same-text toasts merge into the newest one instead of stacking up.
      [...current.filter((toast) => toast.text !== request.text), { id, ...request }].slice(
        -MAX_VISIBLE_TOASTS,
      ),
    );
  }, []);

  return { toasts, pushToast, dismiss };
}

function ToastIcon({ tone }: { tone: ToastTone }) {
  if (tone === "error") return <AlertTriangle size={15} />;
  if (tone === "success") return <CheckCircle2 size={15} />;
  return <Info size={15} />;
}

function ToastRow({ toast, onDismiss }: { toast: LumioToast; onDismiss: (id: number) => void }) {
  useEffect(() => {
    const timer = setTimeout(() => onDismiss(toast.id), TOAST_LIFETIME_MS);
    return () => clearTimeout(timer);
  }, [onDismiss, toast.id]);

  return (
    <div className={`lumio-toast is-${toast.tone}`}>
      <ToastIcon tone={toast.tone} />
      <span>{toast.text}</span>
      <button aria-label="关闭提示" onClick={() => onDismiss(toast.id)} type="button">
        <X size={13} />
      </button>
    </div>
  );
}

export function ToastHost({ toasts, onDismiss }: { toasts: LumioToast[]; onDismiss: (id: number) => void }) {
  if (toasts.length === 0) return null;

  return (
    <div aria-live="polite" className="lumio-toasts">
      {toasts.map((toast) => (
        <ToastRow key={toast.id} onDismiss={onDismiss} toast={toast} />
      ))}
    </div>
  );
}
