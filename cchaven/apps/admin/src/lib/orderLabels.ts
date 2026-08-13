import type { OrderStatus } from "../api/types";
import { t } from "../i18n";

/** 订单状态徽标配色，订单页与用户详情页共用同一套。 */
export const ORDER_STATUS_TONE: Record<OrderStatus, "green" | "orange" | "gray" | "red"> = {
  pending: "gray",
  paid: "green",
  refunding: "orange",
  refunded: "gray",
  failed: "red",
};

export function orderStatusLabel(status: OrderStatus): string {
  return status === "pending" ? t("orders.status.pending") : t(`orders.filter.${status}`);
}

export function orderChannelLabel(channel: string): string {
  switch (channel) {
    case "alipay":
      return t("orders.channel.alipay");
    case "wechat":
      return t("orders.channel.wechat");
    case "card":
      return t("orders.channel.card");
    case "mock":
      return t("orders.channel.mock");
    default:
      return channel;
  }
}
