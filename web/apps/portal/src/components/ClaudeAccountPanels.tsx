import { useEffect, useState } from "react";

import { ErrorBlock, LoadingBlock, SectionCard, siteUrl } from "@lumio/ui";

import {
  fetchClaudeEntitlement,
  fetchClaudeOrders,
  messageOfControlError,
  type ClaudeEntitlement,
  type ClaudeOrder,
} from "@/lib/ccControl";
import { fetchBalanceTransactions, type BalanceTransaction } from "@/lib/lumioWallet";

export function formatLocalCalendarDate(input: string | null | undefined): string {
  if (!input) return "—";
  const date = new Date(input);
  if (Number.isNaN(date.getTime())) return "—";
  return `${date.getFullYear()}年${date.getMonth() + 1}月${date.getDate()}日`;
}

export function formatCentsYuan(cents: number): string {
  return `¥${(cents / 100).toFixed(2)}`;
}

export function formatClaudeEntitlementLine(entitlement: ClaudeEntitlement): string {
  switch (entitlement.status) {
    case "active":
      return `已订阅 · 有效期至 ${formatLocalCalendarDate(entitlement.expiresAt)}（剩余 ${entitlement.daysLeft} 天）`;
    case "trialing":
      return `免费试用中 · 剩余 ${entitlement.daysLeft} 天`;
    case "expired":
      return "订阅已过期";
    default:
      return "未订阅";
  }
}

function channelLabel(channel: string): string {
  if (channel === "balance") return "余额";
  if (channel === "alipay") return "支付宝";
  if (channel === "wechat") return "微信支付";
  return channel || "—";
}

function orderStatusLabel(status: string): string {
  if (status === "paid") return "已支付";
  if (status === "pending") return "处理中，请勿重复支付";
  if (status === "failed") return "失败";
  return status || "—";
}

export function ClaudeAccountPanels({ accessToken }: { accessToken?: string }) {
  if (!accessToken) return null;
  return (
    <>
      <ClaudeSubscriptionCard accessToken={accessToken} />
      <ClaudeOrdersCard accessToken={accessToken} />
      <BalanceTransactionsCard accessToken={accessToken} />
    </>
  );
}

function ClaudeSubscriptionCard({ accessToken }: { accessToken: string }) {
  const [entitlement, setEntitlement] = useState<ClaudeEntitlement | null>(null);
  const [orders, setOrders] = useState<ClaudeOrder[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [nonce, setNonce] = useState(0);

  useEffect(() => {
    let cancelled = false;
    setError(null);
    Promise.all([fetchClaudeEntitlement(accessToken), fetchClaudeOrders(accessToken)])
      .then(([nextEntitlement, nextOrders]) => {
        if (cancelled) return;
        setEntitlement(nextEntitlement);
        setOrders(nextOrders);
      })
      .catch((failure) => {
        if (!cancelled) setError(messageOfControlError(failure));
      });
    return () => {
      cancelled = true;
    };
  }, [accessToken, nonce]);

  return (
    <SectionCard title="Claude 订阅" id="claude-subscription">
      {error ? (
        <ErrorBlock message={error} onRetry={() => setNonce((n) => n + 1)} />
      ) : !entitlement ? (
        <LoadingBlock label="读取 Claude 订阅…" lines={2} />
      ) : (
        <>
          <p className={`claude-entitlement-line${entitlement.expiringSoon ? " is-expiring" : ""}`}>
            {formatClaudeEntitlementLine(entitlement)}
            {entitlement.expiringSoon ? <span> 即将到期，请及时续期。</span> : null}
          </p>
          {(entitlement.status === "none" || entitlement.status === "expired") &&
          orders?.some((order) => order.status === "pending") ? (
            <p className="note">
              钱可能已扣、权益尚未到账，请勿重复支付。账单里的处理中订单是同一笔开通。
            </p>
          ) : null}
          {entitlement.status === "none" || entitlement.status === "expired" ? (
            <p className="note">
              请在 BestCodex 桌面 Claude Tab 用余额开通。门户不扣款。
              {" "}
              <a href={siteUrl("cc")}>了解 Claude</a>
            </p>
          ) : null}
        </>
      )}
    </SectionCard>
  );
}

function ClaudeOrdersCard({ accessToken }: { accessToken: string }) {
  const [orders, setOrders] = useState<ClaudeOrder[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [nonce, setNonce] = useState(0);

  useEffect(() => {
    let cancelled = false;
    setError(null);
    fetchClaudeOrders(accessToken)
      .then((items) => {
        if (!cancelled) setOrders(items);
      })
      .catch((failure) => {
        if (!cancelled) setError(messageOfControlError(failure));
      });
    return () => {
      cancelled = true;
    };
  }, [accessToken, nonce]);

  return (
    <SectionCard title="账单 / 开通记录" id="claude-orders">
      {error ? (
        <ErrorBlock message={error} onRetry={() => setNonce((n) => n + 1)} />
      ) : orders === null ? (
        <LoadingBlock label="读取开通记录…" lines={3} />
      ) : orders.length === 0 ? (
        <p className="note">还没有开通记录。</p>
      ) : (
        <ul className="plain-list claude-order-list">
          {orders.map((order) => (
            <li key={order.orderNo}>
              <span className="mono">{order.orderNo}</span>
              {" · "}
              {formatCentsYuan(order.amountCents)}
              {" · "}
              {channelLabel(order.channel)}
              {" · "}
              {orderStatusLabel(order.status)}
              {" · "}
              {formatLocalCalendarDate(order.createdAt)}
            </li>
          ))}
        </ul>
      )}
    </SectionCard>
  );
}

function BalanceTransactionsCard({ accessToken }: { accessToken: string }) {
  const [items, setItems] = useState<BalanceTransaction[] | null>(null);
  const [hidden, setHidden] = useState(false);

  useEffect(() => {
    let cancelled = false;
    fetchBalanceTransactions(accessToken)
      .then((rows) => {
        if (!cancelled) setItems(rows);
      })
      .catch(() => {
        if (!cancelled) setHidden(true);
      });
    return () => {
      cancelled = true;
    };
  }, [accessToken]);

  if (hidden) return null;
  return (
    <SectionCard title="余额流水" id="balance-transactions">
      {items === null ? (
        <LoadingBlock label="读取余额流水…" lines={2} />
      ) : items.length === 0 ? (
        <p className="note">暂无流水。这是钱包记录，不是 Claude 订阅时长。</p>
      ) : (
        <ul className="plain-list claude-order-list">
          {items.map((row, index) => (
            <li key={`${row.ref}-${row.createdAt}-${index}`}>
              {row.purpose || "—"}
              {" · "}
              {row.ref || "—"}
              {" · "}
              {`¥${row.amount.toFixed(2)}`}
              {row.createdAt ? ` · ${formatLocalCalendarDate(row.createdAt)}` : ""}
            </li>
          ))}
        </ul>
      )}
    </SectionCard>
  );
}
