import { t } from "../i18n";

/** DASH 是缺数占位符。指标缺数时显示它而不是 0，避免把「没数据」误读成「为零」。 */
export const DASH = "—";

function parse(value: string | null | undefined): Date | null {
  if (!value) return null;
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? null : date;
}

const pad = (n: number) => String(n).padStart(2, "0");

/** 6.5 节的展示日期格式：YYYY年M月D日。 */
export function formatDate(value: string | null | undefined): string {
  const date = parse(value);
  if (!date) return DASH;
  return `${date.getFullYear()}年${date.getMonth() + 1}月${date.getDate()}日`;
}

/** 表格内时间用紧凑格式 YYYY-MM-DD HH:mm，与原型一致。 */
export function formatDateTime(value: string | null | undefined): string {
  const date = parse(value);
  if (!date) return DASH;
  return (
    `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ` +
    `${pad(date.getHours())}:${pad(date.getMinutes())}`
  );
}

/** 柱状图横轴用 M/D，末位由调用方替换为「今天」。 */
export function formatDayAxis(value: string): string {
  const date = parse(value);
  if (!date) return DASH;
  return `${date.getMonth() + 1}/${date.getDate()}`;
}

/** 「最近活跃」列：近的用相对时间，远的落回日期，与原型的观感一致。 */
export function formatRelative(value: string | null | undefined, now: Date = new Date()): string {
  const date = parse(value);
  if (!date) return DASH;

  const seconds = Math.floor((now.getTime() - date.getTime()) / 1000);
  if (seconds < 60) return "刚刚";
  if (seconds < 3600) return `${Math.floor(seconds / 60)} 分钟前`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)} 小时前`;
  if (seconds < 172800) return "昨天";
  if (seconds < 30 * 86400) return `${Math.floor(seconds / 86400)} 天前`;
  return formatDate(value);
}

export function formatCount(value: number | null | undefined): string {
  if (value === null || value === undefined) return DASH;
  return Math.round(value).toLocaleString("zh-CN");
}

/** 金额：分 → 「1,234.00」，表格与确认弹窗用它。 */
export function formatAmount(cents: number | null | undefined): string {
  if (cents === null || cents === undefined) return DASH;
  return (cents / 100).toLocaleString("zh-CN", {
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  });
}

/** 金额紧凑写法：整元时省略小数，用于收入卡与当日汇总。 */
export function formatAmountCompact(cents: number | null | undefined): string {
  if (cents === null || cents === undefined) return DASH;
  const yuan = cents / 100;
  return yuan.toLocaleString("zh-CN", {
    minimumFractionDigits: 0,
    maximumFractionDigits: Number.isInteger(yuan) ? 0 : 2,
  });
}

/** 比率 0.382 → 「38.2%」。 */
export function formatRate(value: number | null | undefined): string {
  if (value === null || value === undefined) return DASH;
  return `${(value * 100).toFixed(1)}%`;
}

/** 环比/变化量带符号：0.058 → 「+5.8%」，-0.012 → 「-1.2%」。 */
export function formatDelta(value: number | null | undefined): string {
  if (value === null || value === undefined) return DASH;
  const pct = value * 100;
  const sign = pct > 0 ? "+" : "";
  return `${sign}${pct.toFixed(1)}%`;
}

/** 空串或 null 一律显示「—」，避免表格出现空格子。 */
export function orDash(value: string | null | undefined): string {
  return value ? value : DASH;
}

/** 「使用平台」列：未登录过 APP 的用户后端返回空串。 */
export function formatPlatform(platform: string | null | undefined): string {
  return platform ? platform : t("users.platform.none");
}
