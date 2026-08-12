import { Link } from "react-router-dom";

import { ErrorBlock, Skeleton } from "@/components/ui";
import { useT } from "@/i18n";
import { formatMoney } from "@/lib/format";
import { usePublicConfig } from "@/state/publicConfig";
import { useSession } from "@/state/session";

/**
 * 4.2 节的单一套餐卡片（首页定价摘要与 /pricing 共用）。
 * 价格从 `GET /config/public` 读取，页面不写死；CTA 随订阅状态变化。
 */
export function PlanCard() {
  const t = useT();
  const { status, data, error, reload } = usePublicConfig();

  if (status === "loading") {
    return (
      <div className="pricing-grid">
        <div className="plan featured" aria-busy="true">
          <span className="sr-only">{t("common.loading")}</span>
          <Skeleton height={20} width="50%" />
          <Skeleton height={38} width="40%" />
          <Skeleton height={14} width="60%" />
          <Skeleton height={110} />
          <Skeleton height={38} />
        </div>
      </div>
    );
  }

  if (status === "error" || !data) {
    return (
      <div className="pricing-grid">
        <ErrorBlock error={error} fallback={t("pricing.load_error")} onRetry={reload} />
      </div>
    );
  }

  const price = formatMoney(data.pricing.amount_cents, data.pricing.currency);

  return (
    <div className="pricing-grid">
      <div className="plan featured">
        <span className="tag">{t("pricing.plan_tag")}</span>
        <h3>{t("pricing.plan_name")}</h3>
        <div className="price">{price}</div>
        <div className="per">{t("pricing.per")}</div>
        <ul>
          <li>{t("pricing.feature1")}</li>
          <li>{t("pricing.feature2")}</li>
          <li>{t("pricing.feature3")}</li>
          <li>{t("pricing.feature4")}</li>
          <li>{t("pricing.feature5")}</li>
        </ul>
        <p className="no-limits">{t("pricing.no_limits")}</p>
        <PlanCTA />
        <p className="invite-note">{t("pricing.invite_note")}</p>
      </div>
    </div>
  );
}

function PlanCTA() {
  const t = useT();
  const { status, entitlement } = useSession();

  if (status === "loading") {
    return <Skeleton height={38} />;
  }

  if (status === "anonymous") {
    return (
      <Link to="/signup" className="btn btn-primary btn-block">
        {t("pricing.cta_subscribe")}
      </Link>
    );
  }

  if (entitlement?.status === "active") {
    return (
      <>
        <span className="badge blue">{t("pricing.badge_subscribed")}</span>
        <Link to="/account" className="btn btn-secondary btn-block" style={{ marginTop: 10 }}>
          {t("pricing.cta_manage")}
        </Link>
      </>
    );
  }

  if (entitlement?.status === "trialing") {
    return (
      <>
        <span className="badge green">{t("pricing.badge_trialing", { n: entitlement.days_left })}</span>
        <Link to="/account" className="btn btn-primary btn-block" style={{ marginTop: 10 }}>
          {t("pricing.cta_subscribe")}
        </Link>
      </>
    );
  }

  return (
    <Link to="/account" className="btn btn-primary btn-block">
      {t("pricing.cta_subscribe")}
    </Link>
  );
}
