import { useCallback, useEffect, useState } from "react";
import { ApiError } from "../api/client";
import * as api from "../api/endpoints";
import type { AuditRecord } from "../api/types";
import { useAuth } from "../auth/AuthProvider";
import { ErrorBanner, Pagination, TableSkeleton } from "../components/common";
import { t, type MessageKey } from "../i18n";
import { formatDateTime } from "../lib/format";

const PAGE_SIZE = 10;
const SEARCH_DEBOUNCE_MS = 300;

// 每个受权限控制的动作都有一条 `_denied` 对应项：后端在拒绝前先写审计，
// 「谁试图越权做了什么」要能在这里直接筛出来。
const ACTION_LABELS: Record<string, MessageKey> = {
  "user.disable": "audit.action.userDisable",
  "user.enable": "audit.action.userEnable",
  "user.view_detail": "audit.action.userViewDetail",
  "order.refund": "audit.action.orderRefund",
  "ops_config.update": "audit.action.configUpdate",
  "user.view_detail_denied": "audit.action.userViewDenied",
  "user.disable_denied": "audit.action.userDisableDenied",
  "user.enable_denied": "audit.action.userEnableDenied",
  "order.refund_denied": "audit.action.orderRefundDenied",
  "ops_config.update_denied": "audit.action.configUpdateDenied",
  "orders.export_denied": "audit.action.orderExportDenied",
};

function actionLabel(action: string): string {
  const key = ACTION_LABELS[action];
  return key ? t(key) : action;
}

/** before/after 是任意 JSON，紧凑成一行展示；缺值显示「—」。 */
function formatValue(value: unknown): string {
  if (value === null || value === undefined) return "—";
  if (typeof value === "string") return value;
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

/**
 * 审计日志。7.5 要求破坏性操作留痕（操作人 + 时间 + 前后值），
 * 这里挂在运营配置页下，保持侧栏仍是规范里的四个页面。
 */
export function AuditLogSection() {
  const { handleApiError } = useAuth();
  const [records, setRecords] = useState<AuditRecord[]>([]);
  const [total, setTotal] = useState(0);
  const [page, setPage] = useState(1);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");

  // 操作人是自由输入，防抖后再打请求；动作是枚举，选中即生效。
  const [actorInput, setActorInput] = useState("");
  const [actor, setActor] = useState("");
  const [action, setAction] = useState("");
  const filtered = actor !== "" || action !== "";

  useEffect(() => {
    const timer = setTimeout(() => {
      setActor(actorInput.trim());
      setPage(1);
    }, SEARCH_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [actorInput]);

  const load = useCallback(async () => {
    setLoading(true);
    setError("");
    try {
      const result = await api.fetchAuditLogs({ actor, action, page, pageSize: PAGE_SIZE });
      setRecords(result.items ?? []);
      setTotal(result.total);
    } catch (err) {
      if (!handleApiError(err)) {
        setError(
          t("audit.loadFailed", {
            message: err instanceof ApiError ? err.message : t("error.generic"),
          }),
        );
      }
    } finally {
      setLoading(false);
    }
  }, [action, actor, handleApiError, page]);

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <section className="adm-card audit-section">
      <h2>{t("audit.title")}</h2>
      <p className="muted">{t("audit.intro")}</p>

      <div className="adm-toolbar">
        <input
          className="adm-search"
          type="search"
          inputMode="numeric"
          placeholder={t("audit.filter.actorPlaceholder")}
          aria-label={t("audit.filter.actor")}
          value={actorInput}
          onChange={(event) => setActorInput(event.target.value)}
        />
        <select
          className="adm-select"
          aria-label={t("audit.filter.action")}
          value={action}
          onChange={(event) => {
            setAction(event.target.value);
            setPage(1);
          }}
        >
          <option value="">{t("audit.filter.allActions")}</option>
          {Object.keys(ACTION_LABELS).map((value) => (
            <option key={value} value={value}>
              {actionLabel(value)}
            </option>
          ))}
        </select>
        {filtered && (
          <button
            type="button"
            className="btn btn-ghost btn-sm"
            onClick={() => {
              setActorInput("");
              setActor("");
              setAction("");
              setPage(1);
            }}
          >
            {t("audit.filter.reset")}
          </button>
        )}
      </div>

      {error && <ErrorBanner message={error} onRetry={() => void load()} />}

      <div className="table-scroll">
        <table className="adm-table">
          <thead>
            <tr>
              <th scope="col">{t("audit.col.actor")}</th>
              <th scope="col">{t("audit.col.at")}</th>
              <th scope="col">{t("audit.col.action")}</th>
              <th scope="col">{t("audit.col.target")}</th>
              <th scope="col">{t("audit.col.before")}</th>
              <th scope="col">{t("audit.col.after")}</th>
            </tr>
          </thead>

          {loading ? (
            <TableSkeleton rows={3} cols={6} />
          ) : (
            <tbody>
              {records.map((record) => (
                <tr key={record.id}>
                  <td>{t("audit.actor", { id: record.actor_id })}</td>
                  <td>{formatDateTime(record.created_at)}</td>
                  <td>{actionLabel(record.action)}</td>
                  <td className="mono">
                    {record.target_type}:{record.target_id}
                  </td>
                  <td className="mono dim">{formatValue(record.before)}</td>
                  <td className="mono dim">{formatValue(record.after)}</td>
                </tr>
              ))}

              {records.length === 0 && !error && (
                <tr>
                  <td colSpan={6} className="table-empty">
                    {filtered ? t("audit.emptyFiltered") : t("audit.empty")}
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
    </section>
  );
}
