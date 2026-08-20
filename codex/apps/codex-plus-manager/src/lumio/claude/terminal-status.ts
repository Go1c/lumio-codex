export const TERMINAL_FAIL_BANNER = "没能打开终端。";

export function terminalBanner(
  opened: boolean,
  hasOutput: boolean,
  current?: string | null,
): string | null {
  if (hasOutput) return null;
  if (!opened) return TERMINAL_FAIL_BANNER;
  if (!current || current === TERMINAL_FAIL_BANNER) return null;
  return current;
}
