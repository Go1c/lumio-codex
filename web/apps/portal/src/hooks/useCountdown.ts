import { useEffect, useRef, useState } from "react";

/** 验证码重发倒计时。秒数由服务端下发，本地只负责递减。 */
export function useCountdown(): [number, (seconds: number) => void] {
  const [remaining, setRemaining] = useState(0);
  const timer = useRef<ReturnType<typeof setInterval> | null>(null);

  useEffect(() => {
    if (remaining <= 0) {
      if (timer.current) {
        clearInterval(timer.current);
        timer.current = null;
      }
      return;
    }
    if (timer.current) return;

    timer.current = setInterval(() => {
      setRemaining((value) => (value <= 1 ? 0 : value - 1));
    }, 1000);

    return () => {
      if (timer.current) {
        clearInterval(timer.current);
        timer.current = null;
      }
    };
  }, [remaining]);

  return [remaining, setRemaining];
}
