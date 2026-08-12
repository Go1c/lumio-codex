import { useCallback, useEffect, useState } from "react";
import { ApiError } from "../api/client";
import * as api from "../api/endpoints";
import {
  canExportOrders,
  canRefundOrder,
  type AdminOrder,
  type OrderStatus,
  type OrderStatusFilter,
} from "../api/types";
import { useAuth } from "../auth/AuthProvider";
import { Chips, ErrorBanner, Pagination, Tag, TableSkeleton } from "../components/common";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { useToast } from "../components/ToastProvider";
import { t } from "../i18n";
import { formatAmount, formatAmountCompact, formatDateTime } from "../lib/format";
import { ORDER_STATUS_TONE, orderChannelLabel, orderStatusLabel } from "../lib/orderLabels";

const PAGE_SIZE = 20;

const filters = (): { value: OrderStatusFilter; label: string }[] => [
  { value: "all", label: t("orders.filter.all") },
  { value: "paid", label: t("orders.filter.paid") },
  { value: "refunding", label: t("orders.filter.refunding") },
  { value: "refunded", label: t("orders.filter.refunded") },
  { value: "failed", label: t("orders.filter.failed") },
];

const STATUS_TONE = ORDER_STATUS_TONE;
const statusLabel = orderStatusLabel;
const channelLabel = orderChannelLabel;

