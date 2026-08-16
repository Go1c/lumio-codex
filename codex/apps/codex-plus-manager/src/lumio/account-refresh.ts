/** 在线时的余额定时轮询周期。 */
export const ACCOUNT_AUTO_REFRESH_MS = 60_000;

/**
 * 窗口从托盘唤起后的补刷最小间隔：托盘反复点击/聚焦抖动不允许打爆
 * `/api/v1/auth/me`，距上次成功同步太近就等下一拍。
 */
export const WINDOW_SHOWN_REFRESH_MIN_GAP_MS = 10_000;

/** Rust 侧 `show_main_window` 发出的事件名，前端监听后触发余额补刷。 */
export const WINDOW_SHOWN_EVENT = "lumio://window-shown";

/**
 * 是否值得再拉一次余额：`cachedAt` 是会话内最近一次成功同步的时间
 * （`account-refreshed` 的唯一真值），从未同步过或距上次已超过 `minGapMs`
 * 才动请求。
 */
export function shouldAutoRefresh(
  cachedAt: string | null,
  nowMs: number,
  minGapMs: number,
): boolean {
  if (cachedAt === null) return true;
  const lastMs = new Date(cachedAt).getTime();
  if (Number.isNaN(lastMs)) return true;
  return nowMs - lastMs >= minGapMs;
}
