import { useCallback, useEffect, useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import { ApiError } from "../api/client";
import * as api from "../api/endpoints";
import {
  canManageUsers,
  canViewUserDetail,
  type AdminUser,
  type SubState,
  type UserStatusFilter,
} from "../api/types";
import { useAuth } from "../auth/AuthProvider";
import { Chips, ErrorBanner, Pagination, Tag, TableSkeleton } from "../components/common";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { useToast } from "../components/ToastProvider";
import { t } from "../i18n";
import { formatDateTime, formatPlatform, formatRelative } from "../lib/format";

const PAGE_SIZE = 20;
const SEARCH_DEBOUNCE_MS = 300;

// 文案在渲染时取，切换语言后无需重挂载组件。
const filters = (): { value: UserStatusFilter; label: string }[] => [
  { value: "all", label: t("users.filter.all") },
  { value: "sub", label: t("users.filter.sub") },
  { value: "trial", label: t("users.filter.trial") },
  { value: "none", label: t("users.filter.none") },
  { value: "banned", label: t("users.filter.banned") },
];

const SUB_TONE: Record<SubState, "blue" | "green" | "gray" | "red"> = {
  sub: "blue",
  trial: "green",
  none: "gray",
  banned: "red",
};

const subLabel = (state: SubState): string => t(`users.filter.${state}`);

function sourceText(user: AdminUser): string {
  if (user.inviter_id) return t("users.source.invite", { inviter: user.inviter_id });
  return user.source;
}

export function UsersPage() {
  const { handleApiError, me } = useAuth();
  const { toast } = useToast();
  const navigate = useNavigate();
  // 无权的入口一律禁用并说明原因，而不是让人点下去吃一个 403。
  // 禁用的按钮不可聚焦，tooltip 读不到，所以原因文字要用 aria-describedby 关联。
  const role = me?.role ?? "";
  const canOpenDetail = canViewUserDetail(role);
  const canManage = canManageUsers(role);

  const [search, setSearch] = useState("");
  const [query, setQuery] = useState("");
  const [status, setStatus] = useState<UserStatusFilter>("all");
  const [page, setPage] = useState(1);

  const [users, setUsers] = useState<AdminUser[]>([]);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");

  const [pending, setPending] = useState<AdminUser | null>(null);
  const [busyID, setBusyID] = useState<string | null>(null);
  const [denied, setDenied] = useState("");

  useEffect(() => {
    const timer = setTimeout(() => {
      setQuery(search.trim());
      setPage(1);
    }, SEARCH_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [search]);

  const load = useCallback(async () => {
    setLoading(true);
    setError("");
    try {
      const result = await api.fetchUsers({ query, status, page, pageSize: PAGE_SIZE });
      setUsers(result.items ?? []);
      setTotal(result.total);
    } catch (err) {
      if (!handleApiError(err)) {
        setError(
          t("users.loadFailed", {
            message: err instanceof ApiError ? err.message : t("error.generic"),
          }),
        );
      }
    } finally {
      setLoading(false);
    }
  }, [handleApiError, page, query, status]);

  useEffect(() => {
    void load();
  }, [load]);

  const disabling = pending ? pending.sub_state !== "banned" : false;

  async function confirmToggle() {
    if (!pending) return;
    const target = pending;
    setBusyID(target.id);
    try {
      // 用数字主键 user_id，不是展示号 target.id。
      await api.setUserDisabled(target.user_id, disabling);
      setPending(null);
      toast(disabling ? t("users.disabled", { id: target.id }) : t("users.enabled", { id: target.id }));
      await load();
    } catch (err) {
      // 403 只说明这个操作不对当前角色开放，会话仍然有效：就地说明原因，
      // 不走 handleApiError 把整屏换成 403 页、更不退回登录页。
      if (err instanceof ApiError && err.isForbidden) {
        setPending(null);
        setDenied(t("users.manageDenied"));
      } else if (!handleApiError(err)) {
        setPending(null);
        toast(
          t("users.actionFailed", {
            message: err instanceof ApiError ? err.message : t("error.generic"),
          }),
        );
      }
    } finally {
      setBusyID(null);
    }
  }

  const countLabel = useMemo(() => t("users.count", { n: total.toLocaleString("zh-CN") }), [total]);

  return (
    <div className="adm-page">
      <h1>
        {t("users.title")}
        <span className="adm-count">{countLabel}</span>
      </h1>

      <div className="adm-toolbar">
        <input
          className="adm-search"
          type="search"
          placeholder={t("users.search")}
          aria-label={t("users.searchAria")}
          value={search}
          onChange={(event) => setSearch(event.target.value)}
        />
        <Chips
          label={t("users.filterAria")}
          options={filters()}
          value={status}
          onChange={(next) => {
            setStatus(next);
            setPage(1);
          }}
        />
      </div>

      {!canOpenDetail && (
        <p className="adm-hint" id="detail-denied-hint">
          {t("users.detailDenied")}
        </p>
      )}
      {!canManage && (
        <p className="adm-hint" id="manage-denied-hint">
          {t("users.manageDenied")}
        </p>
      )}

      {denied && <ErrorBanner message={denied} />}
      {error && <ErrorBanner message={error} onRetry={() => void load()} />}

      <div className="adm-card flush">
        <table className="adm-table">
          <thead>
            <tr>
              <th scope="col">{t("users.col.id")}</th>
              <th scope="col">{t("users.col.email")}</th>
              <th scope="col">{t("users.col.regAt")}</th>
              <th scope="col">{t("users.col.source")}</th>
              <th scope="col">{t("users.col.platform")}</th>
              <th scope="col">{t("users.col.sub")}</th>
              <th scope="col">{t("users.col.lastActive")}</th>
              <th scope="col">{t("users.col.actions")}</th>
            </tr>
          </thead>

          {loading ? (
            <TableSkeleton rows={6} cols={8} />
          ) : (
            <tbody>
              {users.map((user) => {
                const banned = user.sub_state === "banned";
                return (
                  <tr key={user.id}>
                    <td className="mono">{user.id}</td>
                    <td>{user.email_masked}</td>
                    <td>{formatDateTime(user.created_at)}</td>
                    <td>{sourceText(user)}</td>
                    <td>{formatPlatform(user.platform)}</td>
                    <td>
                      <Tag tone={SUB_TONE[user.sub_state]} label={subLabel(user.sub_state)} />
                    </td>
                    <td>{formatRelative(user.last_active_at)}</td>
                    <td className="row-actions">
                      <button
                        type="button"
                        className="btn btn-ghost btn-sm"
                        aria-label={t("users.detailAria", { id: user.id })}
                        aria-describedby={canOpenDetail ? undefined : "detail-denied-hint"}
                        disabled={!canOpenDetail}
                        onClick={() => navigate(`/users/${user.user_id}`)}
                      >
                        {t("users.detail")}
                      </button>
                      <button
                        type="button"
                        className={`btn btn-ghost btn-sm ${banned ? "text-green" : "text-red"}`}
                        aria-describedby={canManage ? undefined : "manage-denied-hint"}
                        disabled={!canManage || busyID !== null}
                        onClick={() => setPending(user)}
                      >
                        {banned ? t("users.enable") : t("users.disable")}
                      </button>
                    </td>
                  </tr>
                );
              })}

              {users.length === 0 && !error && (
                <tr>
                  <td colSpan={8} className="table-empty">
                    {t("users.empty")}
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
          title={disabling ? t("users.disableTitle") : t("users.enableTitle")}
          body={
            disabling
              ? t("users.disableBody", { id: pending.id, email: pending.email_masked })
              : t("users.enableBody", { id: pending.id, email: pending.email_masked })
          }
          warning={disabling ? t("users.disableWarning") : undefined}
          confirmLabel={disabling ? t("users.disable") : t("users.enable")}
          danger={disabling}
          busy={busyID !== null}
          onConfirm={() => void confirmToggle()}
          onCancel={() => setPending(null)}
        />
      )}
    </div>
  );
}
