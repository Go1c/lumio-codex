import type { ReactNode } from "react";

import { claudeEntitlementHeadline } from "../../claude/copy.ts";
import { formatClaudePlanYuan } from "../../claude/session.ts";
import type { ClaudeEntitlement, ClaudePayMode } from "../../claude/types.ts";

export function ClaudeSubscribe({
  balance,
  planAmountCents,
  paying,
  payMode,
  entitlement,
  onPay,
  onRecharge,
  onBackToCodex,
  ordersSlot,
}: {
  balance: number;
  planAmountCents: number;
  paying: boolean;
  payMode: ClaudePayMode;
  entitlement: ClaudeEntitlement;
  onPay: () => void;
  onRecharge: () => void;
  onBackToCodex: () => void;
  ordersSlot?: ReactNode;
}) {
  const priceLabel = formatClaudePlanYuan(planAmountCents);
  const needsRecharge = payMode === "recharge" || Math.round(balance * 100) < planAmountCents;

  return (
    <main className="lumio-claude-onboard">
      <div className="lumio-claude-card is-subscribe">
        <span className="lumio-claude-icon" aria-hidden="true">
          <img alt="" src="/lumio-icon.png" />
        </span>
        <span className="lumio-claude-chip">Claude</span>
        <h2>
          在自己的服务器上
          <br />
          跑 Claude
        </h2>
        <p className="lumio-claude-entitlement">{claudeEntitlementHeadline(entitlement.status)}</p>
        <p className="lumio-claude-price">
          ¥{priceLabel}
          <span> / 月</span>
        </p>
        <p>独立环境、双向同步、不限项目。用现在这个账号开通即可。</p>
        <p className="lumio-claude-balance">余额 ¥{balance.toFixed(2)}</p>
        <button
          className="lumio-button is-primary is-large"
          disabled={paying}
          onClick={needsRecharge ? onRecharge : onPay}
          type="button"
        >
          {paying ? "正在支付…" : needsRecharge ? "余额不足，去充值" : `用余额支付 ¥${priceLabel}`}
        </button>
        {ordersSlot}
        <p className="lumio-claude-quiet">
          <button className="lumio-link-button" onClick={onBackToCodex} type="button">
            回到 Codex Tab
          </button>
          ，先启动官方应用。
        </p>
      </div>
    </main>
  );
}
