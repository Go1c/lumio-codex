import { useCallback, useEffect, useRef, useState } from "react";

/**
 * 秒级倒计时，用于验证码 60 秒重发冷却与登录锁定倒计时。
 * 返回剩余秒数与重置函数；0 表示可以再次操作。
 *
 * 以「截止时间戳 + 单个 interval」实现而不是逐次 setTimeout：
 * 标签页被挂起后回来不会少算秒数，也不会因每次重渲染重建定时器。
 */
export function useCountdown(initialSeconds = 0): [number, (seconds: number) => void] {
  const [remaining, setRemaining] = useState(Math.max(0, Math.ceil(initialSeconds)));
  const deadlineRef = useRef(Date.now() + Math.max(0, initialSeconds) * 1000);

  const active = remaining > 0;

  useEffect(() => {
    if (!active) return;

    const timer = setInterval(() => {
      const left = Math.max(0, Math.ceil((deadlineRef.current - Date.now()) / 1000));
      setRemaining(left);
      if (left <= 0) clearInterval(timer);
    }, 1000);

    return () => clearInterval(timer);
  }, [active]);

  const start = useCallback((seconds: number) => {
    const normalized = Math.max(0, Math.ceil(seconds));
    deadlineRef.current = Date.now() + normalized * 1000;
    setRemaining(normalized);
  }, []);

  return [remaining, start];
}
