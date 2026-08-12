import { useCallback, useEffect, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { ApiError } from "../api/client";
import * as api from "../api/endpoints";
import {
  canManageUsers,
  type AdminUserDetail,
  type Entitlement,
  type EntitlementStatus,
} from "../api/types";
import { useAuth } from "../auth/AuthProvider";
import { ErrorBanner, Tag } from "../components/common";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { useToast } from "../components/ToastProvider";
import { t } from "../i18n";
import { formatAmount, formatDateTime, formatRelative, orDash } from "../lib/format";
import { ORDER_STATUS_TONE, orderChannelLabel, orderStatusLabel } from "../lib/orderLabels";

const ENT_TONE: Record<EntitlementStatus, "blue" | "green" | "gray" | "red"> = {
  active: "blue",
  trialing: "green",
  none: "gray",
  expired: "red",
};

function entitlementLabel(entitlement: Entitlement): string {
  switch (entitlement.status) {
    case "active":
      return t("detail.ent.status.active");
    case "trialing":
      return t("detail.ent.status.trialing");
    case "expired":
      return t("detail.ent.status.expired");
    default:
      return t("detail.ent.status.none");
  }
}

function kindLabel(kind: string | undefined): string {
  if (kind === "trial") return t("detail.ent.kind.trial");
  if (kind === "paid") return t("detail.ent.kind.paid");
  return orDash(kind);
}

function referralStatusLabel(status: string): string {
  if (status === "activated") return t("detail.referral.status.activated");
  if (status === "registered") return t("detail.referral.status.registered");
  return status;
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="kv">
      <dt>{label}</dt>
      <dd>{children}</dd>
    </div>
  );
}

function EntitlementBlock({ entitlement }: { entitlement: Entitlement }) {
  return (
    <dl className="kv-grid">
      <Field label={t("detail.ent.status")}>
        <Tag tone={ENT_TONE[entitlement.status] ?? "gray"} label={entitlementLabel(entitlement)} />
        {entitlement.expiring_soon && (
          <span className="text-orange"> {t("detail.ent.expiringSoon")}</span>
        )}
      </Field>
      <Field label={t("detail.ent.kind")}>{kindLabel(entitlement.kind)}</Field>
      <Field label={t("detail.ent.expiresAt")}>
        {entitlement.expires_at ? formatDateTime(entitlement.expires_at) : orDash(null)}
      </Field>
      <Field label={t("detail.ent.daysLeft")}>
        {t("detail.ent.days", { n: entitlement.days_left })}
      </Field>
      <Field label={t("detail.ent.bonusDays")}>
        {t("detail.ent.days", { n: entitlement.bonus_days_total })}
      </Field>
    </dl>
  );
}

