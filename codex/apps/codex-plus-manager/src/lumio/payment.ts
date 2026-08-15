import type { LumioState } from "./state.ts";

/** 充值页 URL。服务设置缺失（apiBaseUrl 未知）时返回 null，调用方按发起失败兜底。 */
export function paymentUrl(state: LumioState): string | null {
  const base = state.service?.apiBaseUrl?.replace(/\/$/, "");
  const path = state.service?.paymentPath ?? "/purchase";
  if (!base) return null;
  return `${base}${path.startsWith("/") ? path : `/${path}`}`;
}
