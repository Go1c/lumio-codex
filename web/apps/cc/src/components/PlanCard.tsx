import { purchaseUrl } from "@lumio/ui";

import { PLAN } from "@/content";

/** 订阅统一走 Sub2API 收银台，产品站不接自己的支付流程。 */
export function PlanCard() {
  return (
    <div className="pricing-grid">
      <div className="plan featured">
        <span className="tag">{PLAN.tag}</span>
        <h3>{PLAN.name}</h3>
        <div className="price">
          {PLAN.price}
          <span className="price-unit"> / 月</span>
        </div>
        <div className="per">{PLAN.per}</div>
        <ul>
          {PLAN.features.map((feature) => (
            <li key={feature}>{feature}</li>
          ))}
        </ul>
        <p className="no-limits">{PLAN.noLimits}</p>
        <a className="btn btn-primary btn-block" href={purchaseUrl()} target="_blank" rel="noreferrer">
          立即订阅
        </a>
        <div className="invite-note">
          <p>{PLAN.inviteLine}</p>
          <p>{PLAN.inviteOnce}</p>
        </div>
      </div>
    </div>
  );
}