export function UserDetailPage() {
  const { userId } = useParams();
  const navigate = useNavigate();
  const { handleApiError, me } = useAuth();
  const { toast } = useToast();

  // 能进到本页的角色目前都能禁用/解禁，但矩阵是两格而不是一格，
  // 将来若拆开（能看详情却不能禁用），入口要是禁用态而不是一个可点的 403。
  const canManage = canManageUsers(me?.role ?? "");

  const [detail, setDetail] = useState<AdminUserDetail | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [forbidden, setForbidden] = useState(false);
  const [denied, setDenied] = useState("");
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);

  const numericID = Number(userId);

  const load = useCallback(async () => {
    setLoading(true);
    setError("");
    setForbidden(false);
    try {
      setDetail(await api.fetchUserDetail(numericID));
    } catch (err) {
      // 403 只说明这个资源不对当前角色开放，会话本身仍然有效，
      // 所以就地渲染 403，不把整个后台踢回登录页。
      if (err instanceof ApiError && err.isForbidden) {
        setForbidden(true);
      } else if (!handleApiError(err)) {
        setError(
          t("detail.loadFailed", {
            message: err instanceof ApiError ? err.message : t("error.generic"),
          }),
        );
      }
    } finally {
      setLoading(false);
    }
  }, [handleApiError, numericID]);

  useEffect(() => {
    void load();
  }, [load]);

  const disabled = detail?.user.status === "disabled";

  async function confirmToggle() {
    if (!detail) return;
    setBusy(true);
    try {
      await api.setUserDisabled(detail.user.user_id, !disabled);
      setConfirming(false);
      toast(
        !disabled
          ? t("users.disabled", { id: detail.user.id })
          : t("users.enabled", { id: detail.user.id }),
      );
      await load();
    } catch (err) {
      if (err instanceof ApiError && err.isForbidden) {
        setConfirming(false);
        setDenied(t("users.manageDenied"));
      } else if (!handleApiError(err)) {
        setConfirming(false);
        toast(
          t("users.actionFailed", {
            message: err instanceof ApiError ? err.message : t("error.generic"),
          }),
        );
      }
    } finally {
      setBusy(false);
    }
  }

  const back = (
    <Link className="adm-back" to="/users">
      {t("detail.back")}
    </Link>
  );

  if (forbidden) {
    return (
      <div className="adm-page">
        {back}
        <div className="adm-card forbidden-inline" role="alert">
          <div className="forbidden-art" aria-hidden="true">
            🔒
          </div>
          <h1>{t("forbidden.title")}</h1>
          <p className="sub">{t("users.detailDenied")}</p>
          <button type="button" className="btn btn-secondary" onClick={() => navigate("/users")}>
            {t("detail.back")}
          </button>
        </div>
      </div>
    );
  }

  if (loading) {
    return (
      <div className="adm-page" data-testid="detail-skeleton">
        {back}
        <div className="skeleton skeleton-card" aria-hidden="true" />
        <div className="skeleton skeleton-card" aria-hidden="true" />
        <span className="sr-only" role="status">
          {t("common.loading")}
        </span>
      </div>
    );
  }

  if (error || !detail) {
    return (
      <div className="adm-page">
        {back}
        <ErrorBanner message={error || t("detail.notFound")} onRetry={() => void load()} />
      </div>
    );
  }

  const { user, entitlement, referral } = detail;
  const devices = detail.devices ?? [];
  const orders = detail.orders ?? [];
  const referralItems = referral.items ?? [];

  return (
    <div className="adm-page">
      {back}

      <h1>
        {t("detail.title")}
        <span className="adm-count mono">{user.id}</span>
      </h1>

      {/* 看明文邮箱这件事本身会被记录，操作者有权先知道。 */}
      <p className="banner warn" role="note">
        {t("detail.auditNotice")}
      </p>

      {!canManage && (
        <p className="adm-hint" id="detail-manage-denied-hint">
          {t("users.manageDenied")}
        </p>
      )}
      {denied && <ErrorBanner message={denied} />}

      <section className="adm-card">
        <div className="card-head">
          <h2>{t("detail.account")}</h2>
          <button
            type="button"
            className={`btn btn-secondary btn-sm ${disabled ? "text-green" : "text-red"}`}
            aria-describedby={canManage ? undefined : "detail-manage-denied-hint"}
            disabled={!canManage || busy}
            onClick={() => setConfirming(true)}
          >
            {disabled ? t("users.enable") : t("users.disable")}
          </button>
        </div>

        <dl className="kv-grid">
          <Field label={t("detail.field.id")}>
            <span className="mono">{user.id}</span>
          </Field>
          <Field label={t("detail.field.email")}>
            <span className="mono">{user.email}</span>
          </Field>
          <Field label={t("detail.field.displayName")}>{orDash(user.display_name)}</Field>
          <Field label={t("detail.field.status")}>
            <Tag
              tone={disabled ? "red" : "green"}
              label={disabled ? t("detail.status.disabled") : t("detail.status.active")}
            />
          </Field>
          <Field label={t("detail.field.createdAt")}>{formatDateTime(user.created_at)}</Field>
          <Field label={t("detail.field.source")}>
            {user.inviter_id
              ? t("users.source.invite", { inviter: user.inviter_id })
              : user.source}
          </Field>
          <Field label={t("detail.field.lastActive")}>
            {user.last_active_at ? formatRelative(user.last_active_at) : orDash(null)}
          </Field>
          {user.deletion_requested_at && (
            <Field label={t("detail.field.deletionRequestedAt")}>
              {formatDateTime(user.deletion_requested_at)}
            </Field>
          )}
        </dl>

        {user.deletion_requested_at && (
          <p className="adm-hint text-orange">{t("detail.deletionPending")}</p>
        )}
      </section>

      <section className="adm-card">
        <h2>{t("detail.entitlement")}</h2>
        <EntitlementBlock entitlement={entitlement} />
      </section>

      <section className="adm-card flush">
        <h2 className="card-title">{t("detail.devices")}</h2>
        <table className="adm-table">
          <thead>
            <tr>
              <th scope="col">{t("detail.devices.col.id")}</th>
              <th scope="col">{t("detail.devices.col.platform")}</th>
              <th scope="col">{t("detail.devices.col.version")}</th>
              <th scope="col">{t("detail.devices.col.firstSeen")}</th>
              <th scope="col">{t("detail.devices.col.lastSeen")}</th>
            </tr>
          </thead>
          <tbody>
            {devices.map((device) => (
              <tr key={device.device_id}>
                <td className="mono">{device.device_id}</td>
                <td>{device.platform}</td>
                <td>{orDash(device.app_version)}</td>
                <td>{formatDateTime(device.first_seen_at)}</td>
                <td>{formatDateTime(device.last_seen_at)}</td>
              </tr>
            ))}
            {devices.length === 0 && (
              <tr>
                <td colSpan={5} className="table-empty">
                  {t("detail.devices.empty")}
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </section>

      <section className="adm-card flush">
        <div className="card-head">
          <h2 className="card-title">{t("detail.referral")}</h2>
          <span className="adm-hint">
            {t("detail.referral.summary", {
              n: referral.invited_count,
              days: referral.total_bonus_days,
            })}
          </span>
        </div>
        <table className="adm-table">
          <thead>
            <tr>
              <th scope="col">{t("detail.referral.col.invitee")}</th>
              <th scope="col">{t("detail.referral.col.status")}</th>
              <th scope="col" className="num">
                {t("detail.referral.col.bonus")}
              </th>
              <th scope="col">{t("detail.referral.col.at")}</th>
            </tr>
          </thead>
          <tbody>
            {referralItems.map((item) => (
              <tr key={`${item.email_masked}-${item.at}`}>
                {/* 被邀请者是另一个用户，其邮箱不在本页的授权范围内，保持打码。 */}
                <td>{item.email_masked}</td>
                <td>{referralStatusLabel(item.status)}</td>
                <td className="num">{t("detail.ent.days", { n: item.bonus_days })}</td>
                <td>{formatDateTime(item.at)}</td>
              </tr>
            ))}
            {referralItems.length === 0 && (
              <tr>
                <td colSpan={4} className="table-empty">
                  {t("detail.referral.empty")}
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </section>

      <section className="adm-card flush">
        <div className="card-head">
          <h2 className="card-title">{t("detail.orders")}</h2>
          <span className="adm-hint">{t("detail.orders.hint")}</span>
        </div>
        <table className="adm-table">
          <thead>
            <tr>
              <th scope="col">{t("orders.col.no")}</th>
              <th scope="col" className="num">
                {t("orders.col.amount")}
              </th>
              <th scope="col">{t("orders.col.channel")}</th>
              <th scope="col">{t("orders.col.status")}</th>
              <th scope="col">{t("orders.col.paidAt")}</th>
            </tr>
          </thead>
          <tbody>
            {orders.map((order) => (
              <tr key={order.order_no}>
                <td className="mono">{order.order_no}</td>
                <td className="num">¥{formatAmount(order.amount_cents)}</td>
                <td>{orderChannelLabel(order.channel)}</td>
                <td>
                  <Tag
                    tone={ORDER_STATUS_TONE[order.status] ?? "gray"}
                    label={orderStatusLabel(order.status)}
                  />
                </td>
                <td>{order.paid_at ? formatDateTime(order.paid_at) : orDash(null)}</td>
              </tr>
            ))}
            {orders.length === 0 && (
              <tr>
                <td colSpan={5} className="table-empty">
                  {t("detail.orders.empty")}
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </section>

      {confirming && (
        <ConfirmDialog
          title={disabled ? t("users.enableTitle") : t("users.disableTitle")}
          body={
            disabled
              ? t("users.enableBody", { id: user.id, email: user.email })
              : t("users.disableBody", { id: user.id, email: user.email })
          }
          warning={disabled ? undefined : t("users.disableWarning")}
          confirmLabel={disabled ? t("users.enable") : t("users.disable")}
          danger={!disabled}
          busy={busy}
          onConfirm={() => void confirmToggle()}
          onCancel={() => setConfirming(false)}
        />
      )}
    </div>
  );
}
