import type { ClaudeEntitlement, ClaudeEntitlementStatus } from "./types.ts";

export function formatLocalCalendarDate(input: string | null | undefined): string {
  if (!input) return "—";
  const date = new Date(input);
  if (Number.isNaN(date.getTime())) return "—";
  return `${date.getFullYear()}年${date.getMonth() + 1}月${date.getDate()}日`;
}

export function formatClaudeEntitlementLine(
  entitlement: Pick<ClaudeEntitlement, "status" | "expiresAt" | "daysLeft">,
): string {
  const n = entitlement.daysLeft ?? 0;
  switch (entitlement.status) {
    case "active":
      return `已订阅 · 有效期至 ${formatLocalCalendarDate(entitlement.expiresAt)}（剩余 ${n} 天）`;
    case "trialing":
      return `免费试用中 · 剩余 ${n} 天`;
    case "expired":
      return "订阅已过期";
    default:
      return "未订阅";
  }
}

export function claudeEntitlementHeadline(status: ClaudeEntitlementStatus): string {
  if (status === "expired") return "订阅已过期";
  return "未订阅";
}
