import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";

/** 6.4: toasts last 4 seconds; ones carrying an undo stay for 10. */
export const TOAST_MS = 4000;
export const TOAST_UNDO_MS = 10_000;

export interface ToastAction {
  label: string;
  onClick: () => void;
}

export interface ToastOptions {
  action?: ToastAction;
  /** Same-kind toasts replace each other instead of stacking (6.4「同类合并」). */
  kind?: string;
  /** Runs when the toast disappears without the action being used. */
  onExpire?: () => void;
}

interface ToastRecord {
  id: number;
  message: string;
  action?: ToastAction;
  kind?: string;
  onExpire?: () => void;
}

interface ToastContextValue {
  toast: (message: string, options?: ToastOptions) => void;
  dismiss: (id: number) => void;
  toasts: ToastRecord[];
}

const ToastContext = createContext<ToastContextValue | null>(null);

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<ToastRecord[]>([]);
  const nextId = useRef(1);
  const timers = useRef(new Map<number, ReturnType<typeof setTimeout>>());

  const remove = useCallback((id: number, expire: boolean) => {
    const timer = timers.current.get(id);
    if (timer) {
      clearTimeout(timer);
      timers.current.delete(id);
    }
    setToasts((current) => {
      const target = current.find((entry) => entry.id === id);
      if (expire) target?.onExpire?.();
      return current.filter((entry) => entry.id !== id);
    });
  }, []);

  const toast = useCallback(
    (message: string, options: ToastOptions = {}) => {
      const id = nextId.current++;
      const record: ToastRecord = { id, message, ...options };
      setToasts((current) => [
        ...current.filter((entry) => !options.kind || entry.kind !== options.kind),
        record,
      ]);
      const lifetime = options.action ? TOAST_UNDO_MS : TOAST_MS;
      timers.current.set(
        id,
        setTimeout(() => remove(id, true), lifetime),
      );
    },
    [remove],
  );

  useEffect(() => {
    const pending = timers.current;
    return () => {
      pending.forEach(clearTimeout);
      pending.clear();
    };
  }, []);

  const value = useMemo(
    () => ({ toast, dismiss: (id: number) => remove(id, false), toasts }),
    [toast, remove, toasts],
  );

  return (
    <ToastContext.Provider value={value}>
      {children}
      <div className="toast-wrap" role="status" aria-live="polite">
        {toasts.map((entry) => (
          <div className="toast" key={entry.id}>
            <span>{entry.message}</span>
            {entry.action && (
              <button
                type="button"
                onClick={() => {
                  entry.action?.onClick();
                  remove(entry.id, false);
                }}
              >
                {entry.action.label}
              </button>
            )}
          </div>
        ))}
      </div>
    </ToastContext.Provider>
  );
}

export function useToast(): ToastContextValue {
  const context = useContext(ToastContext);
  if (!context) throw new Error("useToast 必须在 ToastProvider 内使用");
  return context;
}
