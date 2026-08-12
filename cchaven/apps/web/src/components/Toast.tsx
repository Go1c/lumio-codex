import { createContext, useCallback, useContext, useMemo, useRef, useState, type ReactNode } from "react";

/**
 * 6.4 节：toast 为非阻断结果通知，4 秒自动消失；带撤销动作的停留 10 秒。
 * 使用 role="status" + aria-live="polite"，屏幕阅读器会朗读但不打断当前操作。
 */

interface ToastItem {
  id: number;
  text: string;
  actionLabel?: string;
  onAction?: () => void;
}

interface ToastContextValue {
  toast: (text: string, action?: { label: string; onAction: () => void }) => void;
}

const ToastContext = createContext<ToastContextValue>({ toast: () => {} });

export function ToastProvider({ children }: { children: ReactNode }) {
  const [items, setItems] = useState<ToastItem[]>([]);
  const nextId = useRef(1);

  const dismiss = useCallback((id: number) => {
    setItems((list) => list.filter((item) => item.id !== id));
  }, []);

  const toast = useCallback<ToastContextValue["toast"]>(
    (text, action) => {
      const id = nextId.current++;
      setItems((list) => [...list, { id, text, actionLabel: action?.label, onAction: action?.onAction }]);
      setTimeout(() => dismiss(id), action ? 10000 : 4000);
    },
    [dismiss],
  );

  const value = useMemo(() => ({ toast }), [toast]);

  return (
    <ToastContext.Provider value={value}>
      {children}
      <div className="toast-wrap" role="status" aria-live="polite">
        {items.map((item) => (
          <div key={item.id} className="toast">
            <span>{item.text}</span>
            {item.actionLabel && (
              <button
                type="button"
                onClick={() => {
                  item.onAction?.();
                  dismiss(item.id);
                }}
              >
                {item.actionLabel}
              </button>
            )}
          </div>
        ))}
      </div>
    </ToastContext.Provider>
  );
}

export function useToast() {
  return useContext(ToastContext).toast;
}