export function OrdersPage() {
  const { handleApiError, me } = useAuth();
  const { toast } = useToast();

  // 退款与导出对只读角色禁用。禁用的按钮不可聚焦，原因必须用 aria-describedby 关联，
  // 只挂 tooltip 的话键盘与读屏用户永远看不到。
  const role = me?.role ?? "";
  const canRefund = canRefundOrder(role);
  const canExport = canExportOrders(role);

  const [status, setStatus] = useState<OrderStatusFilter>("all");
  const [page, setPage] = useState(1);
  const [orders, setOrders] = useState<AdminOrder[]>([]);
  const [total, setTotal] = useState(0);
  const [today, setToday] = useState({ count: 0, amount_cents: 0 });
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [denied, setDenied] = useState("");
  const [exporting, setExporting] = useState(false);

  const [pending, setPending] = useState<AdminOrder | null>(null);
  const [busyNo, setBusyNo] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError("");
    try {
      const result = await api.fetchOrders({ status, page, pageSize: PAGE_SIZE });
      setOrders(result.items ?? []);
      setTotal(result.total);
      setToday(result.today);
    } catch (err) {
      if (!handleApiError(err)) {
        setError(
          t("orders.loadFailed", {
            message: err instanceof ApiError ? err.message : t("error.generic"),
          }),
        );
      }
    } finally {
      setLoading(false);
    }
  }, [handleApiError, page, status]);

  useEffect(() => {
    void load();
  }, [load]);

  function patchStatus(orderNo: string, next: OrderStatus) {
    setOrders((current) =>
      current.map((order) => (order.order_no === orderNo ? { ...order, status: next } : order)),
    );
  }

  async function confirmRefund() {
    if (!pending) return;
    const order = pending;

    setBusyNo(order.order_no);
    setPending(null);
    // 状态先转「退款中」，与 7.3 的 paid → refunding → refunded 流转一致。
    patchStatus(order.order_no, "refunding");
    toast(t("orders.refundStarted", { no: order.order_no }));

    try {
      const result = await api.refundOrder(order.order_no);
      patchStatus(order.order_no, result.status);
      toast(
        result.status === "refunded"
          ? t("orders.refundDone", { no: order.order_no })
          : t("orders.refundPending", { no: order.order_no }),
      );
    } catch (err) {
      patchStatus(order.order_no, order.status);
      // 403 是「这个操作不对你开放」，不是会话失效：就地说明，不换整屏。
      if (err instanceof ApiError && err.isForbidden) {
        setDenied(t("orders.refundDenied"));
      } else if (!handleApiError(err)) {
        toast(
          t("orders.refundFailed", {
            message: err instanceof ApiError ? err.message : t("error.generic"),
          }),
        );
      }
    } finally {
      setBusyNo(null);
    }
  }

  async function copyOrderNo(orderNo: string) {
    try {
      await navigator.clipboard.writeText(orderNo);
      toast(t("common.copied"));
    } catch {
      toast(t("error.copyFailed"));
    }
  }

  async function exportCSV() {
    setExporting(true);
    try {
      const blob = await api.exportOrdersCSV(status);
      const url = URL.createObjectURL(blob);
      const link = document.createElement("a");
      link.href = url;
      link.download = `orders-${status}.csv`;
      document.body.appendChild(link);
      link.click();
      link.remove();
      URL.revokeObjectURL(url);
      toast(t("orders.exported"));
    } catch (err) {
      if (err instanceof ApiError && err.isForbidden) {
        setDenied(t("orders.exportDenied"));
      } else if (!handleApiError(err)) {
        toast(
          t("orders.exportFailed", {
            message: err instanceof ApiError ? err.message : t("error.generic"),
          }),
        );
      }
    } finally {
      setExporting(false);
    }
  }

  return (
    <div className="adm-page">
      <h1>
        {t("orders.title")}
        <span className="adm-count">
          {t("orders.today", {
            n: today.count.toLocaleString("zh-CN"),
            amount: formatAmountCompact(today.amount_cents),
          })}
        </span>
      </h1>

      <div className="adm-toolbar">
        <Chips
          label={t("orders.filterAria")}
          options={filters()}
          value={status}
          onChange={(next) => {
            setStatus(next);
            setPage(1);
          }}
        />
        <span className="toolbar-spacer" />
        <button
          type="button"
          className="btn btn-secondary btn-sm"
          aria-describedby={canExport ? undefined : "export-denied-hint"}
          disabled={exporting || !canExport}
          onClick={() => void exportCSV()}
        >
          {exporting ? t("orders.exporting") : t("orders.export")}
        </button>
      </div>

      {!canExport && (
        <p className="adm-hint" id="export-denied-hint">
          {t("orders.exportDenied")}
        </p>
      )}
      {!canRefund && (
        <p className="adm-hint" id="refund-denied-hint">
          {t("orders.refundDenied")}
        </p>
      )}

      {denied && <ErrorBanner message={denied} />}
      {error && <ErrorBanner message={error} onRetry={() => void load()} />}

      <div className="adm-card flush">
        <table className="adm-table">
          <thead>
            <tr>
              <th scope="col">{t("orders.col.no")}</th>
              <th scope="col">{t("orders.col.email")}</th>
              <th scope="col" className="num">
                {t("orders.col.amount")}
              </th>
              <th scope="col">{t("orders.col.channel")}</th>
              <th scope="col">{t("orders.col.status")}</th>
              <th scope="col">{t("orders.col.paidAt")}</th>
              <th scope="col">{t("orders.col.actions")}</th>
            </tr>
          </thead>

          {loading ? (
            <TableSkeleton rows={6} cols={7} />
          ) : (
            <tbody>
              {orders.map((order) => (
                <tr key={order.order_no}>
                  <td className="mono">
                    {order.order_no}
                    <button
                      type="button"
                      className="btn btn-ghost btn-icon"
                      aria-label={t("orders.copyAria", { no: order.order_no })}
                      onClick={() => void copyOrderNo(order.order_no)}
                    >
                      ⧉
                    </button>
                  </td>
                  <td>{order.email_masked}</td>
                  <td className="num">¥{formatAmount(order.amount_cents)}</td>
                  <td>{channelLabel(order.channel)}</td>
                  <td>
                    <Tag tone={STATUS_TONE[order.status]} label={statusLabel(order.status)} />
                  </td>
                  <td>{formatDateTime(order.paid_at ?? order.created_at)}</td>
                  <td>
                    {order.status === "paid" && (
                      <button
                        type="button"
                        className="btn btn-ghost btn-sm text-red"
                        aria-describedby={canRefund ? undefined : "refund-denied-hint"}
                        disabled={!canRefund || busyNo !== null}
                        onClick={() => setPending(order)}
                      >
                        {t("orders.refund")}
                      </button>
                    )}
                  </td>
                </tr>
              ))}

              {orders.length === 0 && !error && (
                <tr>
                  <td colSpan={7} className="table-empty">
                    {t("orders.empty")}
                  </td>
                </tr>
              )}
            </tbody>
          )}
        </table>
      </div>

      <Pagination
        page={page}
        pageSize={PAGE_SIZE}
        total={total}
        disabled={loading}
        onChange={setPage}
      />

      {pending && (
        <ConfirmDialog
          title={t("orders.refundTitle")}
          body={t("orders.refundBody", {
            no: pending.order_no,
            amount: formatAmount(pending.amount_cents),
          })}
          warning={t("orders.refundWarning")}
          confirmLabel={t("orders.refund")}
          danger
          busy={busyNo !== null}
          onConfirm={() => void confirmRefund()}
          onCancel={() => setPending(null)}
        />
      )}
    </div>
  );
}
