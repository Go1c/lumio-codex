import { useState } from "react";

import { listSessions, revokeOtherSessions, revokeSession } from "@/api/endpoints";
import type { SessionView } from "@/api/types";
import { Modal } from "@/components/Modal";
import { useToast } from "@/components/Toast";
import {
  EmptyBlock,
  ErrorBlock,
  LoadingBlock,
  SectionCard,
  Spinner,
  Truncated,
  errorMessage,
} from "@/components/ui";
import { useResource } from "@/hooks/useResource";
import { useT } from "@/i18n";
import { formatRelativeTime } from "@/lib/format";

/**
 * 5.6「登录设备与授权」：浏览器会话与经浏览器授权的 APP 同列，退出即撤销授权。
 * 五态：loading 骨架行 / empty（仅当前会话时的其他设备为空）/ error + 重试 /
 * disabled（撤销进行中按钮禁用）/ 无权限不适用（本人数据）。
 */
export function DevicesSection() {
  const t = useT();
  const toast = useToast();
  const { status, data, error, reload } = useResource<{ items: SessionView[] }>(
    (signal) => listSessions(signal),
    [],
  );

  const [pending, setPending] = useState<SessionView | null>(null);
  const [confirmAll, setConfirmAll] = useState(false);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [actionError, setActionError] = useState("");

  const items = data?.items ?? [];
  const others = items.filter((item) => !item.current);

  async function doRevoke(session: SessionView) {
    setBusyId(session.id);
    setActionError("");
    try {
      await revokeSession(session.id);
      toast(t("account.device_revoked"));
      reload();
    } catch (err) {
      setActionError(errorMessage(err, t("common.unknown_error")));
    } finally {
      setBusyId(null);
      setPending(null);
    }
  }

  async function doRevokeAll() {
    setBusyId("all");
    setActionError("");
    try {
      await revokeOtherSessions();
      toast(t("account.device_revoked_all"));
      reload();
    } catch (err) {
      setActionError(errorMessage(err, t("common.unknown_error")));
    } finally {
      setBusyId(null);
      setConfirmAll(false);
    }
  }

  return (
    <SectionCard id="devices" title={t("account.devices")}>
      <p className="note" style={{ marginBottom: 10 }}>
        {t("account.devices_note")}
      </p>

      {status === "loading" && <LoadingBlock lines={3} />}

      {status === "error" && (
        <ErrorBlock error={error} fallback={t("account.load_error")} onRetry={reload} />
      )}

      {status === "success" && (
        <>
          {actionError && <ErrorBlock fallback={actionError} onRetry={reload} />}

          {items.length === 0 ? (
            <EmptyBlock icon="💻" text={t("account.device_empty")} />
          ) : (
            <ul className="device-list">
              {items.map((item) => (
                <li className="sess-row" key={item.id}>
                  <span aria-hidden="true">{item.kind === "app" ? "💻" : "🌐"}</span>
                  <div className="grow">
                    <div>
                      <Truncated text={item.device_name} max={38} />
                      {item.current && <span className="this">{t("account.device_current")}</span>}
                    </div>
                    <div className="meta">
                      {[item.platform_detail, formatRelativeTime(item.last_seen_at), item.ip_region]
                        .filter(Boolean)
                        .join(" · ")}
                    </div>
                  </div>
                  {!item.current && (
                    <button
                      type="button"
                      className="btn btn-secondary"
                      onClick={() => setPending(item)}
                      disabled={busyId !== null}
                    >
                      {busyId === item.id && <Spinner dark />}
                      {busyId === item.id ? t("account.device_revoking") : t("account.device_revoke")}
                    </button>
                  )}
                </li>
              ))}
            </ul>
          )}

          {others.length > 0 && (
            <button
              type="button"
              className="btn btn-secondary"
              style={{ marginTop: 14 }}
              onClick={() => setConfirmAll(true)}
              disabled={busyId !== null}
            >
              {t("account.device_revoke_all")}
            </button>
          )}
        </>
      )}

      {pending && (
        <Modal
          title={t("account.device_revoke")}
          onClose={() => setPending(null)}
          footer={
            <>
              <button type="button" className="btn btn-secondary" onClick={() => setPending(null)}>
                {t("common.cancel")}
              </button>
              <button
                type="button"
                className="btn btn-danger"
                onClick={() => void doRevoke(pending)}
                disabled={busyId !== null}
              >
                {t("common.confirm")}
              </button>
            </>
          }
        >
          {t("account.device_revoke_confirm", { device: pending.device_name })}
        </Modal>
      )}

      {confirmAll && (
        <Modal
          title={t("account.device_revoke_all")}
          onClose={() => setConfirmAll(false)}
          footer={
            <>
              <button type="button" className="btn btn-secondary" onClick={() => setConfirmAll(false)}>
                {t("common.cancel")}
              </button>
              <button
                type="button"
                className="btn btn-danger"
                onClick={() => void doRevokeAll()}
                disabled={busyId !== null}
              >
                {t("common.confirm")}
              </button>
            </>
          }
        >
          {t("account.device_revoke_all_confirm")}
        </Modal>
      )}
    </SectionCard>
  );
}
