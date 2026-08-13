import { useState } from "react";
import { useNavigate } from "react-router-dom";

import { cancelDeletion, requestDeletion } from "@/api/endpoints";
import { Modal } from "@/components/Modal";
import { useToast } from "@/components/Toast";
import { Banner, ErrorBlock, SectionCard, Spinner, errorMessage } from "@/components/ui";
import { useT } from "@/i18n";
import { formatDate } from "@/lib/format";
import { useSession } from "@/state/session";

/** 5.6 危险区：退出登录 + 注销账号（7 天冷静期，期间可撤销）。 */
export function DangerZone() {
  const t = useT();
  const toast = useToast();
  const navigate = useNavigate();
  const { user, signOut, reload } = useSession();

  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [effectiveAt, setEffectiveAt] = useState<string | undefined>(user?.deletion_effective_at);

  async function doSignOut() {
    setBusy(true);
    try {
      await signOut();
      toast(t("account.logged_out"));
      navigate("/");
    } finally {
      setBusy(false);
    }
  }

  async function doDelete() {
    setBusy(true);
    setError("");
    try {
      const result = await requestDeletion();
      setEffectiveAt(result.effective_at);
      setConfirming(false);
      await reload();
    } catch (err) {
      setError(errorMessage(err, t("common.unknown_error")));
    } finally {
      setBusy(false);
    }
  }

  async function undoDelete() {
    setBusy(true);
    setError("");
    try {
      await cancelDeletion();
      setEffectiveAt(undefined);
      toast(t("account.delete_cancelled"));
      await reload();
    } catch (err) {
      setError(errorMessage(err, t("common.unknown_error")));
    } finally {
      setBusy(false);
    }
  }

  return (
    <SectionCard id="danger" title={t("account.danger")} className="danger-zone">
      {error && <ErrorBlock fallback={error} />}

      {effectiveAt && (
        <Banner
          kind="warn"
          action={
            <button type="button" className="btn btn-secondary" onClick={() => void undoDelete()} disabled={busy}>
              {t("account.delete_cancel")}
            </button>
          }
        >
          {t("account.delete_pending", { date: formatDate(effectiveAt) })}
        </Banner>
      )}

      <div className="danger-actions">
        <button type="button" className="btn btn-secondary" onClick={() => void doSignOut()} disabled={busy}>
          {busy && <Spinner dark />}
          {t("account.logout")}
        </button>
        {!effectiveAt && (
          <button type="button" className="btn btn-danger" onClick={() => setConfirming(true)} disabled={busy}>
            {t("account.delete")}
          </button>
        )}
      </div>

      {confirming && (
        <Modal
          title={t("account.delete_confirm_title")}
          onClose={() => setConfirming(false)}
          footer={
            <>
              <button type="button" className="btn btn-secondary" onClick={() => setConfirming(false)}>
                {t("common.cancel")}
              </button>
              <button type="button" className="btn btn-danger" onClick={() => void doDelete()} disabled={busy}>
                {busy && <Spinner dark />}
                {t("account.delete_submit")}
              </button>
            </>
          }
        >
          {t("account.delete_confirm_body")}
        </Modal>
      )}
    </SectionCard>
  );
}
