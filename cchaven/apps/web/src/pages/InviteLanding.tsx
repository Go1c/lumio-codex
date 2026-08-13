import { useEffect } from "react";
import { Link, useParams } from "react-router-dom";

import { getInviteLanding } from "@/api/endpoints";
import type { InviteLanding as InviteLandingData } from "@/api/types";
import { ErrorBlock, LoadingBlock } from "@/components/ui";
import { useResource } from "@/hooks/useResource";
import { useT } from "@/i18n";
import { useSeedInviteAttribution } from "@/state/inviteAttribution";

/**
 * 4.4 邀请落地页 `/i/{code}`。
 *
 * 归因由服务端在这次请求里下发 HttpOnly cookie 完成（这里同时记 `referral_visits`，
 * 所以必须走 `/invites/{code}`）。响应与 `/invites/current` 同口径，就地写进共享状态，
 * 同一次会话里跳到注册页时横幅无需再问一次。`valid:false` 时展示「此邀请链接已失效」但不阻断注册。
 */
export function InviteLandingPage() {
  const t = useT();
  const { code = "" } = useParams();
  const seedAttribution = useSeedInviteAttribution();

  const { status, data, error, reload } = useResource<InviteLandingData>(
    (signal) => getInviteLanding(code, signal),
    [code],
  );

  // 只在 valid 时写入：失效的邀请码不会清掉浏览器上原有的 cch_ref，
  // 此时该不该显示横幅仍由 `/invites/current` 裁决，前端别替服务端下结论。
  useEffect(() => {
    if (!data?.valid) return;
    seedAttribution({ attributed: true, inviter: data.inviter, trial_days: data.trial_days });
  }, [data, seedAttribution]);

  if (status === "loading") {
    return (
      <div className="auth-page">
        <div className="auth-card wide">
          <LoadingBlock lines={4} />
        </div>
      </div>
    );
  }

  if (status === "error" || !data) {
    return (
      <div className="auth-page">
        <div className="auth-card wide">
          <ErrorBlock error={error} fallback={t("invite.load_error")} onRetry={reload} />
          <Link to="/signup" className="btn btn-secondary btn-block">
            {t("invite.invalid_cta")}
          </Link>
        </div>
      </div>
    );
  }

  if (!data.valid) {
    return (
      <div className="auth-page">
        <div className="auth-card wide">
          <div style={{ fontSize: 40, marginBottom: 8 }} aria-hidden="true">
            🔗
          </div>
          <h2>{t("invite.invalid_title")}</h2>
          <p className="sub">{t("invite.invalid_body")}</p>
          <Link to="/signup" className="btn btn-primary btn-block">
            {t("invite.invalid_cta")}
          </Link>
          <div className="auth-links">
            <Link to="/download">{t("invite.cta_download")}</Link>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="auth-page">
      <div className="auth-card wide">
        <div style={{ fontSize: 40, marginBottom: 8 }} aria-hidden="true">
          🎁
        </div>
        <h2>{t("invite.title", { inviter: data.inviter || "朋友" })}</h2>
        <p className="sub">
          {t("invite.value")}
          <br />
          <strong>{t("invite.highlight")}</strong>
        </p>
        <Link to="/signup" className="btn btn-primary btn-lg btn-block">
          {t("invite.cta_signup")}
        </Link>
        <Link to="/download" className="btn btn-secondary btn-block" style={{ marginTop: 10 }}>
          {t("invite.cta_download")}
        </Link>
        <div className="terms">{t("invite.footnote", { code: data.code })}</div>
      </div>
    </div>
  );
}
