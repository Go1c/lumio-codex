import { Link } from "react-router-dom";

import { Banner, LoadingBlock } from "@/components/ui";
import { useT } from "@/i18n";
import { useSession } from "@/state/session";

import { DangerZone } from "./DangerZone";
import { DevicesSection } from "./DevicesSection";
import { ProfileSection } from "./ProfileSection";
import { ReferralSection } from "./ReferralSection";
import { SecuritySection } from "./SecuritySection";
import { SubscriptionSection } from "./SubscriptionSection";

/**
 * 5.6 官网账户中心：订阅与付款 / 个人资料 / 邀请好友 / 安全 / 登录设备与授权 + 危险区。
 * 页面级五态：loading（会话查询中）/ 无权限（未登录）/ 其余由各分区自己覆盖。
 */
export function Account() {
  const t = useT();
  const { status, user } = useSession();

  if (status === "loading") {
    return (
      <div className="acct">
        <h2>{t("account.title")}</h2>
        <div className="acct-section">
          <LoadingBlock lines={4} />
        </div>
      </div>
    );
  }

  if (status === "anonymous" || !user) {
    return (
      <div className="acct">
        <Banner
          kind="warn"
          action={
            <Link to="/login?next=/account" className="btn btn-primary btn-sm">
              {t("common.go_login")}
            </Link>
          }
        >
          {t("common.login_required")}
        </Banner>
      </div>
    );
  }

  return (
    <div className="acct">
      <h2>{t("account.title")}</h2>
      <SubscriptionSection />
      <ProfileSection />
      <ReferralSection />
      <SecuritySection />
      <DevicesSection />
      <DangerZone />
    </div>
  );
}
