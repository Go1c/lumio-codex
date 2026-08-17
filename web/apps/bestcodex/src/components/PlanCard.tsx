import { readSession } from "@lumio/auth";
import { purchaseUrl } from "@lumio/ui";

import { PLAN } from "@/content";
import { PLAN_EN } from "@/content.en";

/** 充值走 Sub2API 收银台，产品站不接自己的支付流程。 */
export function PlanCard({ locale = "zh" }: { locale?: "zh" | "en" } = {}) {
  const href = purchaseUrl(readSession()?.accessToken);
  const plan = locale === "en" ? PLAN_EN : PLAN;
  return (
    <div className="pricing-grid">
      <div className="plan featured">
        <span className="tag">{plan.tag}</span>
        <h3>{plan.name}</h3>
        <div className="price">{plan.price}</div>
        <div className="per">{plan.per}</div>
        <ul>
          {plan.features.map((feature) => (
            <li key={feature}>{feature}</li>
          ))}
        </ul>
        <p className="no-limits">{plan.noLimits}</p>
        <a className="btn btn-primary btn-block" href={href} target="_blank" rel="noreferrer">
          {locale === "en" ? "Top up" : "去充值"}
        </a>
        <div className="invite-note">
          <p>{plan.inviteLine}</p>
          <p>{plan.inviteOnce}</p>
        </div>
      </div>
    </div>
  );
}
