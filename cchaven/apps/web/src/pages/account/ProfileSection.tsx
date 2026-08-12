import { useState } from "react";

import { updateProfile } from "@/api/endpoints";
import { TextField } from "@/components/fields";
import { useToast } from "@/components/Toast";
import { ErrorBlock, SectionCard, Spinner, Truncated, errorMessage } from "@/components/ui";
import { useT } from "@/i18n";
import { useSession } from "@/state/session";

/** 5.6「个人资料」：邮箱只读（长邮箱中间省略号截断），显示名称可编辑。 */
export function ProfileSection() {
  const t = useT();
  const toast = useToast();
  const { user, patchUser } = useSession();

  const [displayName, setDisplayName] = useState(user?.display_name ?? "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  async function save() {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      patchUser(await updateProfile(displayName.trim()));
      toast(t("account.profile_saved"));
    } catch (err) {
      setError(errorMessage(err, t("common.unknown_error")));
    } finally {
      setBusy(false);
    }
  }

  return (
    <SectionCard id="profile" title={t("account.profile")}>
      {error && <ErrorBlock fallback={error} onRetry={() => void save()} />}

      <div className="field">
        <span className="profile-label">{t("signup.email")}</span>
        <div className="readonly-value">
          <Truncated text={user?.email ?? ""} max={36} />
        </div>
        <div className="hint">账号 {user?.id}</div>
      </div>

      <div className="inline-form">
        <TextField
          label={t("account.display_name")}
          value={displayName}
          maxLength={40}
          onChange={(event) => setDisplayName(event.target.value)}
        />
        <button
          type="button"
          className="btn btn-secondary profile-save"
          onClick={() => void save()}
          disabled={busy || displayName.trim() === (user?.display_name ?? "")}
        >
          {busy && <Spinner dark />}
          {busy ? t("common.saving") : t("common.save")}
        </button>
      </div>
    </SectionCard>
  );
}
