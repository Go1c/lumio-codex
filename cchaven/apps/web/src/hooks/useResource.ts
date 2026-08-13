import { useCallback, useEffect, useRef, useState } from "react";

export type ResourceStatus = "loading" | "success" | "error";

export interface Resource<T> {
  status: ResourceStatus;
  data: T | undefined;
  error: unknown;
  reload: () => void;
  /** 就地替换数据，用于提交后同步 UI，避免整块重新骨架化。 */
  setData: (updater: T | ((prev: T | undefined) => T)) => void;
}

/**
 * 统一的「五态」数据加载：loading / success / error（empty 由调用方按数据判断）。
 * 组件卸载或依赖变化时 abort 掉在途请求，避免竞态覆盖新数据。
 */
export function useResource<T>(
  loader: (signal: AbortSignal) => Promise<T>,
  deps: unknown[] = [],
  options: { enabled?: boolean } = {},
): Resource<T> {
  const enabled = options.enabled ?? true;

  const [status, setStatus] = useState<ResourceStatus>(enabled ? "loading" : "success");
  const [data, setRawData] = useState<T | undefined>(undefined);
  const [error, setError] = useState<unknown>(undefined);
  const [nonce, setNonce] = useState(0);

  const loaderRef = useRef(loader);
  loaderRef.current = loader;

  useEffect(() => {
    if (!enabled) {
      setStatus("success");
      return;
    }

    const controller = new AbortController();
    let active = true;

    setStatus("loading");
    setError(undefined);

    loaderRef
      .current(controller.signal)
      .then((result) => {
        if (!active) return;
        setRawData(result);
        setStatus("success");
      })
      .catch((err) => {
        if (!active || controller.signal.aborted) return;
        setError(err);
        setStatus("error");
      });

    return () => {
      active = false;
      controller.abort();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [enabled, nonce, ...deps]);

  const reload = useCallback(() => setNonce((n) => n + 1), []);

  const setData = useCallback((updater: T | ((prev: T | undefined) => T)) => {
    setRawData((prev) => (typeof updater === "function" ? (updater as (p: T | undefined) => T)(prev) : updater));
  }, []);

  return { status, data, error, reload, setData };
}
